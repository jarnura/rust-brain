//! Chat handlers — OpenCode streaming & sessions.

use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    Json,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::LazyLock;
use tokio::sync::Mutex;
use tracing::{debug, error};

use crate::errors::AppError;
use crate::middleware::auth::{require_chat_access, ApiKeyContext};
use crate::opencode;
use crate::state::AppState;

// =============================================================================
// Chat Event Buffer (for cursor-based SSE reconnection)
// =============================================================================

const CHAT_BUFFER_CAPACITY: usize = 500;

#[derive(Debug, Clone)]
struct BufferedChatEvent {
    seq: u64,
    event_type: String,
    data: String,
}

struct ChatEventBuffer {
    events: Vec<BufferedChatEvent>,
    next_seq: u64,
    capacity: usize,
}

impl ChatEventBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            events: Vec::new(),
            next_seq: 1,
            capacity,
        }
    }

    fn push(&mut self, event_type: String, data: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;

        if self.events.len() >= self.capacity {
            self.events.remove(0);
        }

        self.events.push(BufferedChatEvent {
            seq,
            event_type,
            data,
        });

        seq
    }

    fn events_after(&self, after_seq: u64) -> Vec<BufferedChatEvent> {
        self.events
            .iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect()
    }

    #[allow(dead_code)]
    fn latest_seq(&self) -> u64 {
        self.events.last().map(|e| e.seq).unwrap_or(0)
    }
}

static CHAT_STREAM_BUFFER: LazyLock<Mutex<ChatEventBuffer>> =
    LazyLock::new(|| Mutex::new(ChatEventBuffer::new(CHAT_BUFFER_CAPACITY)));

// =============================================================================
// Chat via OpenCode
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub response: String,
    pub session_id: String,
    pub source: String,
}

