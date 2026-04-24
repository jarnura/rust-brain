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
///
/// Unrecognized `type` values are captured by [`MessagePart::Unknown`],
/// which preserves the original `type` string and full JSON payload. This
/// ensures that new part types introduced by OpenCode are never silently
/// dropped (per RECONCILIATION.md R-4 P0 fix).
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "MessagePartHelper")]
pub enum MessagePart {
    /// Plain text content
    Text { text: String },
    /// Chain-of-thought reasoning (hidden from user display)
    Reasoning { text: String },
    /// A tool invocation with optional arguments and result.
    ///
    /// OpenCode sends `type: "tool"` with fields `tool` (name), `state`
    /// (nested `input`/`output`), `callID`, etc. Legacy format uses
    /// `type: "tool-invocation"` with `toolName`, `args`, `result`.
    ToolInvocation {
        #[serde(rename = "toolName", alias = "tool", default)]
        tool_name: Option<String>,
        #[serde(default)]
        args: Option<serde_json::Value>,
        #[serde(default)]
        result: Option<serde_json::Value>,
        /// OpenCode wraps input/output in a `state` object.
        #[serde(default)]
        state: Option<serde_json::Value>,
        /// Unique identifier for this tool call (e.g. `"functions.task:0"`).
        #[serde(rename = "callID", default)]
        call_id: Option<String>,
    },
    /// Marks the beginning of an agent step
    StepStart {
        #[serde(default)]
        id: Option<String>,
    },
    /// Marks the end of an agent step
    StepFinish {
        #[serde(default)]
        reason: Option<String>,
    },
    /// Catch-all for unrecognized part types.
    ///
    /// Stores the original `type` string and the full JSON object so that
    /// unknown events can be persisted as opaque agent_events rather than
    /// silently dropped.
    Unknown {
        /// The unrecognized `type` value from the JSON payload.
        raw_type: String,
        /// The full JSON object, preserved for display / debugging.
        raw: serde_json::Value,
    },
}

impl Serialize for MessagePart {
    /// Custom serialize: known variants via `KnownMessagePart` (tagged),
    /// `Unknown` emits its `raw` field (which already contains `"type"`).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            MessagePart::Text { text } => {
                KnownMessagePart::Text { text: text.clone() }.serialize(serializer)
            }
            MessagePart::Reasoning { text } => {
                KnownMessagePart::Reasoning { text: text.clone() }.serialize(serializer)
            }
            MessagePart::ToolInvocation {
                tool_name,
                args,
                result,
                state,
                call_id,
            } => KnownMessagePart::ToolInvocation {
                tool_name: tool_name.clone(),
                args: args.clone(),
                result: result.clone(),
                state: state.clone(),
                call_id: call_id.clone(),
            }
            .serialize(serializer),
            MessagePart::StepStart { id } => {
                KnownMessagePart::StepStart { id: id.clone() }.serialize(serializer)
            }
            MessagePart::StepFinish { reason } => KnownMessagePart::StepFinish {
                reason: reason.clone(),
            }
            .serialize(serializer),
            MessagePart::Unknown { raw, .. } => raw.serialize(serializer),
        }
    }
}

/// Intermediate helper for deserializing [`MessagePart`] with catch-all.
///
/// serde's `#[serde(tag = "type")]` + `#[serde(other)]` produces a unit
/// variant that cannot carry fields. We use a two-phase approach:
///
/// 1. Try the known tagged variants via [`KnownMessagePart`].
/// 2. If the `type` value doesn't match any known variant, fall back to
///    [`MessagePart::Unknown`] with the raw data.
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum KnownMessagePart {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    #[serde(rename = "tool-invocation", alias = "tool")]
    ToolInvocation {
        #[serde(rename = "toolName", alias = "tool", default)]
        tool_name: Option<String>,
        #[serde(default)]
        args: Option<serde_json::Value>,
        #[serde(default)]
        result: Option<serde_json::Value>,
        #[serde(default)]
        state: Option<serde_json::Value>,
        #[serde(rename = "callID", default)]
        call_id: Option<String>,
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
}

/// Helper struct that deserializes any JSON object into a [`MessagePart`].
///
/// If the `type` field matches a known variant, the corresponding
/// [`MessagePart`] is produced. Otherwise, the entire object is captured
/// as [`MessagePart::Unknown`].
#[derive(Deserialize)]
struct MessagePartHelper {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(flatten)]
    rest: serde_json::Value,
}

