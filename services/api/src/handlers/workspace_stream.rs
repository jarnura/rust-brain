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
    http::HeaderMap,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::warn;
use uuid::Uuid;

use crate::errors::AppError;
use crate::execution::{get_execution, list_agent_events_after_seq, AgentEvent};
use crate::state::AppState;
use crate::workspace::get_workspace as db_get_workspace;

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
// Query params
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    /// The execution whose events to stream.
    pub execution_id: Uuid,
    /// Optional cursor for SSE reconnect backfill. Alternative to the
    /// `Last-Event-ID` header for `EventSource` clients that cannot set
    /// headers. When both are present, the query string wins (explicit
    /// caller intent).
    #[serde(default)]
    pub last_event_id: Option<i64>,
}

// =============================================================================
// SSE event payload
// =============================================================================

/// Wrapper sent as the SSE `data` field for each agent event.
#[derive(Debug, Serialize)]
pub struct SseAgentEvent {
    pub id: i64,
    pub execution_id: Uuid,
    pub seq: i64,
    pub phase: Option<String>,
    pub event_type: String,
    pub content: serde_json::Value,
    pub ts: String,
}

impl From<AgentEvent> for SseAgentEvent {
    fn from(ev: AgentEvent) -> Self {
        let phase = ev
            .content
            .get("phase")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Self {
            id: ev.id,
            execution_id: ev.execution_id,
            seq: ev.seq,
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
///
/// Supports `Last-Event-ID` header for SSE reconnect backfill using seq-based
/// cursors. When the header is absent, streaming starts from seq 0 (all events).
pub async fn stream_workspace(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
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
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        let write_timeout = Duration::from_secs(SSE_WRITE_TIMEOUT_SECS);

        loop {
            interval.tick().await;

            match list_agent_events_after_seq(&pool, exec_id, last_seq).await {
                Ok(events) => {
                    for ev in events {
                        last_seq = ev.seq;
                        let payload: SseAgentEvent = ev.into();
                        match serde_json::to_string(&payload) {
                            Ok(data) => {
                                let truncated_data = truncate_sse_event(&data, last_seq);
                                let sse_event = Ok::<Event, Infallible>(
                                    Event::default()
                                        .id(last_seq.to_string())
                                        .event("agent_event")
                                        .data(truncated_data),
                                );

                                match tokio::time::timeout(write_timeout, tx.send(sse_event)).await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(_)) => return,
                                    Err(_) => {
                                        warn!(
                                            execution_id = %exec_id,
                                            "SSE write timeout ({}s) for execution {}, closing stream",
                                            SSE_WRITE_TIMEOUT_SECS, exec_id
                                        );
                                        return;
                                    }
                                }
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
                    let done_event = Ok(Event::default().event("done").data(done_data.to_string()));
                    let _ = tokio::time::timeout(write_timeout, tx.send(done_event)).await;
                    break;
                }
                Ok(None) => {
                    break;
                }
                _ => {}
            }
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
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
            seq: 3,
        };
        let sse: SseAgentEvent = ev.into();
        assert_eq!(sse.id, 42);
        assert_eq!(sse.seq, 3);
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
            seq: 1,
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
            seq: 7,
        };
        let sse: SseAgentEvent = ev.into();
        let json = serde_json::to_value(&sse).unwrap();
        assert_eq!(json["event_type"], "tool_call");
        assert!(json["ts"].is_string());
        assert_eq!(json["phase"], "developing");
        assert_eq!(json["seq"], 7);
    }

    #[test]
    fn truncate_sse_event_under_limit_passes_through() {
        let data = r#"{"event_type":"tool_call","content":{"tool":"search"}}"#;
        let result = truncate_sse_event(data, 1);
        assert_eq!(result, data);
    }

    #[test]
    fn truncate_sse_event_over_limit_returns_truncated() {
        let large = "x".repeat(SSE_MAX_FRAME_BYTES + 1000);
        let data = format!(r#"{{"content":{{"text":"{}"}}}}"#, large);
        let result = truncate_sse_event(&data, 5);
        assert!(result.len() < data.len());
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["truncated"], true);
        assert_eq!(parsed["full_payload_seq"], 5);
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
