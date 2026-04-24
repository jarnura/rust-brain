//! Execution REST handlers.
//!
//! | Method | Path | Handler | Description |
//! |--------|------|---------|-------------|
//! | POST | `/workspaces/:id/execute` | [`execute_workspace`] | Start an execution |
//! | GET  | `/workspaces/:id/executions` | [`list_executions`] | List executions for a workspace |
//! | GET  | `/executions/:id` | [`get_execution`] | Fetch execution status |
//! | GET  | `/executions/:id/events` | [`stream_events`] | SSE stream of agent events |

use axum::{
    extract::{Extension, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;
use uuid::Uuid;

use crate::errors::AppError;
use crate::execution::runner::{run_execution, RunParams};
use crate::execution::{
    create_execution, get_agent_event_by_seq, get_execution as db_get_execution,
    list_agent_events_after_seq, list_executions as db_list_executions, CreateExecutionParams,
};
use crate::middleware::auth::{require_write_access, ApiKeyContext};
use crate::state::AppState;
use crate::workspace::get_workspace;

const SSE_MAX_FRAME_BYTES: usize = 64 * 1024;
const SSE_CHANNEL_CAPACITY: usize = 256;
const SSE_WRITE_TIMEOUT_SECS: u64 = 5;

fn truncate_sse_event(data: &str, event_seq: i64) -> String {
    if data.len() <= SSE_MAX_FRAME_BYTES {
        return data.to_string();
    }

    let original_size = data.len();
    let truncated = serde_json::json!({
        "truncated": true,
        "full_payload_seq": event_seq,
        "original_size": original_size,
        "message": format!("Event payload ({} bytes) exceeded {} byte SSE frame limit. Use GET /executions/{{id}}/events/{{seq}} to fetch the full payload.", original_size, SSE_MAX_FRAME_BYTES),
    });
    truncated.to_string()
}

// =============================================================================
// Request / Response types
// =============================================================================

/// Body for `POST /workspaces/:id/execute`.
#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    /// The natural-language prompt driving the multi-agent flow.
    pub prompt: String,
    /// Optional branch name to use for commits (defaults to a generated name).
    pub branch_name: Option<String>,
    /// Override the default timeout in seconds (default 7200 = 2 h).
    pub timeout_secs: Option<i32>,
}

/// Response for a newly created execution (`202 Accepted`).
#[derive(Debug, Serialize)]
pub struct ExecuteResponse {
    pub id: Uuid,
    pub status: String,
    pub message: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// `POST /workspaces/:id/execute` — start a multi-agent execution.
///
/// Returns `202 Accepted` immediately. Poll `GET /executions/:id` for status,
/// or stream `GET /executions/:id/events` for live event updates.
pub async fn execute_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<ExecuteRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_write_access(&ctx)?;
    if req.prompt.trim().is_empty() {
        return Err(AppError::BadRequest("prompt must not be empty".into()));
    }

    // Verify workspace exists and is ready
    let workspace = get_workspace(&state.workspace_manager.pool, workspace_id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace {workspace_id} not found")))?;

    let volume_name = workspace.volume_name.ok_or_else(|| {
        AppError::BadRequest(format!(
            "Workspace {workspace_id} has no volume — ensure it has been cloned first"
        ))
    })?;

    // Create the execution row
    let params = CreateExecutionParams {
        workspace_id,
        prompt: req.prompt.clone(),
        branch_name: req.branch_name.clone(),
        timeout_config_secs: req.timeout_secs,
    };
    let execution = create_execution(&state.workspace_manager.pool, params)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create execution: {e}")))?;

    let exec_id = execution.id;

    // Spawn the orchestrator flow as a background task
    let pool = state.workspace_manager.pool.clone();
    let docker = state.docker.clone();
    let run_params = RunParams {
        execution_id: exec_id,
        volume_name,
        prompt: req.prompt,
        docker_network: state.config.docker_network.clone(),
        opencode_image: state.config.opencode_image.clone(),
        opencode_user: state.config.opencode_auth_user.clone(),
        opencode_pass: state.config.opencode_auth_pass.clone(),
        timeout_secs: execution.timeout_config_secs as u32,
        public_host: state.config.public_host.clone(),
        keep_alive_secs: state.config.container_keep_alive_secs,
        ready_timeout_secs: state.config.opencode_ready_timeout_secs,
        opencode_config_host_path: state.config.opencode_config_host_path.clone(),
        mcp_sse_url: Some(state.config.mcp_sse_url.clone()),
        litellm_base_url: Some(state.config.litellm_base_url.clone()),
        litellm_api_key: std::env::var("LITELLM_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        openai_api_key: std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty()),
        opencode_server_password: std::env::var("OPENCODE_SERVER_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty()),
    };

    tokio::spawn(async move {
        run_execution(pool, docker, run_params).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(ExecuteResponse {
            id: exec_id,
            status: "running".into(),
            message: "Execution started. Stream events at GET /executions/{id}/events".into(),
        }),
    ))
}

/// `GET /workspaces/:id/executions` — list all executions for a workspace.
pub async fn list_executions(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<Vec<crate::execution::Execution>>, AppError> {
    db_list_executions(&state.workspace_manager.pool, workspace_id)
        .await
        .map(Json)
        .map_err(|e| AppError::Database(format!("Failed to list executions: {e}")))
}

/// `GET /executions/:id` — fetch a single execution by ID.
pub async fn get_execution(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::execution::Execution>, AppError> {
    db_get_execution(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch execution: {e}")))?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("Execution {id} not found")))
}

/// Query parameters accepted by [`stream_events`].
#[derive(Debug, Deserialize)]
pub struct StreamEventsQuery {
    /// Cursor for SSE reconnect backfill. Alternative to the `Last-Event-ID`
    /// header for clients (such as the browser `EventSource` API) that cannot
    /// set custom headers when opening a new connection.
    #[serde(default)]
    pub last_event_id: Option<i64>,
}

/// `GET /executions/:id/events` — SSE stream of agent events.
///
/// The stream polls Postgres every 500 ms for new events using seq-based cursors.
/// It terminates once the execution reaches a terminal state (`completed`,
/// `failed`, `aborted`, `timeout`) and all pending events have been delivered.
///
/// Supports two equivalent cursor-resume mechanisms:
///
///   * `Last-Event-ID` header — set automatically by the browser when an
///     `EventSource` auto-reconnects.
///   * `?last_event_id=<seq>` query string — used by our frontend reconnect
///     client (RUSA-257), which cannot set headers because `EventSource` has
///     no header-setting API.
///
/// When both are present, the query string wins (explicit caller intent).
pub async fn stream_events(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<StreamEventsQuery>,
    headers: HeaderMap,
) -> Result<Sse<ReceiverStream<Result<Event, std::convert::Infallible>>>, AppError> {
    // Verify execution exists before starting the stream
    db_get_execution(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch execution: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("Execution {id} not found")))?;

    let pool = state.workspace_manager.pool.clone();

    // Parse cursor for SSE reconnect backfill. Prefer the explicit query
    // string (see handler doc), falling back to the `Last-Event-ID` header.
    let initial_seq = query.last_event_id.unwrap_or_else(|| {
        headers
            .get("Last-Event-ID")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    });

    let (tx, rx) = mpsc::channel(SSE_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let mut last_seq: i64 = initial_seq;
        let poll_interval = Duration::from_millis(500);
        let write_timeout = Duration::from_secs(SSE_WRITE_TIMEOUT_SECS);

        loop {
            // Fetch new events since last seen seq
            let events = match list_agent_events_after_seq(&pool, id, last_seq).await {
                Ok(evts) => evts,
                Err(e) => {
                    let payload = serde_json::json!({
                        "error": e.to_string(),
                        "persistence_degraded": true,
                    });
                    let _ = tx
                        .send(Ok(Event::default()
                            .event("error")
                            .data(payload.to_string())))
                        .await;
                    break;
                }
            };

            for event in &events {
                let data = truncate_sse_event(
                    &serde_json::to_string(event).unwrap_or_default(),
                    event.seq,
                );
                let sse_event = Ok(Event::default()
                    .id(event.seq.to_string())
                    .event("agent_event")
                    .data(data));

                match tokio::time::timeout(write_timeout, tx.send(sse_event)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => {
                        // Receiver dropped — client disconnected
                        return;
                    }
                    Err(_) => {
                        // Write timeout — slow consumer
                        warn!(
                            execution_id = %id,
                            "SSE write timeout ({}s) for execution {}, closing stream",
                            SSE_WRITE_TIMEOUT_SECS, id
                        );
                        return;
                    }
                }
                last_seq = event.seq;
            }

            // Check execution terminal state
            match db_get_execution(&pool, id).await {
                Ok(Some(exec)) => {
                    let terminal = matches!(
                        exec.status.as_str(),
                        "completed" | "failed" | "aborted" | "timeout"
                    );
                    if terminal {
                        if let Ok(remaining) =
                            list_agent_events_after_seq(&pool, id, last_seq).await
                        {
                            for event in &remaining {
                                let data = truncate_sse_event(
                                    &serde_json::to_string(event).unwrap_or_default(),
                                    event.seq,
                                );
                                let sse_event = Ok(Event::default()
                                    .id(event.seq.to_string())
                                    .event("agent_event")
                                    .data(data));

                                match tokio::time::timeout(write_timeout, tx.send(sse_event)).await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(_)) | Err(_) => return,
                                }
                            }
                        }
                        // Emit a `done` event carrying the final status
                        let done_payload = serde_json::json!({ "status": exec.status }).to_string();
                        let _ = tx
                            .send(Ok(Event::default().event("done").data(done_payload)))
                            .await;
                        break;
                    }
                }
                Ok(None) => break, // Execution disappeared
                Err(_) => {}       // Transient DB error — keep streaming
            }

            tokio::time::sleep(poll_interval).await;
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()))
}

/// `GET /executions/:id/events/:seq` — fetch a single agent event by seq.
///
/// Returns the full untruncated event payload. This is the companion endpoint
/// to the SSE frame size limit: when an SSE event is truncated (content replaced
/// with `{truncated: true, full_payload_seq: <seq>}`), the client can fetch
/// the complete content via this endpoint.
pub async fn get_event_by_seq(
    State(state): State<AppState>,
    Path((id, seq)): Path<(Uuid, i64)>,
) -> Result<Json<crate::execution::AgentEvent>, AppError> {
    get_agent_event_by_seq(&state.workspace_manager.pool, id, seq)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch event: {e}")))?
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("Event {seq} not found for execution {id}")))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_request_deserializes_minimal() {
        let json = r#"{"prompt": "fix the race condition in src/lib.rs"}"#;
        let req: ExecuteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.prompt, "fix the race condition in src/lib.rs");
        assert!(req.branch_name.is_none());
        assert!(req.timeout_secs.is_none());
    }

    #[test]
    fn execute_request_deserializes_full() {
        let json = r#"{"prompt":"fix bug","branch_name":"fix/race","timeout_secs":3600}"#;
        let req: ExecuteRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.branch_name.as_deref(), Some("fix/race"));
        assert_eq!(req.timeout_secs, Some(3600));
    }

    #[test]
    fn execute_response_serializes() {
        let r = ExecuteResponse {
            id: Uuid::new_v4(),
            status: "running".into(),
            message: "Execution started.".into(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["status"], "running");
    }

    #[test]
    fn stream_events_query_deserializes_empty() {
        let q: StreamEventsQuery = serde_json::from_str("{}").unwrap();
        assert!(q.last_event_id.is_none());
    }

    #[test]
    fn stream_events_query_deserializes_cursor() {
        let q: StreamEventsQuery = serde_json::from_str(r#"{"last_event_id":42}"#).unwrap();
        assert_eq!(q.last_event_id, Some(42));
    }

    #[test]
    fn truncate_sse_event_under_limit_passes_through() {
        let data = r#"{"event_type":"reasoning","content":{"text":"hello"}}"#;
        let result = truncate_sse_event(data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn truncate_sse_event_over_limit_returns_truncated() {
        let large = "x".repeat(SSE_MAX_FRAME_BYTES + 1000);
        let data = format!(r#"{{"content":{{"text":"{}"}}}}"#, large);
        let result = truncate_sse_event(&data, 42);
        assert!(result.len() < data.len());
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["full_payload_seq"], 42);
        assert!(parsed["original_size"].as_u64().unwrap() > SSE_MAX_FRAME_BYTES as u64);
    }

    #[test]
    fn truncate_sse_event_exactly_at_limit_passes() {
        let data = "x".repeat(SSE_MAX_FRAME_BYTES);
        let result = truncate_sse_event(&data, 1);
        assert_eq!(result.len(), SSE_MAX_FRAME_BYTES);
    }

    #[test]
    fn truncate_sse_event_one_byte_over_truncates() {
        let data = "x".repeat(SSE_MAX_FRAME_BYTES + 1);
        let result = truncate_sse_event(&data, 1);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["truncated"], true);
    }
}
