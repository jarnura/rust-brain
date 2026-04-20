//! Typed event schema for the OpenCode agent protocol.
//!
//! Events are produced by the execution runner
//! (`services/api/src/execution/runner.rs`) and persisted as rows in the
//! `agent_events` Postgres table. Each row carries an `event_type` string
//! and a `content` JSONB value.
//!
//! This module defines strongly-typed Rust structs for all 7 known event
//! content shapes, plus an [`EventContent::Unknown`] fallback that preserves
//! raw data for event types not yet recognised by this version of the crate.
//!
//! Per `docs/opencode-tracing/RECONCILIATION.md` R-1/R-2:
//!
//! - Tool calls are **atomic** — each event is self-contained with no
//!   streaming update phases and no correlation keys.
//! - Unknown `MessagePart` variants must be stored as opaque events rather
//!   than silently dropped (R-4 P0 fix).
//!
//! # Wire format
//!
//! The `content` JSONB column uses an internally-tagged representation
//! with `"kind"` as the discriminant, mirroring the TypeScript
//! `TypedEventContent` discriminated union in the frontend:
//!
//! ```json
//! { "kind": "reasoning", "agent": "research", "text": "..." }
//! { "kind": "tool_call",  "agent": "develop", "tool": "read_file", ... }
//! { "kind": "unknown",    "raw_event_type": "new_thing", "raw": { ... } }
//! ```
//!
//! The [`EventType`] enum captures the 7 allowed `event_type` values from
//! the Postgres `CHECK` constraint in
//! `services/api/migrations/20260403000003_agent_events.sql`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, trace, warn};

// =============================================================================
// EventType — mirrors the Postgres CHECK constraint
// =============================================================================

/// The kind of agent event, stored in the `event_type` column.
///
/// Matches the `CHECK (event_type IN (...))` constraint in the
/// `agent_events` table migration.
///
/// Serialized as snake_case strings (e.g., `"tool_call"`, `"agent_dispatch"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Agent reasoning or text output.
    Reasoning,
    /// Atomic tool invocation (input + output delivered together).
    ToolCall,
    /// File edit event.
    FileEdit,
    /// Runner-side error.
    Error,
    /// Phase transition event (legacy; superseded by `AgentDispatch`).
    PhaseChange,
    /// Sub-agent dispatch detected from a `task` tool call.
    AgentDispatch,
    /// Container kept-alive heartbeat for debugging sessions.
    ContainerKeptAlive,
    /// Unrecognised event type — preserves raw data instead of dropping.
    Unknown,
}

impl EventType {
    /// Returns the snake_case string representation used in the database
    /// and JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::Reasoning => "reasoning",
            EventType::ToolCall => "tool_call",
            EventType::FileEdit => "file_edit",
            EventType::Error => "error",
            EventType::PhaseChange => "phase_change",
            EventType::AgentDispatch => "agent_dispatch",
            EventType::ContainerKeptAlive => "container_kept_alive",
            EventType::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for EventType {
    type Err = String;

    /// Parses an event type from its snake_case string representation.
    ///
    /// # Errors
    ///
    /// Returns an error string for any value not in the known set.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        trace!(input = s, "EventType::from_str entry");
        let result = match s {
            "reasoning" => Ok(EventType::Reasoning),
            "tool_call" => Ok(EventType::ToolCall),
            "file_edit" => Ok(EventType::FileEdit),
            "error" => Ok(EventType::Error),
            "phase_change" => Ok(EventType::PhaseChange),
            "agent_dispatch" => Ok(EventType::AgentDispatch),
            "container_kept_alive" => Ok(EventType::ContainerKeptAlive),
            "unknown" => Ok(EventType::Unknown),
            _ => Err(format!("Unknown event type: {}", s)),
        };
        match &result {
            Ok(t) => debug!(event_type = ?t, "EventType::from_str success"),
            Err(e) => warn!(input = s, error = %e, "EventType::from_str failed"),
        }
        result
    }
}

// =============================================================================
// Content structs — one per event shape
// =============================================================================

/// Reasoning / text content emitted by an agent.
///
/// Wire examples:
/// - `{ "kind": "reasoning", "agent": "research", "text": "I should look at..." }`
/// - `{ "kind": "reasoning", "agent": "research", "reasoning": "I should look at..." }`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasoningContent {
    /// Name of the agent that produced this reasoning.
    pub agent: String,
    /// The reasoning text. Named `text` in the canonical shape; the legacy
    /// `reasoning` key is accepted on deserialization and normalised here.
    #[serde(alias = "reasoning")]
    pub text: String,
}

