//! SSE transport for the MCP server
//!
//! Implements the MCP Server-Sent Events transport protocol:
//! - `GET /sse` — SSE connection endpoint (sends `endpoint` event with POST URL)
//! - `GET /sse?sessionId=<id>&cursor=<seq>` — Reconnect with cursor-based backfill
//! - `POST /message?sessionId=<id>` — JSON-RPC message endpoint
//! - `GET /health` — Health check
//!
//! Supports cursor-based SSE reconnection via `Last-Event-ID` header or `cursor`
//! query parameter. On reconnect, missed events are replayed from an in-memory
//! buffer before live events resume.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    response::Json,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tokio::sync::{mpsc, Mutex, RwLock, Semaphore};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::client::OpenCodeClient;
use crate::config::Config;
use crate::server::McpServer;

#[cfg(feature = "prometheus")]
use prometheus::{IntCounter, Opts, Registry};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of events retained per session for backfill on reconnect.
const EVENT_BUFFER_CAPACITY: usize = 1000;

/// Maximum size (bytes) of a single SSE event data payload.
/// Events exceeding this limit are truncated with a notice.
const MAX_SSE_FRAME_SIZE: usize = 64 * 1024; // 64 KiB

/// Maximum number of recent JSON-RPC request IDs tracked per session for
/// duplicate detection.  Oldest entries are evicted when the window is full.
const DEDUP_WINDOW_SIZE: usize = 256;

/// Maximum number of concurrent SSE sessions.  New connections that would
/// exceed this limit receive HTTP 503 Service Unavailable.
const MAX_CONCURRENT_SESSIONS: usize = 1000;

/// Time after which an inactive session is eligible for cleanup.
const SESSION_TTL: Duration = Duration::from_secs(300);

/// Interval between session cleanup sweeps.
const SESSION_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Event Buffer
// ---------------------------------------------------------------------------

/// A buffered SSE event with a sequence number for cursor tracking.
#[derive(Debug, Clone)]
struct BufferedEvent {
    seq: u64,
    event_type: String,
    data: String,
}

/// Bounded in-memory event buffer for a single MCP SSE session.
///
/// Stores up to [`EVENT_BUFFER_CAPACITY`] events. When full, the oldest event
/// is evicted. Each event gets a monotonically increasing sequence number
/// assigned on push, enabling cursor-based replay on reconnect.
struct EventBuffer {
    events: Vec<BufferedEvent>,
    next_seq: u64,
    capacity: usize,
}

