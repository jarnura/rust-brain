//! Execution REST handlers.
//!
//! | Method | Path | Handler | Description |
//! |--------|------|---------|-------------|
//! | POST | `/workspaces/:id/execute` | [`execute_workspace`] | Start an execution |
//! | GET  | `/workspaces/:id/executions` | [`list_executions`] | List executions for a workspace |
//! | GET  | `/executions/:id` | [`get_execution`] | Fetch execution status |
//! | GET  | `/executions/:id/events` | [`stream_events`] | SSE stream of agent events |

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

use crate::errors::AppError;
use crate::execution::runner::{run_execution, RunParams};
use crate::execution::{
    create_execution, get_execution as db_get_execution, list_agent_events_after,
    list_executions as db_list_executions, CreateExecutionParams,
};
use crate::state::AppState;
use crate::workspace::get_workspace;

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
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<ExecuteRequest>,
) -> Result<impl IntoResponse, AppError> {
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

/// `GET /executions/:id/events` — SSE stream of agent events.
///
/// The stream polls Postgres every 500 ms for new events. It terminates once
/// the execution reaches a terminal state (`completed`, `failed`, `aborted`,
/// `timeout`) and all pending events have been delivered.
pub async fn stream_events(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>>, AppError>
{
    // Verify execution exists before starting the stream
    db_get_execution(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch execution: {e}")))?
        .ok_or_else(|| AppError::NotFound(format!("Execution {id} not found")))?;

    let pool = state.workspace_manager.pool.clone();

    let stream = async_stream::stream! {
        let mut last_event_id: i64 = 0;
        let poll_interval = Duration::from_millis(500);

        loop {
            // Fetch new events since last seen
            let events = match list_agent_events_after(&pool, id, last_event_id).await {
                Ok(evts) => evts,
                Err(e) => {
                    // Emit an error event and stop
                    let payload = serde_json::json!({ "error": e.to_string() });
                    yield Ok(Event::default()
                        .event("error")
                        .data(payload.to_string()));
                    break;
                }
            };

            for event in &events {
                let data = serde_json::to_string(event).unwrap_or_default();
                yield Ok(Event::default()
                    .id(event.id.to_string())
                    .event("agent_event")
                    .data(data));
                last_event_id = event.id;
            }

            // Check execution terminal state
            match db_get_execution(&pool, id).await {
                Ok(Some(exec)) => {
                    let terminal = matches!(
                        exec.status.as_str(),
                        "completed" | "failed" | "aborted" | "timeout"
                    );
                    if terminal {
                        // Drain any remaining events before closing
                        if let Ok(remaining) = list_agent_events_after(&pool, id, last_event_id).await {
                            for event in &remaining {
                                let data = serde_json::to_string(event).unwrap_or_default();
                                yield Ok(Event::default()
                                    .id(event.id.to_string())
                                    .event("agent_event")
                                    .data(data));
                            }
                        }
                        // Emit a `done` event carrying the final status
                        let done_payload = serde_json::json!({ "status": exec.status }).to_string();
                        yield Ok(Event::default().event("done").data(done_payload));
                        break;
                    }
                }
                Ok(None) => break, // Execution disappeared
                Err(_) => {}       // Transient DB error — keep streaming
            }

            tokio::time::sleep(poll_interval).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
}