/// Atomic tool invocation — both `args` and `result` are delivered together.
///
/// Per RECONCILIATION.md R-2, tool calls are single-shot: there is no
/// separate "call" / "update" / "result" phase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallContent {
    /// Name of the agent that invoked the tool.
    pub agent: String,
    /// Tool name (e.g., `"read_file"`, `"task"`).
    pub tool: Option<String>,
    /// Tool input arguments. `None` if not yet available or not applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// Tool output result. `None` if not yet available or not applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// Sub-agent dispatch event minted when a `task` tool call is detected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentDispatchContent {
    /// Name of the dispatched sub-agent.
    pub agent: String,
}

/// Runner-side error. `stage` is present when the failure is tied to a
/// pipeline stage (e.g., `"container_spawn"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorContent {
    /// Error message.
    pub error: String,
    /// Pipeline stage where the error occurred, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

/// File edit event. Only `path` is required; additional fields are
/// best-effort and preserved via the `extra` map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileEditContent {
    /// Path of the edited file.
    pub path: String,
    /// Additional fields from the content JSON that are not individually typed.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Legacy phase transition event (superseded by [`AgentDispatchContent`]
/// but still emitted by the runner).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhaseChangeContent {
    /// Target phase name (e.g., `"researching"`, `"developing"`).
    pub phase: String,
}

/// Container kept-alive heartbeat for debugging sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContainerKeptAliveContent {
    /// ISO 8601 timestamp when the container expires.
    pub expires_at: String,
    /// Duration in seconds the container will stay alive.
    pub keep_alive_secs: i64,
}

/// Fallback for unrecognised event types or content that fails shape
/// validation.
///
/// Per RECONCILIATION.md R-4, unknown `MessagePart` types must be stored
/// as opaque events rather than silently dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnknownContent {
    /// The original `event_type` string from the database row.
    pub raw_event_type: String,
    /// The raw content JSON object, preserved for display / debugging.
    #[serde(with = "serde_json::Value")]
    pub raw: serde_json::Value,
}

// =============================================================================
// EventContent — discriminated union
// =============================================================================

/// All recognised content variants, discriminated by the `"kind"` JSON key.
///
/// Uses `#[serde(tag = "kind")]` for an internally-tagged representation
/// that matches the TypeScript `TypedEventContent` discriminated union in
/// the frontend.
///
/// The [`EventContent::Unknown`] variant captures event types not yet
/// recognised by this version of the crate — they are never silently
/// dropped.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventContent {
    /// Agent reasoning or text output.
    Reasoning(ReasoningContent),
    /// Atomic tool invocation.
    ToolCall(ToolCallContent),
    /// Sub-agent dispatch.
    AgentDispatch(AgentDispatchContent),
    /// Runner-side error.
    Error(ErrorContent),
    /// File edit.
    FileEdit(FileEditContent),
    /// Legacy phase transition.
    PhaseChange(PhaseChangeContent),
    /// Container kept-alive heartbeat.
    ContainerKeptAlive(ContainerKeptAliveContent),
    /// Unrecognised event — preserves raw data instead of dropping.
    Unknown(UnknownContent),
}

impl EventContent {
    /// Returns the [`EventType`] that corresponds to this content variant.
    pub fn event_type(&self) -> Option<EventType> {
        match self {
            EventContent::Reasoning(_) => Some(EventType::Reasoning),
            EventContent::ToolCall(_) => Some(EventType::ToolCall),
            EventContent::AgentDispatch(_) => Some(EventType::AgentDispatch),
            EventContent::Error(_) => Some(EventType::Error),
            EventContent::FileEdit(_) => Some(EventType::FileEdit),
            EventContent::PhaseChange(_) => Some(EventType::PhaseChange),
            EventContent::ContainerKeptAlive(_) => Some(EventType::ContainerKeptAlive),
            EventContent::Unknown(_) => Some(EventType::Unknown),
        }
    }