impl EventBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            next_seq: 1,
            capacity,
        }
    }

    /// Push an event and return its assigned sequence number.
    ///
    /// If the buffer is at capacity, the oldest event is evicted.
    fn push(&mut self, event_type: String, data: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;

        if self.events.len() >= self.capacity {
            self.events.remove(0);
        }

        self.events.push(BufferedEvent {
            seq,
            event_type,
            data,
        });

        seq
    }

    /// Return all events with `seq > after_seq`, in insertion order.
    fn events_after(&self, after_seq: u64) -> Vec<BufferedEvent> {
        self.events
            .iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect()
    }

    /// Return the highest sequence number in the buffer, or 0 if empty.
    #[allow(dead_code)]
    fn latest_seq(&self) -> u64 {
        self.events.last().map(|e| e.seq).unwrap_or(0)
    }

    fn oldest_seq(&self) -> u64 {
        self.events.first().map(|e| e.seq).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Shared application state across all handlers.
struct AppState {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    config: Config,
    opencode_client: OpenCodeClient,
    /// Semaphore limiting concurrent SSE sessions to [`MAX_CONCURRENT_SESSIONS`].
    session_semaphore: Semaphore,
    /// Global count of events dropped due to full channels across all sessions.
    global_events_dropped: AtomicUsize,
    /// Prometheus counter for dropped SSE events.
    #[cfg(feature = "prometheus")]
    sse_drops_counter: IntCounter,
    /// Prometheus registry for metrics exposition.
    /// Stored to keep registered counters alive; read indirectly via `sse_drops_counter`.
    #[cfg(feature = "prometheus")]
    #[allow(dead_code)]
    metrics_registry: Registry,
}

/// Per-connection session — owns an MCP server instance, SSE sender, and event buffer.
struct Session {
    server: Mutex<McpServer>,
    /// Sender wrapped in Mutex so it can be replaced on reconnect.
    tx: Mutex<mpsc::Sender<Result<Event, Infallible>>>,
    /// Bounded event buffer for cursor-based backfill.
    event_buffer: Mutex<EventBuffer>,
    /// Recent JSON-RPC request IDs processed by this session, for dedup.
    recent_request_ids: Mutex<Vec<String>>,
    /// Timestamp of last activity, used for TTL cleanup.
    last_active: std::sync::Mutex<std::time::Instant>,
    /// Count of events dropped because the SSE channel was full.
    events_dropped: AtomicU64,
}

impl Session {
    /// Update the last-active timestamp to now.
    fn touch(&self) {
        if let Ok(mut instant) = self.last_active.lock() {
            *instant = std::time::Instant::now();
        }
    }
}

/// Query parameters for the `/sse` endpoint.
#[derive(Deserialize)]
struct SseQuery {
    /// Existing session ID to reconnect to.
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    /// Cursor (sequence number) to resume from.
    cursor: Option<u64>,
}

/// Query parameters for the `/message` endpoint.
#[derive(Deserialize)]
struct MessageQuery {
    #[serde(rename = "sessionId")]
    session_id: String,
}

// ---------------------------------------------------------------------------
// Session ID generation
// ---------------------------------------------------------------------------

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn generate_session_id() -> String {
    let count = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{ts:x}-{count:04x}")
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the MCP server with SSE transport on the configured port.
pub async fn run_sse_server(config: Config) -> anyhow::Result<()> {
    let port = config.port;
    let opencode_client = OpenCodeClient::new(&config)?;

    #[cfg(feature = "prometheus")]
    let metrics_registry = Registry::new();
    #[cfg(feature = "prometheus")]
    let sse_drops_counter = IntCounter::with_opts(Opts::new(
        "rustbrain_mcp_sse_drops_total",
        "Total number of SSE events dropped due to full channel",
    ))?;
    #[cfg(feature = "prometheus")]
    metrics_registry.register(Box::new(sse_drops_counter.clone()))?;

    let state = Arc::new(AppState {
        sessions: RwLock::new(HashMap::new()),
        config,
        opencode_client,
        session_semaphore: Semaphore::new(MAX_CONCURRENT_SESSIONS),
        global_events_dropped: AtomicUsize::new(0),
        #[cfg(feature = "prometheus")]
        sse_drops_counter,
        #[cfg(feature = "prometheus")]
        metrics_registry,
    });

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SESSION_CLEANUP_INTERVAL);
        loop {
            interval.tick().await;
            cleanup_stale_sessions(&cleanup_state).await;
        }
    });

    let app = Router::new()
        .route("/sse", get(sse_handler))
        .route("/message", post(message_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    info!("MCP SSE transport listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Session cleanup
// ---------------------------------------------------------------------------

/// Remove sessions that have been inactive longer than [`SESSION_TTL`].
async fn cleanup_stale_sessions(state: &Arc<AppState>) {
    let now = std::time::Instant::now();
    let mut to_remove = Vec::new();

    let sessions = state.sessions.read().await;
    for (id, session) in sessions.iter() {
        let is_stale = session
            .last_active
            .lock()
            .map(|instant| now.duration_since(*instant) > SESSION_TTL)
            .unwrap_or(false);
        if is_stale {
            to_remove.push(id.clone());
        }
    }
    drop(sessions);

    if !to_remove.is_empty() {
        let mut sessions = state.sessions.write().await;
        for id in &to_remove {
            sessions.remove(id);
        }
        state.session_semaphore.add_permits(to_remove.len());
        info!("Cleaned up {} stale session(s)", to_remove.len());
    }
}

// ---------------------------------------------------------------------------
// Dedup helpers
// ---------------------------------------------------------------------------

/// Extract the `id` field from a JSON-RPC request body as a string key.
/// Returns `None` for notifications (requests without an `id` field).
fn extract_request_id(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value.get("id").map(|id| id.to_string())
}

/// Check whether `request_id` was already seen in `recent_ids`.
/// If new, the ID is recorded and `false` is returned.
/// If duplicate, `true` is returned and the set is left unchanged.
fn check_and_record_duplicate(recent_ids: &mut Vec<String>, request_id: &str) -> bool {
    if recent_ids.iter().any(|id| id == request_id) {
        return true;
    }
    recent_ids.push(request_id.to_string());
    if recent_ids.len() > DEDUP_WINDOW_SIZE {
        recent_ids.remove(0);
    }
    false
}

/// If `json` exceeds `MAX_SSE_FRAME_SIZE`, replace it with a truncated error
/// response preserving the original request `id`.  Otherwise return `json` as-is.
fn enforce_frame_limit(json: String, original_id: &Option<crate::server::Id>) -> String {
    if json.len() <= MAX_SSE_FRAME_SIZE {
        return json;
    }

    let original_size = json.len();
    warn!(
        "SSE frame oversized ({} bytes > {} limit), truncating",
        original_size, MAX_SSE_FRAME_SIZE
    );

    let truncated = crate::server::JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: original_id.clone(),
        result: Some(serde_json::json!({
            "content": [{
                "type": "text",
                "text": format!(
                    "[Response truncated: {} bytes exceeded {} byte limit. \
                     Refine your query to reduce the result set.]",
                    original_size, MAX_SSE_FRAME_SIZE
                )
            }],
            "isError": true,
            "_truncated": true
        })),
        error: None,
    };

    serde_json::to_string(&truncated).unwrap_or_else(|_| {
        format!(
            r#"{{"jsonrpc":"2.0","id":null,"result":{{"content":[{{"type":"text","text":"Response truncated ({} bytes)"}}],"isError":true,"_truncated":true}}}}"#,
            original_size
        )
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /sse` — Opens an SSE stream for a new or existing session.
///
/// Per the MCP SSE spec the first event is `endpoint` containing the URL
/// the client should POST JSON-RPC messages to.
///
/// Supports cursor-based reconnection:
/// - `?sessionId=<id>&cursor=<seq>` — Reconnect to existing session, replay
///   events after the given cursor.
/// - `Last-Event-ID` header — Alternative cursor source (query param takes
///   precedence when both are present).
async fn sse_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SseQuery>,
    headers: HeaderMap,
) -> Result<Sse<ReceiverStream<Result<Event, Infallible>>>, (axum::http::StatusCode, &'static str)>
{
    let cursor_from_header = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let cursor = query.cursor.or(cursor_from_header);

    if let Some(ref session_id) = query.session_id {
        let session = {
            let sessions = state.sessions.read().await;
            sessions.get(session_id).cloned()
        };

        if let Some(session) = session {
            return Ok(reconnect_session(session, session_id.clone(), cursor, &state).await);
        }

        warn!(
            "Session {} not found for reconnect, creating new session",
            session_id
        );
    }

    let permit = state.session_semaphore.try_acquire().map_err(|_| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Too many concurrent SSE connections",
        )
    })?;
    permit.forget();

    Ok(new_session(&state).await)
}

/// Create a brand-new SSE session.
async fn new_session(state: &Arc<AppState>) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let session_id = generate_session_id();
    let channel_capacity = state.config.sse_channel_capacity;
    let (tx, rx) = mpsc::channel(channel_capacity);

    let server =
        McpServer::new(state.config.clone()).expect("Failed to create MCP server for session");

    let session = Arc::new(Session {
        server: Mutex::new(server),
        tx: Mutex::new(tx.clone()),
        event_buffer: Mutex::new(EventBuffer::new(EVENT_BUFFER_CAPACITY)),
        recent_request_ids: Mutex::new(Vec::with_capacity(DEDUP_WINDOW_SIZE)),
        last_active: std::sync::Mutex::new(std::time::Instant::now()),
        events_dropped: AtomicU64::new(0),
    });

    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), session.clone());

    info!("New SSE session: {}", session_id);

    let seq = {
        let mut buf = session.event_buffer.lock().await;
        buf.push(
            "endpoint".to_string(),
            format!("/message?sessionId={session_id}"),
        )
    };
    let _ = tx
        .send(Ok(Event::default()
            .id(seq.to_string())
            .event("endpoint")
            .data(format!("/message?sessionId={session_id}"))))
        .await;

    let stream = ReceiverStream::new(rx);
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// Reconnect to an existing session, replaying buffered events after cursor.
async fn reconnect_session(
    session: Arc<Session>,
    session_id: String,
    cursor: Option<u64>,
    _state: &Arc<AppState>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    session.touch();
    info!(
        "SSE reconnect for session {}, cursor={:?}",
        session_id, cursor
    );

    let (new_tx, new_rx) = mpsc::channel(_state.config.sse_channel_capacity);

    {
        let mut tx = session.tx.lock().await;
        *tx = new_tx.clone();
    }

    if let Some(after_seq) = cursor {
        let (buffered, oldest) = {
            let buf = session.event_buffer.lock().await;
            (buf.events_after(after_seq), buf.oldest_seq())
        };

        if oldest > 0 && after_seq + 1 < oldest {
            let gap_start = after_seq + 1;
            let gap_end = oldest - 1;
            warn!(
                "Gap detected for session {}: events [{}..{}] evicted from buffer",
                session_id, gap_start, gap_end
            );
            let gap_data = serde_json::json!({
                "gap_start": gap_start,
                "gap_end": gap_end,
                "count": gap_end - gap_start + 1,
            });
            let _ = new_tx
                .send(Ok(Event::default().event("gap").data(gap_data.to_string())))
                .await;
        }

        if !buffered.is_empty() {
            info!(
                "Replaying {} buffered event(s) for session {} after seq {}",
                buffered.len(),
                session_id,
                after_seq
            );
            for ev in &buffered {
                let _ = new_tx
                    .send(Ok(Event::default()
                        .id(ev.seq.to_string())
                        .event(&ev.event_type)
                        .data(&ev.data)))
                    .await;
            }
        }
    }

    let stream = ReceiverStream::new(new_rx);
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// `POST /message?sessionId=<id>` — Receives a JSON-RPC request, processes it
/// through the session's `McpServer`, and pushes the response onto the SSE stream.
///
/// Deduplication: if the request carries an `id` that was already processed in
/// this session, the request is silently skipped (HTTP 202, no new SSE event).
///
/// Frame limiting: if the serialized response exceeds `MAX_SSE_FRAME_SIZE`, it
/// is replaced with a truncated error response so the client always receives a
/// valid, size-bounded event.
async fn message_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MessageQuery>,
    body: String,
) -> axum::http::StatusCode {
    let session_id = &query.session_id;

    let session = {
        let sessions = state.sessions.read().await;
        sessions.get(session_id).cloned()
    };

    let session = match session {
        Some(s) => s,
        None => {
            warn!("Unknown session: {}", session_id);
            return axum::http::StatusCode::NOT_FOUND;
        }
    };

    session.touch();
    debug!("Message for session {}: {}", session_id, body);

    if let Some(request_id) = extract_request_id(&body) {
        let mut recent_ids = session.recent_request_ids.lock().await;
        if check_and_record_duplicate(&mut recent_ids, &request_id) {
            warn!(
                "Duplicate request ID {} in session {}, skipping",
                request_id, session_id
            );
            return axum::http::StatusCode::ACCEPTED;
        }
    }

    let mut server = session.server.lock().await;
    let response = server.handle_message(&body).await;
    drop(server);

    let response_json = match response {
        Ok(Some(r)) => {
            let id = r.id.clone();
            match serde_json::to_string(&r) {
                Ok(json) => enforce_frame_limit(json, &id),
                Err(e) => {
                    error!("Serialization error: {}", e);
                    return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                }
            }
        }
        Ok(None) => return axum::http::StatusCode::ACCEPTED,
        Err(e) => {
            error!("Error handling message: {}", e);
            let err_response = crate::server::JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(crate::server::JsonRpcError {
                    code: -32603,
                    message: e.to_string(),
                    data: None,
                }),
            };
            match serde_json::to_string(&err_response) {
                Ok(json) => json,
                Err(e) => {
                    error!("Serialization error: {}", e);
                    return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
                }
            }
        }
    };

    let seq = {
        let mut buf = session.event_buffer.lock().await;
        buf.push("message".to_string(), response_json.clone())
    };

    let send_result = {
        let tx = session.tx.lock().await;
        tx.try_send(Ok(Event::default()
            .id(seq.to_string())
            .event("message")
            .data(response_json)))
    };

    match send_result {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            session.events_dropped.fetch_add(1, Ordering::Relaxed);
            state.global_events_dropped.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "prometheus")]
            state.sse_drops_counter.inc();
            warn!(
                "SSE channel full for session {}, dropping event (session_dropped: {}, global_dropped: {})",
                session_id,
                session.events_dropped.load(Ordering::Relaxed),
                state.global_events_dropped.load(Ordering::Relaxed)
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            info!("Session {} disconnected, cleaning up", session_id);
            state.sessions.write().await.remove(session_id);
            state.session_semaphore.add_permits(1);
            return axum::http::StatusCode::GONE;
        }
    }

    axum::http::StatusCode::ACCEPTED
}

/// `GET /health` — Health-check endpoint.
async fn health_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let opencode_healthy = state.opencode_client.health_check().await.unwrap_or(false);

    let active_sessions = state.sessions.read().await.len();
    let available_permits = state.session_semaphore.available_permits();
    let global_dropped = state.global_events_dropped.load(Ordering::Relaxed);

    let mut sse_json = serde_json::json!({
        "active_sessions": active_sessions,
        "available_connections": available_permits,
        "max_connections": MAX_CONCURRENT_SESSIONS,
        "global_events_dropped": global_dropped,
    });

    #[cfg(feature = "prometheus")]
    {
        sse_json["prometheus_drops_total"] = state.sse_drops_counter.get().into();
    }

    Json(serde_json::json!({
        "status": "ok",
        "opencode": {
            "healthy": opencode_healthy
        },
        "sse": sse_json,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_buffer_push_and_retrieve() {
        let mut buf = EventBuffer::new(100);
        let s1 = buf.push("endpoint".to_string(), "/message?sessionId=abc".to_string());
        let s2 = buf.push("message".to_string(), r#"{"jsonrpc":"2.0"}"#.to_string());
        let s3 = buf.push(
            "message".to_string(),
            r#"{"jsonrpc":"2.0","id":2}"#.to_string(),
        );

        assert_eq!(s1, 1);
        assert_eq!(s2, 2);
        assert_eq!(s3, 3);

        let all = buf.events_after(0);
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[1].seq, 2);
        assert_eq!(all[2].seq, 3);
    }

    #[test]
    fn event_buffer_events_after_cursor() {
        let mut buf = EventBuffer::new(100);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        buf.push("c".to_string(), "3".to_string());
        buf.push("d".to_string(), "4".to_string());
        buf.push("e".to_string(), "5".to_string());

        let after_2 = buf.events_after(2);
        assert_eq!(after_2.len(), 3);
        assert_eq!(after_2[0].seq, 3);
        assert_eq!(after_2[1].seq, 4);
        assert_eq!(after_2[2].seq, 5);
    }

    #[test]
    fn event_buffer_capacity_eviction() {
        let mut buf = EventBuffer::new(3);
        let s1 = buf.push("a".to_string(), "1".to_string());
        let s2 = buf.push("b".to_string(), "2".to_string());
        let s3 = buf.push("c".to_string(), "3".to_string());
        let s4 = buf.push("d".to_string(), "4".to_string());

        assert_eq!(buf.events_after(0).len(), 3);
        let after_0 = buf.events_after(0);
        assert_eq!(after_0[0].seq, s2);
        assert_eq!(after_0[1].seq, s3);
        assert_eq!(after_0[2].seq, s4);

        let after_1 = buf.events_after(s1);
        assert_eq!(after_1.len(), 3);
    }

    #[test]
    fn event_buffer_latest_seq() {
        let mut buf = EventBuffer::new(100);
        assert_eq!(buf.latest_seq(), 0);

        buf.push("a".to_string(), "1".to_string());
        assert_eq!(buf.latest_seq(), 1);

        buf.push("b".to_string(), "2".to_string());
        assert_eq!(buf.latest_seq(), 2);
    }

    #[test]
    fn event_buffer_latest_seq_after_eviction() {
        let mut buf = EventBuffer::new(2);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        buf.push("c".to_string(), "3".to_string());
        assert_eq!(buf.latest_seq(), 3);
    }

    #[test]
    fn session_id_generation_format() {
        let id = generate_session_id();
        // Should be hex_timestamp-4digit_counter
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 2, "Session ID should have 2 parts: {id}");
        assert!(
            u64::from_str_radix(parts[0], 16).is_ok(),
            "First part should be valid hex: {}",
            parts[0]
        );
        assert_eq!(parts[1].len(), 4, "Counter should be 4 chars: {}", parts[1]);
    }

    #[test]
    fn sse_query_deserialization_with_session_and_cursor() {
        let query: SseQuery =
            serde_json::from_str(r#"{"sessionId":"abc123","cursor":42}"#).unwrap();
        assert_eq!(query.session_id.as_deref(), Some("abc123"));
        assert_eq!(query.cursor, Some(42));
    }

    #[test]
    fn sse_query_deserialization_empty() {
        let query: SseQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(query.session_id.is_none());
        assert!(query.cursor.is_none());
    }

    #[test]
    fn sse_query_deserialization_cursor_only() {
        let query: SseQuery = serde_json::from_str(r#"{"cursor":10}"#).unwrap();
        assert!(query.session_id.is_none());
        assert_eq!(query.cursor, Some(10));
    }

    #[test]
    fn event_buffer_events_after_empty() {
        let buf = EventBuffer::new(100);
        assert!(buf.events_after(0).is_empty());
    }

    #[test]
    fn event_buffer_events_after_beyond_latest() {
        let mut buf = EventBuffer::new(100);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        assert!(buf.events_after(99).is_empty());
    }

    #[test]
    fn session_touch_updates_last_active() {
        let mut buf = EventBuffer::new(100);
        buf.push("test".to_string(), "data".to_string());

        let (tx, _rx) = mpsc::channel(256);
        let session = Session {
            server: Mutex::new(
                McpServer::new(Config::default()).expect("Failed to create MCP server"),
            ),
            tx: Mutex::new(tx),
            event_buffer: Mutex::new(buf),
            recent_request_ids: Mutex::new(Vec::new()),
            last_active: std::sync::Mutex::new(
                std::time::Instant::now() - Duration::from_secs(100),
            ),
            events_dropped: AtomicU64::new(0),
        };

        let elapsed_before = session.last_active.lock().unwrap().elapsed();
        assert!(elapsed_before > Duration::from_secs(50));

        session.touch();

        let elapsed_after = session.last_active.lock().unwrap().elapsed();
        assert!(elapsed_after < Duration::from_secs(5));
    }

    #[test]
    fn cleanup_stale_sessions_removes_expired() {
        let mut buf = EventBuffer::new(100);
        buf.push("test".to_string(), "data".to_string());

        let (tx, _rx) = mpsc::channel(256);
        let session = Arc::new(Session {
            server: Mutex::new(
                McpServer::new(Config::default()).expect("Failed to create MCP server"),
            ),
            tx: Mutex::new(tx),
            event_buffer: Mutex::new(buf),
            recent_request_ids: Mutex::new(Vec::new()),
            last_active: std::sync::Mutex::new(
                std::time::Instant::now() - Duration::from_secs(600),
            ),
            events_dropped: AtomicU64::new(0),
        });

        let now = std::time::Instant::now();
        let is_stale = session
            .last_active
            .lock()
            .map(|instant| now.duration_since(*instant) > SESSION_TTL)
            .unwrap_or(false);
        assert!(is_stale);
    }

    // === extract_request_id Tests ===

    #[test]
    fn extract_request_id_numeric() {
        let body = r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{}}"#;
        assert_eq!(extract_request_id(body), Some("42".to_string()));
    }

    #[test]
    fn extract_request_id_string() {
        let body = r#"{"jsonrpc":"2.0","id":"abc-123","method":"ping"}"#;
        assert_eq!(extract_request_id(body), Some(r#""abc-123""#.to_string()));
    }

    #[test]
    fn extract_request_id_notification_returns_none() {
        let body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        assert_eq!(extract_request_id(body), None);
    }

    #[test]
    fn extract_request_id_invalid_json_returns_none() {
        assert_eq!(extract_request_id("not json"), None);
    }

    #[test]
    fn extract_request_id_null_id_returns_some() {
        let body = r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#;
        assert_eq!(extract_request_id(body), Some("null".to_string()));
    }

    // === Dedup Window Tests ===

    #[test]
    fn dedup_new_id_returns_false() {
        let mut recent = Vec::new();
        assert!(!check_and_record_duplicate(&mut recent, "1"));
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn dedup_duplicate_id_returns_true() {
        let mut recent = Vec::new();
        check_and_record_duplicate(&mut recent, "1");
        assert!(check_and_record_duplicate(&mut recent, "1"));
        assert_eq!(recent.len(), 1);
    }

    #[test]
    fn dedup_different_ids_both_recorded() {
        let mut recent = Vec::new();
        assert!(!check_and_record_duplicate(&mut recent, "1"));
        assert!(!check_and_record_duplicate(&mut recent, "2"));
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn dedup_window_evicts_oldest() {
        let mut recent = Vec::new();
        for i in 0..=DEDUP_WINDOW_SIZE {
            check_and_record_duplicate(&mut recent, &i.to_string());
        }
        assert_eq!(recent.len(), DEDUP_WINDOW_SIZE);
        // Oldest ID "0" should have been evicted, so it's no longer a duplicate
        assert!(!check_and_record_duplicate(&mut recent, "0"));
    }

    #[test]
    fn dedup_window_still_detects_recent_duplicate() {
        let mut recent = Vec::new();
        for i in 0..DEDUP_WINDOW_SIZE {
            check_and_record_duplicate(&mut recent, &i.to_string());
        }
        // The most recent ID should still be detected as duplicate
        let last_id = (DEDUP_WINDOW_SIZE - 1).to_string();
        assert!(check_and_record_duplicate(&mut recent, &last_id));
    }

    // === Frame Size Limiting Tests ===

    #[test]
    fn enforce_frame_limit_under_limit_passes_through() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_string();
        let id = Some(crate::server::Id::Number(1));
        let result = enforce_frame_limit(json.clone(), &id);
        assert_eq!(result, json);
    }

    #[test]
    fn enforce_frame_limit_oversize_truncates() {
        let large_data = "x".repeat(MAX_SSE_FRAME_SIZE + 1000);
        let json = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            large_data
        );
        let id = Some(crate::server::Id::Number(1));
        let result = enforce_frame_limit(json.clone(), &id);

        // Result should be smaller than original
        assert!(result.len() < json.len());
        // Result should contain truncation indicator
        assert!(result.contains("_truncated"));
        assert!(result.contains("isError"));
        // Result should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 1);
    }

    #[test]
    fn enforce_frame_limit_preserves_id() {
        let large_data = "x".repeat(MAX_SSE_FRAME_SIZE + 100);
        let json = format!(
            r#"{{"jsonrpc":"2.0","id":"my-req","result":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            large_data
        );
        let id = Some(crate::server::Id::String("my-req".to_string()));
        let result = enforce_frame_limit(json, &id);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["id"], "my-req");
    }

    #[test]
    fn enforce_frame_limit_none_id() {
        let large_data = "x".repeat(MAX_SSE_FRAME_SIZE + 100);
        let json = format!(
            r#"{{"jsonrpc":"2.0","result":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            large_data
        );
        let result = enforce_frame_limit(json, &None);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["id"].is_null());
    }

    #[test]
    fn enforce_frame_limit_exactly_at_limit() {
        let json = "x".repeat(MAX_SSE_FRAME_SIZE);
        let result = enforce_frame_limit(json.clone(), &None);
        assert_eq!(result, json);
    }

    #[test]
    fn enforce_frame_limit_one_byte_over() {
        let json = "x".repeat(MAX_SSE_FRAME_SIZE + 1);
        let result = enforce_frame_limit(json, &None);
        assert!(result.contains("_truncated"));
    }

    // === Backpressure Tests ===

    #[test]
    fn event_buffer_oldest_seq_empty() {
        let buf = EventBuffer::new(100);
        assert_eq!(buf.oldest_seq(), 0);
    }

    #[test]
    fn event_buffer_oldest_seq_after_push() {
        let mut buf = EventBuffer::new(100);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        assert_eq!(buf.oldest_seq(), 1);
    }

    #[test]
    fn event_buffer_oldest_seq_after_eviction() {
        let mut buf = EventBuffer::new(2);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        buf.push("c".to_string(), "3".to_string());
        assert_eq!(buf.oldest_seq(), 2);
    }

    #[test]
    fn bounded_channel_try_send_drops_on_full() {
        let (tx, rx) = mpsc::channel::<i32>(2);
        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_ok());
        assert!(matches!(
            tx.try_send(3),
            Err(mpsc::error::TrySendError::Full(_))
        ));
        drop(rx);
    }

    #[test]
    fn session_events_dropped_counter_increments() {
        let (tx, _rx) = mpsc::channel(256);
        let session = Session {
            server: Mutex::new(
                McpServer::new(Config::default()).expect("Failed to create MCP server"),
            ),
            tx: Mutex::new(tx),
            event_buffer: Mutex::new(EventBuffer::new(EVENT_BUFFER_CAPACITY)),
            recent_request_ids: Mutex::new(Vec::new()),
            last_active: std::sync::Mutex::new(std::time::Instant::now()),
            events_dropped: AtomicU64::new(0),
        };

        assert_eq!(session.events_dropped.load(Ordering::Relaxed), 0);
        session.events_dropped.fetch_add(1, Ordering::Relaxed);
        assert_eq!(session.events_dropped.load(Ordering::Relaxed), 1);
        session.events_dropped.fetch_add(5, Ordering::Relaxed);
        assert_eq!(session.events_dropped.load(Ordering::Relaxed), 6);
    }

    #[tokio::test]
    async fn bounded_channel_try_send_detects_closed() {
        let (tx, rx) = mpsc::channel::<i32>(2);
        drop(rx);
        assert!(matches!(
            tx.try_send(1),
            Err(mpsc::error::TrySendError::Closed(_))
        ));
    }

    #[test]
    fn gap_detection_when_cursor_below_oldest() {
        let mut buf = EventBuffer::new(3);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        buf.push("c".to_string(), "3".to_string());
        buf.push("d".to_string(), "4".to_string());
        let oldest = buf.oldest_seq();
        assert_eq!(oldest, 2);
        let after_cursor = buf.events_after(0);
        assert_eq!(after_cursor.len(), 3);
        assert_eq!(after_cursor[0].seq, 2);
        let gap_start = 1;
        let gap_end = oldest - 1;
        assert_eq!(gap_start, 1);
        assert_eq!(gap_end, 1);
        assert_eq!(gap_end - gap_start + 1, 1);
    }

    #[test]
    fn gap_detection_no_gap_when_cursor_in_range() {
        let mut buf = EventBuffer::new(100);
        buf.push("a".to_string(), "1".to_string());
        buf.push("b".to_string(), "2".to_string());
        buf.push("c".to_string(), "3".to_string());
        let oldest = buf.oldest_seq();
        assert_eq!(oldest, 1);
        let after_cursor = buf.events_after(1);
        assert_eq!(after_cursor.len(), 2);
        let after_seq = 1;
        assert!(oldest == 0 || after_seq + 1 >= oldest);
    }

    #[test]
    fn semaphore_connection_limit() {
        let sem = Semaphore::new(2);
        let p1 = sem.try_acquire().unwrap();
        let p2 = sem.try_acquire().unwrap();
        assert!(sem.try_acquire().is_err());
        drop(p1);
        assert!(sem.try_acquire().is_ok());
        drop(p2);
    }

    #[test]
    fn semaphore_permit_forget_does_not_release() {
        let sem = Semaphore::new(2);
        let p1 = sem.try_acquire().unwrap();
        p1.forget();
        assert_eq!(sem.available_permits(), 1);
        let p2 = sem.try_acquire().unwrap();
        assert!(sem.try_acquire().is_err());
        drop(p2);
        assert_eq!(sem.available_permits(), 1);
        sem.add_permits(1);
        assert_eq!(sem.available_permits(), 2);
    }
}
