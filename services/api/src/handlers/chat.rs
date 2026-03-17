//! Ollama chat handlers.

use axum::{
    extract::State,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::errors::AppError;
use crate::state::AppState;

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

    let session_id = req.session_id.unwrap_or_else(|| {
        format!("rustbrain-{}", chrono::Utc::now().timestamp_millis())
    });

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