    /// Parse an `EventContent` from a raw `event_type` string and `content`
    /// JSONB value as stored in Postgres.
    ///
    /// This is the primary entry point for deserialising events read from
    /// the database. If `event_type` is known, the `content` JSON is
    /// deserialised into the corresponding typed struct. If `event_type`
    /// is unknown **or** deserialisation fails, an [`EventContent::Unknown`]
    /// variant is returned — this function never returns an error.
    ///
    /// # Unknown handling
    ///
    /// - Unrecognised `event_type` → `Unknown { raw_event_type, raw }`
    /// - Known `event_type` but malformed `content` → `Unknown { raw_event_type, raw }`
    /// - Non-object `content` (e.g., a JSON string or array) → `Unknown`
    pub fn from_raw(event_type: &str, content: &serde_json::Value) -> Self {
        trace!(event_type = event_type, "EventContent::from_raw entry");

        // Fast path: if the content already has a "kind" field, try direct
        // deserialisation via the tagged enum.
        if content.is_object() && content.get("kind").is_some() {
            match serde_json::from_value::<EventContent>(content.clone()) {
                Ok(ec) => {
                    debug!(
                        event_type = event_type,
                        "EventContent::from_raw tagged deserialisation success"
                    );
                    return ec;
                }
                Err(e) => {
                    warn!(
                        event_type = event_type,
                        error = %e,
                        "EventContent::from_raw tagged deserialisation failed, falling back to Unknown"
                    );
                }
            }
        }

        // Slow path: construct the tagged payload from event_type + content.
        let event_type_enum = event_type.parse::<EventType>();
        match event_type_enum {
            Ok(et) => {
                // Inject the "kind" tag so the tagged enum can deserialise.
                let mut tagged = match content.as_object() {
                    Some(obj) => obj.clone(),
                    None => serde_json::Map::new(),
                };
                tagged.insert(
                    "kind".to_string(),
                    serde_json::Value::String(et.as_str().to_string()),
                );

                let tagged_value = serde_json::Value::Object(tagged);
                match serde_json::from_value::<EventContent>(tagged_value) {
                    Ok(ec) => {
                        debug!(
                            event_type = event_type,
                            "EventContent::from_raw manual-tag deserialisation success"
                        );
                        ec
                    }
                    Err(e) => {
                        warn!(
                            event_type = event_type,
                            error = %e,
                            "EventContent::from_raw manual-tag deserialisation failed, falling back to Unknown"
                        );
                        EventContent::Unknown(UnknownContent {
                            raw_event_type: event_type.to_string(),
                            raw: content.clone(),
                        })
                    }
                }
            }
            Err(_) => {
                warn!(
                    event_type = event_type,
                    "EventContent::from_raw unknown event_type, storing as Unknown"
                );
                EventContent::Unknown(UnknownContent {
                    raw_event_type: event_type.to_string(),
                    raw: content.clone(),
                })
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- EventType roundtrips ---

    #[test]
    fn event_type_roundtrip() {
        for (s, et) in [
            ("reasoning", EventType::Reasoning),
            ("tool_call", EventType::ToolCall),
            ("file_edit", EventType::FileEdit),
            ("error", EventType::Error),
            ("phase_change", EventType::PhaseChange),
            ("agent_dispatch", EventType::AgentDispatch),
            ("container_kept_alive", EventType::ContainerKeptAlive),
            ("unknown", EventType::Unknown),
        ] {
            assert_eq!(et.as_str(), s);
            assert_eq!(s.parse::<EventType>().unwrap(), et);
        }
    }

    #[test]
    fn event_type_display() {
        assert_eq!(EventType::ToolCall.to_string(), "tool_call");
        assert_eq!(EventType::AgentDispatch.to_string(), "agent_dispatch");
    }

    #[test]
    fn event_type_unknown_string() {
        assert!("no_such_type".parse::<EventType>().is_err());
        assert_eq!("unknown".parse::<EventType>().unwrap(), EventType::Unknown);
    }

    #[test]
    fn event_type_serialization() {
        let json = serde_json::to_string(&EventType::ToolCall).unwrap();
        assert_eq!(json, "\"tool_call\"");
        let back: EventType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, EventType::ToolCall);
    }

    // --- ReasoningContent ---

    #[test]
    fn reasoning_content_text_key() {
        let json = serde_json::json!({
            "kind": "reasoning",
            "agent": "research",
            "text": "I should look at..."
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::Reasoning(r) => {
                assert_eq!(r.agent, "research");
                assert_eq!(r.text, "I should look at...");
            }
            _ => panic!("Expected Reasoning variant"),
        }
    }

    #[test]
    fn reasoning_content_legacy_reasoning_key() {
        // The runner emits {"agent": "...", "reasoning": "..."} for
        // MessagePart::Reasoning variants. The `#[serde(alias)]` on
        // ReasoningContent::text handles this.
        let json = serde_json::json!({
            "kind": "reasoning",
            "agent": "research",
            "reasoning": "I should look at..."
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::Reasoning(r) => {
                assert_eq!(r.text, "I should look at...");
            }
            _ => panic!("Expected Reasoning variant"),
        }
    }

    // --- ToolCallContent ---

    #[test]
    fn tool_call_content_full() {
        let json = serde_json::json!({
            "kind": "tool_call",
            "agent": "develop",
            "tool": "read_file",
            "args": {"path": "/src/main.rs"},
            "result": {"content": "fn main() {}"}
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::ToolCall(tc) => {
                assert_eq!(tc.agent, "develop");
                assert_eq!(tc.tool.as_deref(), Some("read_file"));
                assert!(tc.args.is_some());
                assert!(tc.result.is_some());
            }
            _ => panic!("Expected ToolCall variant"),
        }
    }

    #[test]
    fn tool_call_content_minimal() {
        // Atomic tool call without args/result (per R-2: tool calls may
        // arrive without input/output in some edge cases).
        let json = serde_json::json!({
            "kind": "tool_call",
            "agent": "develop",
            "tool": "bash"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::ToolCall(tc) => {
                assert_eq!(tc.agent, "develop");
                assert!(tc.args.is_none());
                assert!(tc.result.is_none());
            }
            _ => panic!("Expected ToolCall variant"),
        }
    }

    // --- AgentDispatchContent ---

    #[test]
    fn agent_dispatch_content() {
        let json = serde_json::json!({
            "kind": "agent_dispatch",
            "agent": "explore"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::AgentDispatch(ad) => {
                assert_eq!(ad.agent, "explore");
            }
            _ => panic!("Expected AgentDispatch variant"),
        }
    }

    // --- ErrorContent ---

    #[test]
    fn error_content_with_stage() {
        let json = serde_json::json!({
            "kind": "error",
            "error": "spawn failed",
            "stage": "container_spawn"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::Error(e) => {
                assert_eq!(e.error, "spawn failed");
                assert_eq!(e.stage.as_deref(), Some("container_spawn"));
            }
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn error_content_without_stage() {
        let json = serde_json::json!({
            "kind": "error",
            "error": "timeout"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::Error(e) => {
                assert_eq!(e.error, "timeout");
                assert!(e.stage.is_none());
            }
            _ => panic!("Expected Error variant"),
        }
    }

    // --- FileEditContent ---

    #[test]
    fn file_edit_content_with_extra() {
        let json = serde_json::json!({
            "kind": "file_edit",
            "path": "/src/main.rs",
            "diff": "+fn main() {}"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::FileEdit(fe) => {
                assert_eq!(fe.path, "/src/main.rs");
                assert_eq!(
                    fe.extra.get("diff").unwrap().as_str(),
                    Some("+fn main() {}")
                );
            }
            _ => panic!("Expected FileEdit variant"),
        }
    }

    // --- PhaseChangeContent ---

    #[test]
    fn phase_change_content() {
        let json = serde_json::json!({
            "kind": "phase_change",
            "phase": "researching"
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::PhaseChange(pc) => {
                assert_eq!(pc.phase, "researching");
            }
            _ => panic!("Expected PhaseChange variant"),
        }
    }

    // --- ContainerKeptAliveContent ---

    #[test]
    fn container_kept_alive_content() {
        let json = serde_json::json!({
            "kind": "container_kept_alive",
            "expires_at": "2026-04-21T12:00:00Z",
            "keep_alive_secs": 3600
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::ContainerKeptAlive(ck) => {
                assert_eq!(ck.expires_at, "2026-04-21T12:00:00Z");
                assert_eq!(ck.keep_alive_secs, 3600);
            }
            _ => panic!("Expected ContainerKeptAlive variant"),
        }
    }

    // --- UnknownContent ---

    #[test]
    fn unknown_content_explicit() {
        let json = serde_json::json!({
            "kind": "unknown",
            "raw_event_type": "new_thing",
            "raw": {"foo": "bar"}
        });
        let ec: EventContent = serde_json::from_value(json).unwrap();
        match ec {
            EventContent::Unknown(u) => {
                assert_eq!(u.raw_event_type, "new_thing");
                assert_eq!(u.raw["foo"], "bar");
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    // --- EventContent::from_raw ---

    #[test]
    fn from_raw_tagged_content() {
        // Content already has "kind" field
        let content = serde_json::json!({
            "kind": "reasoning",
            "agent": "research",
            "text": "thinking..."
        });
        let ec = EventContent::from_raw("reasoning", &content);
        match ec {
            EventContent::Reasoning(r) => {
                assert_eq!(r.agent, "research");
                assert_eq!(r.text, "thinking...");
            }
            _ => panic!("Expected Reasoning variant"),
        }
    }

    #[test]
    fn from_raw_untagged_content() {
        // Content from the DB may not have "kind" — from_raw injects it
        let content = serde_json::json!({
            "agent": "develop",
            "tool": "bash",
            "args": null,
            "result": null
        });
        let ec = EventContent::from_raw("tool_call", &content);
        match ec {
            EventContent::ToolCall(tc) => {
                assert_eq!(tc.agent, "develop");
                assert_eq!(tc.tool.as_deref(), Some("bash"));
            }
            _ => panic!("Expected ToolCall variant, got {:?}", ec),
        }
    }

    #[test]
    fn from_raw_unknown_event_type() {
        let content = serde_json::json!({"something": "else"});
        let ec = EventContent::from_raw("future_event_type", &content);
        match ec {
            EventContent::Unknown(u) => {
                assert_eq!(u.raw_event_type, "future_event_type");
                assert_eq!(u.raw["something"], "else");
            }
            _ => panic!("Expected Unknown variant"),
        }
    }

    #[test]
    fn from_raw_malformed_content() {
        // Known event_type but content doesn't match expected shape
        let content = serde_json::json!("just a string");
        let ec = EventContent::from_raw("reasoning", &content);
        match ec {
            EventContent::Unknown(u) => {
                assert_eq!(u.raw_event_type, "reasoning");
            }
            _ => panic!("Expected Unknown fallback for malformed content"),
        }
    }

    #[test]
    fn from_raw_error_with_stage() {
        let content = serde_json::json!({
            "error": "spawn failed",
            "stage": "container_spawn"
        });
        let ec = EventContent::from_raw("error", &content);
        match ec {
            EventContent::Error(e) => {
                assert_eq!(e.error, "spawn failed");
                assert_eq!(e.stage.as_deref(), Some("container_spawn"));
            }
            _ => panic!("Expected Error variant"),
        }
    }

    #[test]
    fn from_raw_container_kept_alive() {
        let content = serde_json::json!({
            "expires_at": "2026-04-21T12:00:00Z",
            "keep_alive_secs": 3600
        });
        let ec = EventContent::from_raw("container_kept_alive", &content);
        match ec {
            EventContent::ContainerKeptAlive(ck) => {
                assert_eq!(ck.keep_alive_secs, 3600);
            }
            _ => panic!("Expected ContainerKeptAlive variant"),
        }
    }

    // --- EventContent::event_type ---

    #[test]
    fn event_content_type_mapping() {
        assert_eq!(
            EventContent::Reasoning(ReasoningContent {
                agent: "a".into(),
                text: "t".into(),
            })
            .event_type(),
            Some(EventType::Reasoning)
        );
        assert_eq!(
            EventContent::Unknown(UnknownContent {
                raw_event_type: "x".into(),
                raw: serde_json::Value::Null,
            })
            .event_type(),
            Some(EventType::Unknown)
        );
    }

    // --- Roundtrip serialization ---

    #[test]
    fn event_content_roundtrip() {
        let contents = vec![
            EventContent::Reasoning(ReasoningContent {
                agent: "research".into(),
                text: "thinking...".into(),
            }),
            EventContent::ToolCall(ToolCallContent {
                agent: "develop".into(),
                tool: Some("bash".into()),
                args: None,
                result: None,
            }),
            EventContent::AgentDispatch(AgentDispatchContent {
                agent: "explore".into(),
            }),
            EventContent::Error(ErrorContent {
                error: "timeout".into(),
                stage: None,
            }),
            EventContent::FileEdit(FileEditContent {
                path: "/src/main.rs".into(),
                extra: HashMap::new(),
            }),
            EventContent::PhaseChange(PhaseChangeContent {
                phase: "developing".into(),
            }),
            EventContent::ContainerKeptAlive(ContainerKeptAliveContent {
                expires_at: "2026-04-21T12:00:00Z".into(),
                keep_alive_secs: 3600,
            }),
            EventContent::Unknown(UnknownContent {
                raw_event_type: "future".into(),
                raw: serde_json::json!({"key": "val"}),
            }),
        ];

        for original in contents {
            let json = serde_json::to_value(&original).unwrap();
            let restored: EventContent = serde_json::from_value(json).unwrap();
            assert_eq!(original, restored);
        }
    }
}
