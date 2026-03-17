//! SSE transport for the MCP server
//!
//! Implements the MCP Server-Sent Events transport protocol:
//! - `GET /sse` — SSE connection endpoint (sends `endpoint` event with POST URL)
//! - `POST /message?sessionId=<id>` — JSON-RPC message endpoint
//! - `GET /health` — Health check

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Query, State},
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
// Types
// ---------------------------------------------------------------------------

/// Shared application state across all handlers.
struct AppState {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    config: Config,
    opencode_client: OpenCodeClient,
}

/// Per-connection session — owns an MCP server instance and an SSE sender.
struct Session {
    server: Mutex<McpServer>,
    tx: mpsc::UnboundedSender<Result<Event, Infallible>>,
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
// Handlers
// ---------------------------------------------------------------------------

/// `GET /sse` — Opens an SSE stream for a new session.
///
/// Per the MCP SSE spec the first event is `endpoint` containing the URL
/// the client should POST JSON-RPC messages to.
async fn sse_handler(
    State(state): State<Arc<AppState>>,
) -> Sse<UnboundedReceiverStream<Result<Event, Infallible>>> {
    let session_id = generate_session_id();
    let (tx, rx) = mpsc::unbounded_channel();

    let server =
        McpServer::new(state.config.clone()).expect("Failed to create MCP server for session");

    let session = Arc::new(Session {
        server: Mutex::new(server),
        tx: tx.clone(),
    });

    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), session);

    info!("New SSE session: {}", session_id);

    // Send the endpoint event so the client knows where to POST.
    let endpoint_url = format!("/message?sessionId={session_id}");
    let _ = tx.send(Ok(Event::default().event("endpoint").data(endpoint_url)));

    let stream = UnboundedReceiverStream::new(rx);
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

    debug!("Message for session {}: {}", session_id, body);

    // Process the JSON-RPC request.
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
        // Notification — no response required.
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

    // Push the response onto the SSE stream.
    if session
        .tx
        .send(Ok(Event::default().event("message").data(response_json)))
        .is_err()
    {
        // Client disconnected — clean up the session.
        info!("Session {} disconnected, cleaning up", session_id);
        state.sessions.write().await.remove(session_id);
        return axum::http::StatusCode::GONE;
    }

    axum::http::StatusCode::ACCEPTED
}

/// `GET /health` — Health-check endpoint.
async fn health_handler(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let opencode_healthy = state.opencode_client.health_check().await.unwrap_or(false);

    Json(serde_json::json!({
        "status": "ok",
        "opencode": {
            "healthy": opencode_healthy
        }
    }))
}
