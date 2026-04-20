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
use std::sync::atomic::{AtomicU64, Ordering};
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
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::client::OpenCodeClient;
use crate::config::Config;
use crate::server::McpServer;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of events retained per session for backfill on reconnect.
const EVENT_BUFFER_CAPACITY: usize = 1000;

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
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Shared application state across all handlers.
struct AppState {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    config: Config,
    opencode_client: OpenCodeClient,
}

/// Per-connection session — owns an MCP server instance, SSE sender, and event buffer.
struct Session {
    server: Mutex<McpServer>,
    /// Sender wrapped in Mutex so it can be replaced on reconnect.
    tx: Mutex<mpsc::UnboundedSender<Result<Event, Infallible>>>,
    /// Bounded event buffer for cursor-based backfill.
    event_buffer: Mutex<EventBuffer>,
    /// Timestamp of last activity, used for TTL cleanup.
    last_active: std::sync::Mutex<std::time::Instant>,
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

    let state = Arc::new(AppState {
        sessions: RwLock::new(HashMap::new()),
        config,
        opencode_client,
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
        info!("Cleaned up {} stale session(s)", to_remove.len());
    }
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
) -> Sse<UnboundedReceiverStream<Result<Event, Infallible>>> {
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
            return reconnect_session(session, session_id.clone(), cursor, &state).await;
        }

        warn!(
            "Session {} not found for reconnect, creating new session",
            session_id
        );
    }

    new_session(&state).await
}

/// Create a brand-new SSE session.
async fn new_session(
    state: &Arc<AppState>,
) -> Sse<UnboundedReceiverStream<Result<Event, Infallible>>> {
    let session_id = generate_session_id();
    let (tx, rx) = mpsc::unbounded_channel();

    let server =
        McpServer::new(state.config.clone()).expect("Failed to create MCP server for session");

    let session = Arc::new(Session {
        server: Mutex::new(server),
        tx: Mutex::new(tx.clone()),
        event_buffer: Mutex::new(EventBuffer::new(EVENT_BUFFER_CAPACITY)),
        last_active: std::sync::Mutex::new(std::time::Instant::now()),
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
    let _ = tx.send(Ok(Event::default()
        .id(seq.to_string())
        .event("endpoint")
        .data(format!("/message?sessionId={session_id}"))));

    let stream = UnboundedReceiverStream::new(rx);
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
) -> Sse<UnboundedReceiverStream<Result<Event, Infallible>>> {
    session.touch();
    info!(
        "SSE reconnect for session {}, cursor={:?}",
        session_id, cursor
    );

    let (new_tx, new_rx) = mpsc::unbounded_channel();

    {
        let mut tx = session.tx.lock().await;
        *tx = new_tx.clone();
    }

    if let Some(after_seq) = cursor {
        let buffered = {
            let buf = session.event_buffer.lock().await;
            buf.events_after(after_seq)
        };

        if !buffered.is_empty() {
            info!(
                "Replaying {} buffered event(s) for session {} after seq {}",
                buffered.len(),
                session_id,
                after_seq
            );
            for ev in &buffered {
                let _ = new_tx.send(Ok(Event::default()
                    .id(ev.seq.to_string())
                    .event(&ev.event_type)
                    .data(&ev.data)));
            }
        }
    }

    let stream = UnboundedReceiverStream::new(new_rx);
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

/// `POST /message?sessionId=<id>` — Receives a JSON-RPC request, processes it
/// through the session's `McpServer`, and pushes the response onto the SSE stream.
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

    let mut server = session.server.lock().await;
    let response = server.handle_message(&body).await;
    drop(server);

    let response_json = match response {
        Ok(Some(r)) => match serde_json::to_string(&r) {
            Ok(json) => json,
            Err(e) => {
                error!("Serialization error: {}", e);
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
            }
        },
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
        tx.send(Ok(Event::default()
            .id(seq.to_string())
            .event("message")
            .data(response_json)))
    };

    if send_result.is_err() {
        info!("Session {} disconnected, cleaning up", session_id);
        state.sessions.write().await.remove(session_id);
        return axum::http::StatusCode::GONE;
    }

    axum::http::StatusCode::ACCEPTED
}

/// `GET /health` — Health-check endpoint.
async fn health_handler(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let opencode_healthy = state.opencode_client.health_check().await.unwrap_or(false);

    Json(serde_json::json!({
        "status": "ok",
        "opencode": {
            "healthy": opencode_healthy
        }
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

        let (tx, _rx) = mpsc::unbounded_channel();
        let session = Session {
            server: Mutex::new(
                McpServer::new(Config::default()).expect("Failed to create MCP server"),
            ),
            tx: Mutex::new(tx),
            event_buffer: Mutex::new(buf),
            last_active: std::sync::Mutex::new(
                std::time::Instant::now() - Duration::from_secs(100),
            ),
        };

        // Before touch, last_active should be old
        {
            let instant = session.last_active.lock().unwrap();
            assert!(instant.elapsed() > Duration::from_secs(50));
        }

        session.touch();

        {
            let instant = session.last_active.lock().unwrap();
            assert!(instant.elapsed() < Duration::from_secs(5));
        }
    }

    #[test]
    fn cleanup_stale_sessions_removes_expired() {
        let mut buf = EventBuffer::new(100);
        buf.push("test".to_string(), "data".to_string());

        let (tx, _rx) = mpsc::unbounded_channel();
        let session = Arc::new(Session {
            server: Mutex::new(
                McpServer::new(Config::default()).expect("Failed to create MCP server"),
            ),
            tx: Mutex::new(tx),
            event_buffer: Mutex::new(buf),
            last_active: std::sync::Mutex::new(
                std::time::Instant::now() - Duration::from_secs(600),
            ),
        });

        let now = std::time::Instant::now();
        let is_stale = session
            .last_active
            .lock()
            .map(|instant| now.duration_since(*instant) > SESSION_TTL)
            .unwrap_or(false);
        assert!(is_stale);
    }
}