pub async fn chat_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    require_chat_access(&ctx)?;
    debug!("Chat request: {:?}", req.message);

    // Check if OpenCode is reachable before attempting to create a session
    match state.opencode_client.health_check().await {
        Ok(true) => {}
        _ => {
            return Err(AppError::OpenCode(
                "Chat is unavailable — the OpenCode service is not running. \
                 Search, call graph, types, traits, and all other code intelligence \
                 features work without it. To enable chat, start the OpenCode container: \
                 docker compose up -d opencode"
                    .to_string(),
            ));
        }
    }

    // Reuse existing session if the frontend passed one; otherwise create a new one.
    let session_id = if let Some(ref sid) = req.session_id {
        if sid.starts_with("ses_") {
            sid.clone()
        } else {
            state
                .opencode_client
                .create_session(Some("Chat"))
                .await
                .map_err(|e| AppError::OpenCode(format!("Failed to create session: {}", e)))?
                .id
        }
    } else {
        state
            .opencode_client
            .create_session(Some("Chat"))
            .await
            .map_err(|e| AppError::OpenCode(format!("Failed to create session: {}", e)))?
            .id
    };

    let msg = state
        .opencode_client
        .send_message(&session_id, &req.message)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to send message: {}", e)))?;

    let response_text = msg
        .parts
        .iter()
        .filter_map(|p| match p {
            opencode::MessagePart::Text { text } => Some(text.as_str()),
            opencode::MessagePart::Unknown { raw_type, .. } => {
                debug!(raw_type, "Non-Text MessagePart in chat handler");
                None
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");

    Ok(Json(ChatResponse {
        response: if response_text.is_empty() {
            "I couldn't generate a response. Please try again.".to_string()
        } else {
            response_text
        },
        session_id,
        source: "opencode".to_string(),
    }))
}

// =============================================================================
// OpenCode SSE Streaming
// =============================================================================

/// SSE stream endpoint — bridges OpenCode events to the playground.
///
/// Translates OpenCode event types into frontend-friendly events:
///   message.part.delta  (type=text)  → "token"  {token}
///   message.part.updated (type=text, with final text) → (buffered, emitted on finish)
///   message.part.updated (type=tool-invocation) → "tool_call" {name,args,status,result}
///   message.part.updated (type=step-finish)     → "complete" {message}
///   session.status (idle after busy)            → "complete" {message}
pub async fn chat_stream_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let base_url = state.opencode_client.base_url().to_string();
    let tier_denied = require_chat_access(&ctx).is_err();

    let initial_cursor = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let stream = async_stream::stream! {
        if tier_denied {
            let data_str = serde_json::json!({"error": "Chat access requires standard or admin tier"}).to_string();
            yield Ok(Event::default().event("error").data(data_str));
            yield Ok(Event::default().event("done").data("{\"source\":\"opencode\"}"));
            return;
        }

        if initial_cursor > 0 {
            let buffered = {
                let buf = CHAT_STREAM_BUFFER.lock().await;
                buf.events_after(initial_cursor)
            };
            for ev in buffered {
                yield Ok(Event::default()
                    .id(ev.seq.to_string())
                    .event(&ev.event_type)
                    .data(&ev.data));
            }
        }

        let url = format!("{}/event", base_url);
        match reqwest::get(&url).await {
            Ok(resp) => {
                let mut buffer = String::new();
                let mut accumulated_text = String::new();
                let mut was_busy = false;
                let mut part_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                let mut message_roles: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                let bytes_stream = resp.bytes_stream();
                use futures_util::StreamExt;
                let mut bytes_stream = std::pin::pin!(bytes_stream);

                while let Some(chunk) = bytes_stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));

                            while let Some(pos) = buffer.find("\n\n") {
                                let event_str = buffer[..pos].to_string();
                                buffer = buffer[pos + 2..].to_string();

                                let data_line = event_str.lines()
                                    .find_map(|l| l.strip_prefix("data: "));
                                let data = match data_line {
                                    Some(d) => d,
                                    None => continue,
                                };

                                let json: serde_json::Value = match serde_json::from_str(data) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };

                                let event_type = json["type"].as_str().unwrap_or("");
                                let props = &json["properties"];

                                match event_type {
                                    "message.updated" => {
                                        let info = &props["info"];
                                        if let (Some(id), Some(role)) = (info["id"].as_str(), info["role"].as_str()) {
                                            let is_new = !message_roles.contains_key(id);
                                            message_roles.insert(id.to_string(), role.to_string());
                                            if is_new && role == "assistant" {
                                                accumulated_text.clear();
                                                part_types.clear();
                                            }
                                        }
                                    }

                                    "message.part.delta" => {
                                        let msg_id = props["messageID"].as_str().unwrap_or("");
                                        let is_assistant = message_roles.get(msg_id)
                                            .map(|r| r == "assistant").unwrap_or(false);
                                        if !is_assistant { continue; }

                                        let part_id = props["partID"].as_str().unwrap_or("");
                                        let known_type = part_types.get(part_id).map(|s| s.as_str());

                                        if known_type == Some("text") {
                                            if let Some(delta) = props["delta"].as_str() {
                                                accumulated_text.push_str(delta);
                                                let payload = serde_json::json!({"token": delta});
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("token".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("token")
                                                    .data(data_str));
                                            }
                                        }
                                    }

                                    "message.part.updated" => {
                                        let part = &props["part"];
                                        if let (Some(id), Some(ptype)) = (part["id"].as_str(), part["type"].as_str()) {
                                            part_types.insert(id.to_string(), ptype.to_string());
                                        }

                                        let msg_id = part["messageID"].as_str().unwrap_or("");
                                        let is_assistant = message_roles.get(msg_id)
                                            .map(|r| r == "assistant").unwrap_or(false);
                                        if !is_assistant { continue; }

                                        match part["type"].as_str() {
                                            Some("text") => {
                                                if let Some(text) = part["text"].as_str() {
                                                    if !text.is_empty() {
                                                        accumulated_text = text.to_string();
                                                    }
                                                }
                                            }
                                            Some("tool-invocation") | Some("tool_invocation") => {
                                                let name = part["toolName"].as_str()
                                                    .or_else(|| part["tool_name"].as_str())
                                                    .or_else(|| part["tool"].as_str())
                                                    .unwrap_or("unknown");
                                                let args = &part["args"];
                                                let result = &part["result"];
                                                let state = &part["state"];
                                                let status = if state.get("error").is_some() || part["state"].as_str() == Some("error") {
                                                    "error"
                                                } else if !result.is_null() || state.get("output").is_some() {
                                                    "done"
                                                } else {
                                                    "running"
                                                };
                                                let args_val = if args.is_null() {
                                                    state.get("input").unwrap_or(args)
                                                } else {
                                                    args
                                                };
                                                let result_val = if result.is_null() {
                                                    state.get("output").unwrap_or(result)
                                                } else {
                                                    result
                                                };
                                                let payload = serde_json::json!({
                                                    "name": name,
                                                    "args": args_val,
                                                    "status": status,
                                                    "result": result_val,
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("tool_call".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("tool_call")
                                                    .data(data_str));
                                            }
                                            Some("reasoning") => {
                                                if let Some(text) = part["text"].as_str() {
                                                    let payload = serde_json::json!({
                                                        "text": text,
                                                    });
                                                    let data_str = payload.to_string();
                                                    let seq = CHAT_STREAM_BUFFER.lock().await.push("reasoning".to_string(), data_str.clone());
                                                    yield Ok(Event::default()
                                                        .id(seq.to_string())
                                                        .event("reasoning")
                                                        .data(data_str));
                                                }
                                            }
                                            Some("step-start") => {
                                                let id = part["id"].as_str();
                                                let payload = serde_json::json!({
                                                    "step": "start",
                                                    "id": id,
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("step".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("step")
                                                    .data(data_str));
                                            }
                                            Some("step-finish")
                                                if !accumulated_text.is_empty() =>
                                            {
                                                let payload = serde_json::json!({
                                                    "message": accumulated_text,
                                                    "source": "opencode",
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("complete".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("complete")
                                                    .data(data_str));
                                                accumulated_text.clear();
                                            }
                                            Some("step-finish") => {
                                                let reason = part["reason"].as_str();
                                                let payload = serde_json::json!({
                                                    "step": "finish",
                                                    "reason": reason,
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("step".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("step")
                                                    .data(data_str));
                                            }
                                            Some(unknown_type) => {
                                                debug!(
                                                    part_type = unknown_type,
                                                    "Unknown MessagePart type in SSE stream, forwarding as opaque event"
                                                );
                                                let payload = serde_json::json!({
                                                    "type": unknown_type,
                                                    "raw": part,
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("unknown_part".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("unknown_part")
                                                    .data(data_str));
                                            }
                                            None => {}
                                        }
                                    }

                                    "session.status" => {
                                        let status_type = props["status"]["type"].as_str().unwrap_or("");
                                        if status_type == "busy" {
                                            was_busy = true;
                                        } else if status_type == "idle" && was_busy {
                                            was_busy = false;
                                            if !accumulated_text.is_empty() {
                                                let payload = serde_json::json!({
                                                    "message": accumulated_text,
                                                    "source": "opencode",
                                                });
                                                let data_str = payload.to_string();
                                                let seq = CHAT_STREAM_BUFFER.lock().await.push("complete".to_string(), data_str.clone());
                                                yield Ok(Event::default()
                                                    .id(seq.to_string())
                                                    .event("complete")
                                                    .data(data_str));
                                                accumulated_text.clear();
                                            }
                                            let data_str = serde_json::json!({"source": "opencode"}).to_string();
                                            let seq = CHAT_STREAM_BUFFER.lock().await.push("done".to_string(), data_str.clone());
                                            yield Ok(Event::default()
                                                .id(seq.to_string())
                                                .event("done")
                                                .data(data_str));
                                        }
                                    }

                                    _ => {
                                        debug!(event_type, "Unhandled OpenCode SSE event type");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("SSE stream error: {}", e);
                            let data_str = serde_json::json!({"error": e.to_string()}).to_string();
                            let seq = CHAT_STREAM_BUFFER.lock().await.push("error".to_string(), data_str.clone());
                            yield Ok(Event::default()
                                .id(seq.to_string())
                                .event("error")
                                .data(data_str));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect to OpenCode SSE: {}", e);
                let data_str = serde_json::json!({"error": format!("Failed to connect: {}", e)}).to_string();
                let seq = CHAT_STREAM_BUFFER.lock().await.push("error".to_string(), data_str.clone());
                yield Ok(Event::default()
                    .id(seq.to_string())
                    .event("error")
                    .data(data_str));
            }
        }
    };

    Sse::new(stream)
}

// =============================================================================
// OpenCode Async Send
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct ChatSendRequest {
    pub session_id: String,
    pub message: String,
}

/// Async send — uses OpenCode's non-blocking endpoint, returns 202 with message ID.
/// Results arrive via the SSE stream.
pub async fn chat_send_handler(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Json(req): Json<ChatSendRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_chat_access(&ctx)?;
    let message_id = state
        .opencode_client
        .send_message_async(&req.session_id, &req.message)
        .await
        .map_err(|e| {
            error!(
                "send_message_async failed for session {}: {}",
                req.session_id, e
            );
            AppError::OpenCode(format!("Failed to send message: {}", e))
        })?;

    debug!(
        "Async message queued: {} for session {}",
        message_id, req.session_id
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"message_id": message_id})),
    ))
}

// =============================================================================
// Session Management
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
}

pub async fn chat_sessions_create(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<opencode::Session>, AppError> {
    require_chat_access(&ctx)?;
    let session = state
        .opencode_client
        .create_session(req.title.as_deref())
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to create session: {}", e)))?;
    Ok(Json(session))
}

pub async fn chat_sessions_list(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
) -> Result<Json<Vec<opencode::Session>>, AppError> {
    require_chat_access(&ctx)?;
    let sessions = state
        .opencode_client
        .list_sessions()
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to list sessions: {}", e)))?;
    Ok(Json(sessions))
}

#[derive(Debug, Serialize)]
pub struct SessionDetail {
    pub session: opencode::Session,
    pub messages: Vec<opencode::Message>,
}

pub async fn chat_sessions_get(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<String>,
) -> Result<Json<SessionDetail>, AppError> {
    require_chat_access(&ctx)?;
    let session = state
        .opencode_client
        .get_session(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to get session: {}", e)))?;
    let messages = state
        .opencode_client
        .get_messages(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to get messages: {}", e)))?;
    Ok(Json(SessionDetail { session, messages }))
}

pub async fn chat_sessions_delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    require_chat_access(&ctx)?;
    state
        .opencode_client
        .delete_session(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to delete session: {}", e)))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ForkSessionRequest {
    #[serde(default)]
    pub message_id: Option<String>,
}

pub async fn chat_sessions_fork(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<String>,
    Json(req): Json<ForkSessionRequest>,
) -> Result<Json<opencode::Session>, AppError> {
    require_chat_access(&ctx)?;
    let session = state
        .opencode_client
        .fork_session(&id, req.message_id.as_deref())
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to fork session: {}", e)))?;
    Ok(Json(session))
}

pub async fn chat_sessions_abort(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    require_chat_access(&ctx)?;
    state
        .opencode_client
        .abort_session(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to abort session: {}", e)))?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_event_buffer_push_and_retrieve() {
        let mut buf = ChatEventBuffer::new(100);
        let s1 = buf.push("token".to_string(), r#"{"token":"hello"}"#.to_string());
        let s2 = buf.push("tool_call".to_string(), r#"{"name":"search"}"#.to_string());
        let s3 = buf.push("complete".to_string(), r#"{"message":"done"}"#.to_string());

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
    fn chat_event_buffer_events_after_cursor() {
        let mut buf = ChatEventBuffer::new(100);
        buf.push("token".to_string(), "1".to_string());
        buf.push("token".to_string(), "2".to_string());
        buf.push("tool_call".to_string(), "3".to_string());
        buf.push("complete".to_string(), "4".to_string());
        buf.push("done".to_string(), "5".to_string());

        let after_2 = buf.events_after(2);
        assert_eq!(after_2.len(), 3);
        assert_eq!(after_2[0].seq, 3);
        assert_eq!(after_2[1].seq, 4);
        assert_eq!(after_2[2].seq, 5);
    }

    #[test]
    fn chat_event_buffer_capacity_eviction() {
        let mut buf = ChatEventBuffer::new(3);
        let _s1 = buf.push("token".to_string(), "1".to_string());
        let s2 = buf.push("token".to_string(), "2".to_string());
        let s3 = buf.push("token".to_string(), "3".to_string());
        let s4 = buf.push("token".to_string(), "4".to_string());

        assert_eq!(buf.events_after(0).len(), 3);
        let after_0 = buf.events_after(0);
        assert_eq!(after_0[0].seq, s2);
        assert_eq!(after_0[1].seq, s3);
        assert_eq!(after_0[2].seq, s4);
    }

    #[test]
    fn chat_event_buffer_latest_seq() {
        let mut buf = ChatEventBuffer::new(100);
        assert_eq!(buf.latest_seq(), 0);

        buf.push("token".to_string(), "1".to_string());
        assert_eq!(buf.latest_seq(), 1);

        buf.push("complete".to_string(), "2".to_string());
        assert_eq!(buf.latest_seq(), 2);
    }

    #[test]
    fn chat_event_buffer_events_after_empty() {
        let buf = ChatEventBuffer::new(100);
        assert!(buf.events_after(0).is_empty());
    }

    #[test]
    fn chat_event_buffer_events_after_beyond_latest() {
        let mut buf = ChatEventBuffer::new(100);
        buf.push("token".to_string(), "1".to_string());
        buf.push("token".to_string(), "2".to_string());
        assert!(buf.events_after(99).is_empty());
    }
}
