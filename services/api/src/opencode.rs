//! OpenCode API client for session and message management.
//!
//! [`OpenCodeClient`] wraps the OpenCode REST API, providing typed methods for
//! creating sessions, sending messages, and managing chat state. It is used by
//! the playground chat handlers to delegate LLM interactions to the OpenCode
//! container.
//!
//! All network methods return [`anyhow::Result`] and propagate HTTP and
//! deserialization errors with context.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// =============================================================================
// Types
// =============================================================================

/// An OpenCode chat session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique session identifier (e.g., `"ses_abc123"`)
    pub id: String,
    /// URL-friendly slug
    pub slug: Option<String>,
    /// Human-readable title
    #[serde(default)]
    pub title: String,
    /// Parent project identifier
    pub project_id: Option<String>,
    /// Working directory for the session
    pub directory: Option<String>,
    /// Protocol version
    pub version: Option<String>,
    /// Diff summary (lines added/deleted, files changed)
    pub summary: Option<SessionSummary>,
    /// Creation and last-update timestamps
    pub time: SessionTime,
}

/// Diff statistics for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Lines added
    pub additions: Option<i64>,
    /// Lines deleted
    pub deletions: Option<i64>,
    /// Files changed
    pub files: Option<i64>,
}

/// Unix-epoch timestamps for session lifecycle events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTime {
    /// When the session was created (Unix seconds)
    pub created: i64,
    /// When the session was last updated (Unix seconds)
    pub updated: i64,
}

/// A single chat message (user or assistant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message identifier
    pub id: String,
    /// `"user"` or `"assistant"`
    pub role: String,
    /// Ordered content parts (text, tool calls, step markers)
    pub parts: Vec<MessagePart>,
    /// Optional timestamp
    #[serde(default)]
    pub time: Option<MessageTime>,
}

/// Timestamp for a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageTime {
    /// When the message was created (Unix milliseconds)
    pub created: Option<i64>,
}

/// Response from OpenCode's message list endpoint.
///
/// Shape: `[{info: {...}, parts: [...]}, ...]`
#[derive(Debug, Clone, Deserialize)]
pub struct MessageListEntry {
    /// Message metadata (id, role, timestamps)
    pub info: MessageInfo,
    /// Ordered content parts
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

/// Response from OpenCode's `POST /session/{id}/message` endpoint.
///
/// Shape: `{"info": {...}, "parts": [...]}`
#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    /// Message metadata
    pub info: MessageInfo,
    /// Ordered content parts
    pub parts: Vec<MessagePart>,
}

/// Shared metadata fields present in both list and send responses.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageInfo {
    /// Unique message identifier
    pub id: String,
    /// `"user"` or `"assistant"` (absent for some internal messages)
    pub role: Option<String>,
    /// Session this message belongs to
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    /// Optional timestamp
    #[serde(default)]
    pub time: Option<MessageTime>,
}

impl SendMessageResponse {
    /// Converts this response into a [`Message`], defaulting role to `"assistant"`.
    pub fn into_message(self) -> Message {
        Message {
            id: self.info.id,
            role: self.info.role.unwrap_or_else(|| "assistant".to_string()),
            parts: self.parts,
            time: self.info.time,
        }
    }
}

/// A single content part within a [`Message`].
///
/// Messages are composed of an ordered list of parts. The `type` field in
/// JSON selects the variant (internally tagged, kebab-case).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MessagePart {
    /// Plain text content
    Text { text: String },
    /// Chain-of-thought reasoning (hidden from user display)
    Reasoning { text: String },
    /// A tool invocation with optional arguments and result
    #[serde(rename = "tool-invocation")]
    ToolInvocation {
        #[serde(rename = "toolName", default)]
        tool_name: Option<String>,
        #[serde(default)]
        args: Option<serde_json::Value>,
        #[serde(default)]
        result: Option<serde_json::Value>,
    },
    /// Marks the beginning of an agent step
    #[serde(rename = "step-start")]
    StepStart {
        #[serde(default)]
        id: Option<String>,
    },
    /// Marks the end of an agent step
    #[serde(rename = "step-finish")]
    StepFinish {
        #[serde(default)]
        reason: Option<String>,
    },
    /// Catch-all for unrecognized part types
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

/// HTTP client for the OpenCode session management API.
///
/// Wraps `reqwest::Client` with a 10-minute timeout (to accommodate long
/// LLM response times) and optional HTTP Basic Auth credentials.
#[derive(Debug, Clone)]
pub struct OpenCodeClient {
    client: reqwest::Client,
    base_url: String,
    username: Option<String>,
    password: Option<String>,
}

impl OpenCodeClient {
    /// Creates a new client targeting the given OpenCode base URL.
    ///
    /// If `username` and `password` are provided, every request will include
    /// an `Authorization: Basic ...` header.
    ///
    /// # Panics
    ///
    /// Panics if the internal `reqwest::Client` cannot be constructed (e.g.,
    /// TLS backend unavailable).
    pub fn new(base_url: String, username: Option<String>, password: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600)) // 10 minutes for long LLM responses
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            base_url,
            username,
            password,
        }
    }

    /// Returns the configured base URL (e.g., `http://opencode:4096`).
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

    /// Checks whether the OpenCode server is reachable.
    ///
    /// Returns `true` if `GET /health` returns a 2xx status.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request itself fails (connection refused,
    /// timeout, DNS resolution failure).
    pub async fn health_check(&self) -> Result<bool> {
        let resp = self
            .request(reqwest::Method::GET, "/health")
            .send()
            .await
            .context("health_check request failed")?;
        Ok(resp.status().is_success())
    }

    /// Creates a new chat session.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, or JSON parse failure.
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

    /// Lists all existing sessions.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, or JSON parse failure.
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

    /// Retrieves a session by its identifier.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status (including 404 if
    /// the session does not exist), or JSON parse failure.
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

    /// Deletes a session and all its messages.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure or non-2xx status.
    pub async fn delete_session(&self, session_id: &str) -> Result<()> {
        let path = format!("/session/{}", session_id);
        self.request(reqwest::Method::DELETE, &path)
            .send()
            .await
            .context("delete_session request failed")?
            .error_for_status()?;
        Ok(())
    }

    /// Sends a text message and waits for the full assistant response.
    ///
    /// This is a blocking call that waits for the LLM to finish generating
    /// (up to the 10-minute client timeout).
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, or JSON parse failure.
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

    /// Sends a text message without waiting for the response.
    ///
    /// Returns the message ID immediately. The assistant response arrives
    /// via the SSE event stream.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, JSON parse
    /// failure, or if the response is missing the `id` field.
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

    /// Aborts any in-progress generation for the given session.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure or non-2xx status.
    pub async fn abort_session(&self, session_id: &str) -> Result<()> {
        let path = format!("/session/{}/abort", session_id);
        self.request(reqwest::Method::POST, &path)
            .send()
            .await
            .context("abort_session request failed")?
            .error_for_status()?;
        Ok(())
    }

    /// Forks a session, optionally from a specific message.
    ///
    /// Creates a new session containing messages up to `message_id` (or all
    /// messages if `None`).
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, or JSON parse failure.
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

    /// Retrieves all messages for a session, ordered chronologically.
    ///
    /// # Errors
    ///
    /// Returns an error on network failure, non-2xx status, or JSON parse failure.
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