impl From<MessagePartHelper> for MessagePart {
    fn from(helper: MessagePartHelper) -> Self {
        let mut full = match helper.rest.as_object() {
            Some(obj) => obj.clone(),
            None => serde_json::Map::new(),
        };
        full.insert(
            "type".to_string(),
            serde_json::Value::String(helper.part_type.clone()),
        );
        let full_value = serde_json::Value::Object(full);

        match serde_json::from_value::<KnownMessagePart>(full_value.clone()) {
            Ok(known) => match known {
                KnownMessagePart::Text { text } => MessagePart::Text { text },
                KnownMessagePart::Reasoning { text } => MessagePart::Reasoning { text },
                KnownMessagePart::ToolInvocation {
                    tool_name,
                    args,
                    result,
                    state,
                    call_id,
                } => MessagePart::ToolInvocation {
                    tool_name,
                    args,
                    result,
                    state,
                    call_id,
                },
                KnownMessagePart::StepStart { id } => MessagePart::StepStart { id },
                KnownMessagePart::StepFinish { reason } => MessagePart::StepFinish { reason },
            },
            Err(_) => MessagePart::Unknown {
                raw_type: helper.part_type,
                raw: full_value,
            },
        }
    }
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
        let resp = resp.error_for_status()?;
        let bytes = resp.bytes().await.context("send_message read body")?;
        if bytes.is_empty() {
            // OpenCode returned async (empty body). Poll until the assistant
            // message appears, up to the client timeout.
            for _ in 0..120 {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                if let Ok(msgs) = self.get_messages(session_id).await {
                    if let Some(last) = msgs.into_iter().rev().find(|m| m.role == "assistant") {
                        return Ok(last);
                    }
                }
            }
            anyhow::bail!("send_message: no assistant response after polling");
        }
        let send_resp: SendMessageResponse =
            serde_json::from_slice(&bytes).context("send_message parse failed")?;
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_invocation_deserializes_opencode_format() {
        // Actual format from OpenCode API
        let json = serde_json::json!({
            "type": "tool",
            "tool": "task",
            "callID": "functions.task:0",
            "state": {
                "status": "completed",
                "input": {
                    "subagent_type": "explore",
                    "prompt": "Find auth patterns",
                    "description": "Search for authentication"
                },
                "output": "Found 5 matches",
                "metadata": {},
                "title": "Explorer task",
                "time": { "start": 1, "end": 2 }
            }
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::ToolInvocation {
                tool_name,
                args,
                result,
                state,
                call_id,
            } => {
                assert_eq!(
                    tool_name,
                    Some("task".to_string()),
                    "tool_name should be 'task'"
                );
                assert!(args.is_none(), "args should be None for OpenCode format");
                assert!(
                    result.is_none(),
                    "result should be None for OpenCode format"
                );
                assert!(state.is_some(), "state should be populated");
                assert_eq!(call_id, Some("functions.task:0".to_string()));

                let state = state.unwrap();
                assert_eq!(
                    state.get("status").and_then(|s| s.as_str()),
                    Some("completed")
                );
                let input = state.get("input").expect("state.input should exist");
                assert_eq!(
                    input.get("subagent_type").and_then(|s| s.as_str()),
                    Some("explore")
                );
            }
            _ => panic!("Expected ToolInvocation variant, got {part:?}"),
        }
    }

    #[test]
    fn tool_invocation_deserializes_legacy_format() {
        // Legacy format with toolName and args
        let json = serde_json::json!({
            "type": "tool-invocation",
            "toolName": "bash",
            "args": { "command": "ls -la" },
            "result": "file1.txt\nfile2.txt"
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::ToolInvocation {
                tool_name,
                args,
                result,
                state,
                ..
            } => {
                assert_eq!(tool_name, Some("bash".to_string()));
                assert!(args.is_some());
                assert!(result.is_some());
                assert!(state.is_none());
            }
            _ => panic!("Expected ToolInvocation variant"),
        }
    }

    #[test]
    fn tool_invocation_type_alias_matches_both_formats() {
        // Both "type": "tool" and "type": "tool-invocation" should work
        let json_tool = serde_json::json!({
            "type": "tool",
            "tool": "read",
            "state": { "input": { "file": "test.rs" } }
        });

        let json_tool_invocation = serde_json::json!({
            "type": "tool-invocation",
            "toolName": "read",
            "args": { "file": "test.rs" }
        });

        let part1: MessagePart =
            serde_json::from_value(json_tool).expect("Failed to deserialize tool");
        let part2: MessagePart = serde_json::from_value(json_tool_invocation)
            .expect("Failed to deserialize tool-invocation");

        // Both should be ToolInvocation variant
        assert!(matches!(part1, MessagePart::ToolInvocation { .. }));
        assert!(matches!(part2, MessagePart::ToolInvocation { .. }));
    }

    #[test]
    fn unknown_message_part_preserves_raw_type_and_json() {
        let json = serde_json::json!({
            "type": "image",
            "url": "https://example.com/diagram.png",
            "alt": "diagram"
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::Unknown { raw_type, raw } => {
                assert_eq!(raw_type, "image");
                assert_eq!(raw.get("type").and_then(|v| v.as_str()), Some("image"));
                assert_eq!(
                    raw.get("url").and_then(|v| v.as_str()),
                    Some("https://example.com/diagram.png")
                );
                assert_eq!(raw.get("alt").and_then(|v| v.as_str()), Some("diagram"));
            }
            _ => panic!("Expected Unknown variant, got {part:?}"),
        }
    }

    #[test]
    fn unknown_message_part_captures_custom_widget_type() {
        let json = serde_json::json!({
            "type": "custom-widget",
            "widget_name": "chart",
            "data": { "values": [1, 2, 3] }
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::Unknown { raw_type, raw } => {
                assert_eq!(raw_type, "custom-widget");
                assert_eq!(
                    raw.get("widget_name").and_then(|v| v.as_str()),
                    Some("chart")
                );
                assert!(raw.get("data").is_some());
            }
            _ => panic!("Expected Unknown variant, got {part:?}"),
        }
    }

    #[test]
    fn unknown_message_part_roundtrip_serialization() {
        let original = MessagePart::Unknown {
            raw_type: "video".to_string(),
            raw: serde_json::json!({
                "type": "video",
                "url": "https://example.com/video.mp4",
                "duration": 120
            }),
        };

        let serialized = serde_json::to_value(&original).expect("Failed to serialize");
        let deserialized: MessagePart =
            serde_json::from_value(serialized.clone()).expect("Failed to deserialize");

        match deserialized {
            MessagePart::Unknown { raw_type, raw } => {
                assert_eq!(raw_type, "video");
                assert_eq!(raw.get("type").and_then(|v| v.as_str()), Some("video"));
                assert_eq!(
                    raw.get("url").and_then(|v| v.as_str()),
                    Some("https://example.com/video.mp4")
                );
                assert_eq!(raw.get("duration").and_then(|v| v.as_i64()), Some(120));
            }
            _ => panic!("Expected Unknown variant after roundtrip, got {deserialized:?}"),
        }

        assert_eq!(
            serialized.get("type").and_then(|v| v.as_str()),
            Some("video")
        );
        assert_eq!(
            serialized.get("url").and_then(|v| v.as_str()),
            Some("https://example.com/video.mp4")
        );
    }

    #[test]
    fn text_variant_still_deserializes_correctly() {
        let json = serde_json::json!({
            "type": "text",
            "text": "Hello, world!"
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::Text { text } => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Expected Text variant, got {part:?}"),
        }
    }

    #[test]
    fn reasoning_variant_still_deserializes_correctly() {
        let json = serde_json::json!({
            "type": "reasoning",
            "text": "Let me think about this..."
        });

        let part: MessagePart = serde_json::from_value(json).expect("Failed to deserialize");

        match part {
            MessagePart::Reasoning { text } => {
                assert_eq!(text, "Let me think about this...");
            }
            _ => panic!("Expected Reasoning variant, got {part:?}"),
        }
    }
}
