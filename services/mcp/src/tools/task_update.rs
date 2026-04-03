//! MCP tool: task_update
//!
//! Create or update a task. Used by the Orchestrator to manage task lifecycle.

use crate::client::ApiClient;
use crate::error::{McpError, Result};
use serde::Deserialize;
use tracing::instrument;

/// Request for task_update
#[derive(Debug, Deserialize)]
pub struct TaskUpdateRequest {
    /// Operation: create or update
    pub op: String,
    /// Task ID (for update)
    pub task_id: Option<String>,
    /// Full task body (for create)
    pub task: Option<serde_json::Value>,
    /// New status (for update)
    pub status: Option<String>,
    /// Error message (optional, for update)
    pub error: Option<String>,
}

/// Execute the task_update tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: TaskUpdateRequest) -> Result<String> {
    match request.op.as_str() {
        "create" => {
            let task = request.task.ok_or_else(|| {
                McpError::InvalidRequest("'task' field required for create operation".to_string())
            })?;
            let result: serde_json::Value = client.post("/api/tasks", &task).await?;
            Ok(format!(
                "**Task created**\n\n- ID: {}\n- Phase: {}\n- Agent: {}\n- Status: {}",
                result["id"].as_str().unwrap_or("?"),
                result["phase"].as_str().unwrap_or("?"),
                result["agent"].as_str().unwrap_or("?"),
                result["status"].as_str().unwrap_or("?"),
            ))
        }
        "update" => {
            let task_id = request.task_id.ok_or_else(|| {
                McpError::InvalidRequest("'task_id' required for update operation".to_string())
            })?;

            let mut body = serde_json::Map::new();
            if let Some(status) = &request.status {
                body.insert("status".to_string(), serde_json::json!(status));
            }
            if let Some(error) = &request.error {
                body.insert("error".to_string(), serde_json::json!(error));
            }

            let result: serde_json::Value = client
                .put(
                    &format!("/api/tasks/{}", task_id),
                    &serde_json::Value::Object(body),
                )
                .await?;
            Ok(format!(
                "**Task updated**\n\n- ID: {}\n- Status: {}\n- Retry Count: {}\n- Error: {}",
                result["id"].as_str().unwrap_or("?"),
                result["status"].as_str().unwrap_or("?"),
                result["retry_count"],
                result["error"].as_str().unwrap_or("none"),
            ))
        }
        unknown => Err(McpError::InvalidRequest(format!(
            "Unknown operation: '{}'. Must be 'create' or 'update'",
            unknown
        ))),
    }
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "task_update",
        "description": "Create or update a task. Used by the Orchestrator to manage task lifecycle.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["create", "update"],
                    "description": "Operation to perform"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for update)"
                },
                "task": {
                    "type": "object",
                    "description": "Full task body (required for create)"
                },
                "status": {
                    "type": "string",
                    "description": "New status (for update)"
                },
                "error": {
                    "type": "string",
                    "description": "Error message (optional, for update)"
                }
            },
            "required": ["op"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        assert_eq!(def["name"], "task_update");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["op"].is_object());
        assert!(schema["properties"]["task_id"].is_object());
        assert!(schema["properties"]["task"].is_object());
        assert!(schema["properties"]["status"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("op")));
    }

    #[test]
    fn test_task_update_request_deserialization_create() {
        let json = r#"{"op": "create", "task": {"id": "task-001", "phase": "build", "class": "B", "agent": "dev"}}"#;
        let request: TaskUpdateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.op, "create");
        assert!(request.task.is_some());
    }

    #[test]
    fn test_task_update_request_deserialization_update() {
        let json = r#"{"op": "update", "task_id": "task-001", "status": "in_progress"}"#;
        let request: TaskUpdateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.op, "update");
        assert_eq!(request.task_id, Some("task-001".to_string()));
        assert_eq!(request.status, Some("in_progress".to_string()));
    }

    #[test]
    fn test_task_update_request_with_error() {
        let json = r#"{"op": "update", "task_id": "task-001", "status": "blocked", "error": "dependency not met"}"#;
        let request: TaskUpdateRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.error, Some("dependency not met".to_string()));
    }
}
