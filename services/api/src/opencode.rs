//! OpenCode API client for session and message management.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub slug: Option<String>,
    #[serde(default)]
    pub title: String,
    pub project_id: Option<String>,
    pub directory: Option<String>,
    pub version: Option<String>,
    pub summary: Option<SessionSummary>,
    pub time: SessionTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub additions: Option<i64>,
    pub deletions: Option<i64>,
    pub files: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTime {
    pub created: i64,
    pub updated: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub parts: Vec<MessagePart>,
    #[serde(default)]
    pub time: Option<MessageTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTime {
    pub created: Option<i64>,
}

/// Response from OpenCode's message list endpoint.
/// Shape: `[{info: {...}, parts: [...]}, ...]`
#[derive(Debug, Clone, Deserialize)]
pub struct MessageListEntry {
    pub info: MessageInfo,
    pub parts: Vec<MessagePart>,
}

impl From<MessageListEntry> for Message {
    fn from(entry: MessageListEntry) -> Self {
        Message {
            id: entry.info.id,
            role: entry.info.role.unwrap_or_else(|| "user".to_string()),
            parts: entry.parts,
            time: entry.info.time,
        }
    }
}

/// Response from OpenCode's send_message endpoint.
/// Shape: `{"info": {...}, "parts": [...]}`
#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    pub info: MessageInfo,
    pub parts: Vec<MessagePart>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    pub role: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub time: Option<MessageTime>,
}

impl SendMessageResponse {
    pub fn into_message(self) -> Message {
        Message {
            id: self.info.id,
            role: self.info.role.unwrap_or_else(|| "assistant".to_string()),
            parts: self.parts,
            time: self.info.time,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MessagePart {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    #[serde(rename = "tool-invocation")]
    ToolInvocation {
        #[serde(rename = "toolName", default)]
        tool_name: Option<String>,
        #[serde(default)]
        args: Option<serde_json::Value>,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "step-start")]
    StepStart {
        #[serde(default)]
        id: Option<String>,
    },
    #[serde(rename = "step-finish")]
    StepFinish {
        #[serde(default)]
        reason: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest {
    parts: Vec<SendMessagePart>,
}

#[derive(Debug, Serialize)]
struct SendMessagePart {
    r#type: String,
    text: String,
}

// =============================================================================
// Client
// =============================================================================

#[derive(Debug, Clone)]
pub struct OpenCodeClient {
    client: reqwest::Client,
    base_url: String,
    username: Option<String>,
    password: Option<String>,
}

impl OpenCodeClient {
    pub fn new(base_url: String, username: Option<String>, password: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))  // 10 minutes for long LLM responses
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            base_url,
            username,
            password,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let builder = self.client.request(method, &url);
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => builder.basic_auth(user, Some(pass)),
            (Some(user), None) => builder.basic_auth(user, None::<&str>),
            _ => builder,
        }
    }

    pub async fn health_check(&self) -> Result<bool> {
        let resp = self
            .request(reqwest::Method::GET, "/health")
            .send()
            .await
            .context("health_check request failed")?;
        Ok(resp.status().is_success())
    }

    pub async fn create_session(&self, title: Option<&str>) -> Result<Session> {
        let body = serde_json::json!({ "title": title.unwrap_or("New Session") });
        let resp = self
            .request(reqwest::Method::POST, "/session")
            .json(&body)
            .send()
            .await
            .context("create_session request failed")?;
        resp.error_for_status()?
            .json::<Session>()
            .await
            .context("create_session parse failed")
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        let resp = self
            .request(reqwest::Method::GET, "/session")
            .send()
            .await
            .context("list_sessions request failed")?;
        resp.error_for_status()?
            .json::<Vec<Session>>()
            .await
            .context("list_sessions parse failed")
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Session> {
        let path = format!("/session/{}", session_id);
        let resp = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await
            .context("get_session request failed")?;
        resp.error_for_status()?
            .json::<Session>()
            .await
            .context("get_session parse failed")
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let path = format!("/session/{}", session_id);
        self.request(reqwest::Method::DELETE, &path)
            .send()
            .await
            .context("delete_session request failed")?
            .error_for_status()?;
        Ok(())
    }

    pub async fn send_message(&self, session_id: &str, content: &str) -> Result<Message> {
        let path = format!("/session/{}/message", session_id);
        let body = SendMessageRequest {
            parts: vec![SendMessagePart {
                r#type: "text".to_string(),
                text: content.to_string(),
            }],
        };
        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .context("send_message request failed")?;
        let send_resp = resp
            .error_for_status()?
            .json::<SendMessageResponse>()
            .await
            .context("send_message parse failed")?;
        Ok(send_resp.into_message())
    }

    pub async fn send_message_async(&self, session_id: &str, content: &str) -> Result<String> {
        // OpenCode uses the same endpoint for async - it returns immediately with the message ID
        let path = format!("/session/{}/message", session_id);
        let body = SendMessageRequest {
            parts: vec![SendMessagePart {
                r#type: "text".to_string(),
                text: content.to_string(),
            }],
        };
        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .context("send_message_async request failed")?;
        let json: serde_json::Value = resp
            .error_for_status()?
            .json()
            .await
            .context("send_message_async parse failed")?;
        // The response has the message ID in info.id
        json["info"]["id"]
            .as_str()
            .or_else(|| json["id"].as_str())
            .context("send_message_async: missing id field")
            .map(|s| s.to_string())
    }

    pub async fn abort_session(&self, session_id: &str) -> Result<()> {
        let path = format!("/session/{}/abort", session_id);
        self.request(reqwest::Method::POST, &path)
            .send()
            .await
            .context("abort_session request failed")?
            .error_for_status()?;
        Ok(())
    }

    pub async fn fork_session(
        &self,
        session_id: &str,
        message_id: Option<&str>,
    ) -> Result<Session> {
        let path = format!("/session/{}/fork", session_id);
        let body = serde_json::json!({ "message_id": message_id });
        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .context("fork_session request failed")?;
        resp.error_for_status()?
            .json::<Session>()
            .await
            .context("fork_session parse failed")
    }

    pub async fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let path = format!("/session/{}/message", session_id);
        let resp = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await
            .context("get_messages request failed")?;
        let entries = resp
            .error_for_status()?
            .json::<Vec<MessageListEntry>>()
            .await
            .context("get_messages parse failed")?;
        Ok(entries.into_iter().map(Message::from).collect())
    }
}
