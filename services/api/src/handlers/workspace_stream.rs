//! Workspace SSE stream handler.
//!
//! `GET /workspaces/:id/stream?execution_id=<uuid>` — server-sent events stream
//! that tails `agent_events` rows for the given execution.  Sends a keepalive
//! comment every 15 seconds and closes the stream automatically once the
//! execution reaches a terminal state (`completed`, `failed`, `timeout`,
//! `aborted`).

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use serde::{Deserialize, Serialize};
use tracing::warn;
use uuid::Uuid;

use crate::errors::AppError;
use crate::execution::{get_execution, list_agent_events_after, AgentEvent};
use crate::state::AppState;
use crate::workspace::get_workspace as db_get_workspace;

// =============================================================================
// Query params
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// The execution whose events to stream.
    pub execution_id: Uuid,
}

// =============================================================================
// SSE event payload
// =============================================================================

/// Wrapper sent as the SSE `data` field for each agent event.
#[derive(Debug, Serialize)]
pub struct SseAgentEvent {
    pub id: i64,
    pub execution_id: Uuid,
    pub phase: Option<String>,
    pub event_type: String,
    pub content: serde_json::Value,
    pub ts: String,
}

impl From<AgentEvent> for SseAgentEvent {
    fn from(ev: AgentEvent) -> Self {
        // Extract optional `phase` from the content JSON
        let phase = ev
            .content
            .get("phase")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Self {
            id: ev.id,
            execution_id: ev.execution_id,
            phase,
            event_type: ev.event_type,
            content: ev.content,
            ts: ev.timestamp.to_rfc3339(),
        }
    }
}

// =============================================================================
// Handler
// =============================================================================

/// `GET /workspaces/:id/stream?execution_id=<uuid>` — SSE event stream.
///
/// Returns a stream of `agent_event` SSE events.  Sends a `done` event and
/// closes the stream when the execution reaches a terminal state.
pub async fn stream_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, AppError> {
    // Verify workspace exists
    db_get_workspace(&state.workspace_manager.pool, workspace_id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", workspace_id)))?;

    // Verify execution exists and belongs to this workspace
    let execution = get_execution(&state.workspace_manager.pool, query.execution_id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch execution: {}", e)))?
        .ok_or_else(|| {
            AppError::NotFound(format!("Execution not found: {}", query.execution_id))
        })?;

    if execution.workspace_id != workspace_id {
        return Err(AppError::NotFound(format!(
            "Execution {} does not belong to workspace {}",
            query.execution_id, workspace_id
        )));
    }

    let pool = state.workspace_manager.pool.clone();
    let exec_id = query.execution_id;

    let event_stream = async_stream::stream! {
        let mut last_id: i64 = -1;
        let mut interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            interval.tick().await;

            // Fetch new events since last_id
            match list_agent_events_after(&pool, exec_id, last_id).await {
                Ok(events) => {
                    for ev in events {
                        last_id = ev.id;
                        let payload: SseAgentEvent = ev.into();
                        match serde_json::to_string(&payload) {
                            Ok(data) => {
                                yield Ok::<Event, Infallible>(
                                    Event::default().event("agent_event").data(data)
                                );
                            }
                            Err(e) => {
                                warn!(execution_id = %exec_id, error = %e, "Failed to serialize agent event");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(execution_id = %exec_id, error = %e, "Failed to fetch agent events");
                }
            }

            // Check terminal state
            match get_execution(&pool, exec_id).await {
                Ok(Some(ex)) if is_terminal(&ex.status) => {
                    let done_data = serde_json::json!({
                        "execution_id": exec_id,
                        "status": ex.status,
                    });
                    yield Ok(
                        Event::default()
                            .event("done")
                            .data(done_data.to_string())
                    );
                    break;
                }
                Ok(None) => {
                    // Execution was deleted — close stream
                    break;
                }
                _ => {}
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

/// Returns `true` for terminal execution statuses.
fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "timeout" | "aborted")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_terminal_completed() {
        assert!(is_terminal("completed"));
        assert!(is_terminal("failed"));
        assert!(is_terminal("timeout"));
        assert!(is_terminal("aborted"));
    }

    #[test]
    fn is_terminal_running_is_false() {
        assert!(!is_terminal("running"));
        assert!(!is_terminal("pending"));
    }

    #[test]
    fn sse_agent_event_from_agent_event() {
        let ev = AgentEvent {
            id: 42,
            execution_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            event_type: "phase_change".to_string(),
            content: serde_json::json!({"phase": "researching", "extra": "data"}),
        };
        let sse: SseAgentEvent = ev.into();
        assert_eq!(sse.id, 42);
        assert_eq!(sse.event_type, "phase_change");
        assert_eq!(sse.phase.as_deref(), Some("researching"));
    }

    #[test]
    fn sse_agent_event_no_phase_field() {
        let ev = AgentEvent {
            id: 1,
            execution_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            event_type: "reasoning".to_string(),
            content: serde_json::json!({"text": "thinking..."}),
        };
        let sse: SseAgentEvent = ev.into();
        assert!(sse.phase.is_none());
        assert_eq!(sse.event_type, "reasoning");
    }

    #[test]
    fn sse_agent_event_serializes() {
        let ev = AgentEvent {
            id: 5,
            execution_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            event_type: "tool_call".to_string(),
            content: serde_json::json!({"phase": "developing", "tool": "read_file"}),
        };
        let sse: SseAgentEvent = ev.into();
        let json = serde_json::to_value(&sse).unwrap();
        assert_eq!(json["event_type"], "tool_call");
        assert!(json["ts"].is_string());
        assert_eq!(json["phase"], "developing");
    }
}
