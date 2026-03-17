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

    // Try OpenCode first, fall back to Ollama
    let session_id = req.session_id.clone().unwrap_or_else(|| {
        format!("rustbrain-{}", chrono::Utc::now().timestamp_millis())
    });

    // Attempt OpenCode
    match state.opencode_client.create_session(Some("Chat")).await {
        Ok(session) => {
            match state.opencode_client.send_message(&session.id, &req.message).await {
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
                        session_id: session.id,
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

/// SSE stream endpoint — bridges OpenCode events to the playground
pub async fn chat_stream_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let base_url = state.opencode_client.base_url().to_string();

    let stream = async_stream::stream! {
        // Connect to OpenCode SSE endpoint
        let url = format!("{}/event", base_url);
        match reqwest::get(&url).await {
            Ok(resp) => {
                let mut buffer = String::new();
                let bytes_stream = resp.bytes_stream();
                use futures_util::StreamExt;
                let mut bytes_stream = std::pin::pin!(bytes_stream);

                while let Some(chunk) = bytes_stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            // Parse SSE lines
                            while let Some(pos) = buffer.find("\n\n") {
                                let event_str = buffer[..pos].to_string();
                                buffer = buffer[pos + 2..].to_string();

                                // Extract data field
                                for line in event_str.lines() {
                                    if let Some(data) = line.strip_prefix("data: ") {
                                        yield Ok(Event::default().data(data.to_string()));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("SSE stream error: {}", e);
                            yield Ok(Event::default()
                                .event("error")
                                .data(format!("{{\"error\":\"{}\"}}", e)));
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to connect to OpenCode SSE: {}", e);
                yield Ok(Event::default()
                    .event("error")
                    .data(format!("{{\"error\":\"Failed to connect: {}\"}}", e)));
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

/// Async send — returns 204, results come via SSE stream
pub async fn chat_send_handler(
    State(state): State<AppState>,
    Json(req): Json<ChatSendRequest>,
) -> Result<impl IntoResponse, AppError> {
    state.opencode_client
        .send_message_async(&req.session_id, &req.message)
        .await
        .map_err(|e| AppError::OpenCode(format!("Failed to send message: {}", e)))?;

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
