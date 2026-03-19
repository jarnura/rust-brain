//! Chat handlers — legacy Ollama + OpenCode streaming & sessions.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    Json,
};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use tracing::{debug, error, warn};

use crate::errors::AppError;
use crate::opencode;
use crate::state::AppState;

// =============================================================================
// Legacy Ollama Chat (kept for backwards compatibility)
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
}

pub async fn chat_handler(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    state.metrics.record_request("chat", "POST");
    debug!("Chat request: {:?}", req.message);

    // Try OpenCode first, fall back to Ollama.
    // Reuse existing session if the frontend passed one; otherwise create a new one.
    let opencode_session_id = if let Some(ref sid) = req.session_id {
        if sid.starts_with("ses_") {
            Some(sid.clone())
        } else {
            None
        }
    } else {
        None
    };

    // Attempt OpenCode
    let session_result = match opencode_session_id {
        Some(sid) => Ok(sid),
        None => state
            .opencode_client
            .create_session(Some("Chat"))
            .await
            .map(|s| s.id),
    };

    match session_result {
        Ok(sid) => {
            match state.opencode_client.send_message(&sid, &req.message).await {
                Ok(msg) => {
                    let response_text = msg.parts.iter()
                        .filter_map(|p| match p {
                            opencode::MessagePart::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    return Ok(Json(ChatResponse {
                        response: if response_text.is_empty() {
                            "I couldn't generate a response. Please try again.".to_string()
                        } else {
                            response_text
                        },
                        session_id: sid,
                    }));
                }
                Err(e) => {
                    warn!("OpenCode send_message failed, falling back to Ollama: {}", e);
                }
            }
        }
        Err(e) => {
            warn!("OpenCode create_session failed, falling back to Ollama: {}", e);
        }
    }

    let session_id = req.session_id.clone().unwrap_or_else(|| {
        format!("rustbrain-{}", chrono::Utc::now().timestamp_millis())
    });

    // Fallback: Ollama
    let system_prompt = r#"You are a helpful AI assistant with access to a Rust codebase knowledge graph.
You can help users understand code, find functions, trace call graphs, and answer questions about the codebase.
The codebase has been indexed with semantic search, call graphs, and type information.
Be concise but thorough in your responses. When discussing code, reference specific functions or modules when relevant."#;

    let chat_request = serde_json::json!({
        "model": state.config.chat_model,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": req.message}
        ],
        "stream": false
    });

    let response = state.http_client
        .post(format!("{}/api/chat", state.config.ollama_host))
        .json(&chat_request)
        .send()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to call Ollama chat: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Ollama(format!("Ollama chat failed: {} - {}", status, body)));
    }

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to parse Ollama response: {}", e)))?;

    let assistant_message = result.get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("I couldn't generate a response. Please try again.")
        .to_string();

    Ok(Json(ChatResponse {
        response: assistant_message,
        session_id,
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
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let base_url = state.opencode_client.base_url().to_string();

    let stream = async_stream::stream! {
        let url = format!("{}/event", base_url);
        match reqwest::get(&url).await {
            Ok(resp) => {
                let mut buffer = String::new();
                let mut accumulated_text = String::new();
                let mut was_busy = false;
                // Track part IDs → types to filter reasoning deltas
                let mut part_types: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                // Track message IDs → roles to filter user message events
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
                                    // Track message roles (user vs assistant)
                                    "message.updated" => {
                                        let info = &props["info"];
                                        if let (Some(id), Some(role)) = (info["id"].as_str(), info["role"].as_str()) {
                                            message_roles.insert(id.to_string(), role.to_string());
                                        }
                                    }

                                    // Streaming text deltas → token events
                                    "message.part.delta" => {
                                        let msg_id = props["messageID"].as_str().unwrap_or("");
                                        let is_assistant = message_roles.get(msg_id)
                                            .map(|r| r == "assistant").unwrap_or(false);
                                        if !is_assistant { continue; }

                                        // Look up part type by partID
                                        let part_id = props["partID"].as_str().unwrap_or("");
                                        let known_type = part_types.get(part_id).map(|s| s.as_str());

                                        // Only forward text deltas, skip reasoning
                                        if known_type == Some("text") {
                                            if let Some(delta) = props["delta"].as_str() {
                                                accumulated_text.push_str(delta);
                                                let payload = serde_json::json!({"token": delta});
                                                yield Ok(Event::default()
                                                    .event("token")
                                                    .data(payload.to_string()));
                                            }
                                        }
                                    }

                                    // Part registered or finalized — track type
                                    "message.part.updated" => {
                                        let part = &props["part"];
                                        // Register part ID → type
                                        if let (Some(id), Some(ptype)) = (part["id"].as_str(), part["type"].as_str()) {
                                            part_types.insert(id.to_string(), ptype.to_string());
                                        }

                                        // Only process assistant message parts
                                        let msg_id = part["messageID"].as_str().unwrap_or("");
                                        let is_assistant = message_roles.get(msg_id)
                                            .map(|r| r == "assistant").unwrap_or(false);
                                        if !is_assistant { continue; }

                                        match part["type"].as_str() {
                                            Some("text") => {
                                                // Final text — update accumulated
                                                if let Some(text) = part["text"].as_str() {
                                                    if !text.is_empty() {
                                                        accumulated_text = text.to_string();
                                                    }
                                                }
                                            }
                                            Some("tool-invocation") | Some("tool_invocation") => {
                                                let name = part["toolName"].as_str()
                                                    .or_else(|| part["tool_name"].as_str())
                                                    .unwrap_or("unknown");
                                                let args = &part["args"];
                                                let result = &part["result"];
                                                let status = if part["state"].as_str() == Some("error") {
                                                    "error"
                                                } else if !result.is_null() {
                                                    "done"
                                                } else {
                                                    "running"
                                                };
                                                let payload = serde_json::json!({
                                                    "name": name,
                                                    "args": args,
                                                    "status": status,
                                                    "result": result,
                                                });
                                                yield Ok(Event::default()
                                                    .event("tool_call")
                                                    .data(payload.to_string()));
                                            }
                                            Some("step-finish") => {
                                                // Only emit complete if we have accumulated text
                                                if !accumulated_text.is_empty() {
                                                    let payload = serde_json::json!({
                                                        "message": accumulated_text,
                                                    });
                                                    yield Ok(Event::default()
                                                        .event("complete")
                                                        .data(payload.to_string()));
                                                    accumulated_text.clear();
                                                }
                                            }
                                            _ => {}
                                        }
                                    }

                                    // Session status transitions
                                    "session.status" => {
                                        let status_type = props["status"]["type"].as_str().unwrap_or("");
                                        if status_type == "busy" {
                                            was_busy = true;
                                        } else if status_type == "idle" && was_busy {
                                            was_busy = false;
                                            // Flush any remaining text as complete
                                            if !accumulated_text.is_empty() {
                                                let payload = serde_json::json!({
                                                    "message": accumulated_text,
                                                });
                                                yield Ok(Event::default()
                                                    .event("complete")
                                                    .data(payload.to_string()));
                                                accumulated_text.clear();
                                            }
                                            // Signal that generation is fully done
                                            yield Ok(Event::default()
                                                .event("done")
                                                .data("{}".to_string()));
                                        }
                                    }

                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            error!("SSE stream error: {}", e);
                            yield Ok(Event::default()
                                .event("error")
                                .data(serde_json::json!({"error": e.to_string()}).to_string()));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect to OpenCode SSE: {}", e);
                yield Ok(Event::default()
                    .event("error")
                    .data(serde_json::json!({"error": format!("Failed to connect: {}", e)}).to_string()));
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

/// Async send — spawns the blocking OpenCode call in background, returns 204 immediately.
/// Results arrive via the SSE stream.
pub async fn chat_send_handler(
    State(state): State<AppState>,
    Json(req): Json<ChatSendRequest>,
) -> Result<impl IntoResponse, AppError> {
    let client = state.opencode_client.clone();
    let session_id = req.session_id.clone();
    let message = req.message.clone();

    // Spawn the blocking send in the background — SSE stream delivers events
    tokio::spawn(async move {
        if let Err(e) = client.send_message(&session_id, &message).await {
            warn!("Background send_message failed for session {}: {}", session_id, e);
        }
    });

    Ok(StatusCode::NO_CONTENT)
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
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<opencode::Session>, AppError> {
    let session = state.opencode_client
        .create_session(req.title.as_deref())
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to create session: {}", e)))?;
    Ok(Json(session))
}

pub async fn chat_sessions_list(
    State(state): State<AppState>,
) -> Result<Json<Vec<opencode::Session>>, AppError> {
    let sessions = state.opencode_client
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
    Path(id): Path<String>,
) -> Result<Json<SessionDetail>, AppError> {
    let session = state.opencode_client
        .get_session(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to get session: {}", e)))?;
    let messages = state.opencode_client
        .get_messages(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to get messages: {}", e)))?;
    Ok(Json(SessionDetail { session, messages }))
}

pub async fn chat_sessions_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.opencode_client
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
    Path(id): Path<String>,
    Json(req): Json<ForkSessionRequest>,
) -> Result<Json<opencode::Session>, AppError> {
    let session = state.opencode_client
        .fork_session(&id, req.message_id.as_deref())
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to fork session: {}", e)))?;
    Ok(Json(session))
}

pub async fn chat_sessions_abort(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    state.opencode_client
        .abort_session(&id)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to abort session: {}", e)))?;
    Ok(StatusCode::NO_CONTENT)
}
