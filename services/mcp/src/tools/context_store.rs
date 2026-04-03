//! MCP tool: context_store
//!
//! CRUD operations for the artifact store. Used for inter-agent communication.

use crate::client::ApiClient;
use crate::error::{McpError, Result};
use serde::Deserialize;
use tracing::instrument;

/// Request for context_store operations
#[derive(Debug, Deserialize)]
pub struct ContextStoreRequest {
    /// Operation: put, get, list_by_task, list_by_type
    pub op: String,
    /// Artifact ID (for get)
    pub artifact_id: Option<String>,
    /// Task ID (for list_by_task / put)
    pub task_id: Option<String>,
    /// Artifact type (for list_by_type)
    #[serde(rename = "type")]
    pub artifact_type: Option<String>,
    /// Status filter (for list_by_type)
    pub status: Option<String>,
    /// Limit for list operations
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Full artifact body (for put)
    pub artifact: Option<serde_json::Value>,
}

fn default_limit() -> u32 {
    10
}

/// Execute the context_store tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: ContextStoreRequest) -> Result<String> {
    match request.op.as_str() {
        "put" => {
            let artifact = request.artifact.ok_or_else(|| {
                McpError::InvalidRequest("'artifact' field required for put operation".to_string())
            })?;
            let result: serde_json::Value = client.post("/api/artifacts", &artifact).await?;
            Ok(format!(
                "**Artifact created**\n\n- ID: {}\n- Type: {}\n- Status: {}",
                result["id"].as_str().unwrap_or("unknown"),
                result["type"].as_str().unwrap_or("unknown"),
                result["status"].as_str().unwrap_or("unknown"),
            ))
        }
        "get" => {
            let id = request.artifact_id.ok_or_else(|| {
                McpError::InvalidRequest("'artifact_id' required for get operation".to_string())
            })?;
            let result: serde_json::Value =
                client.get(&format!("/api/artifacts/{}", id)).await?;
            Ok(format!(
                "# Artifact: {}\n\n- **Type:** {}\n- **Producer:** {}\n- **Status:** {}\n- **Task:** {}\n- **Confidence:** {}\n\n## Summary\n\n```json\n{}\n```\n\n## Payload\n\n```json\n{}\n```",
                result["id"].as_str().unwrap_or("unknown"),
                result["type"].as_str().unwrap_or("unknown"),
                result["producer"].as_str().unwrap_or("unknown"),
                result["status"].as_str().unwrap_or("unknown"),
                result["task_id"].as_str().unwrap_or("unknown"),
                result["confidence"],
                serde_json::to_string_pretty(&result["summary"]).unwrap_or_default(),
                serde_json::to_string_pretty(&result["payload"]).unwrap_or_default(),
            ))
        }
        "list_by_task" => {
            let task_id = request.task_id.ok_or_else(|| {
                McpError::InvalidRequest("'task_id' required for list_by_task operation".to_string())
            })?;
            let result: Vec<serde_json::Value> = client
                .get(&format!(
                    "/api/artifacts?task_id={}&limit={}",
                    task_id, request.limit
                ))
                .await?;
            format_artifact_list(&result, &format!("Artifacts for task {}", task_id))
        }
        "list_by_type" => {
            let artifact_type = request.artifact_type.ok_or_else(|| {
                McpError::InvalidRequest("'type' required for list_by_type operation".to_string())
            })?;
            let mut url = format!(
                "/api/artifacts?type={}&limit={}",
                artifact_type, request.limit
            );
            if let Some(status) = &request.status {
                url.push_str(&format!("&status={}", status));
            }
            let result: Vec<serde_json::Value> = client.get(&url).await?;
            format_artifact_list(&result, &format!("Artifacts of type '{}'", artifact_type))
        }
        unknown => Err(McpError::InvalidRequest(format!(
            "Unknown operation: '{}'. Must be one of: put, get, list_by_task, list_by_type",
            unknown
        ))),
    }
}

fn format_artifact_list(artifacts: &[serde_json::Value], title: &str) -> Result<String> {
    let mut output = format!("# {}\n\n", title);

    if artifacts.is_empty() {
        output.push_str("No artifacts found.\n");
        return Ok(output);
    }

    output.push_str(&format!("**{} artifact(s)**\n\n", artifacts.len()));
    output.push_str("| ID | Type | Producer | Status | Confidence |\n");
    output.push_str("| --- | --- | --- | --- | --- |\n");

    for a in artifacts {
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            a["id"].as_str().unwrap_or("?"),
            a["type"].as_str().unwrap_or("?"),
            a["producer"].as_str().unwrap_or("?"),
            a["status"].as_str().unwrap_or("?"),
            a["confidence"],
        ));
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "context_store",
        "description": "CRUD operations for the artifact store. Used for inter-agent communication.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["put", "get", "list_by_task", "list_by_type"],
                    "description": "Operation to perform"
                },
                "artifact_id": {
                    "type": "string",
                    "description": "Artifact ID (required for get)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (required for list_by_task and put)"
                },
                "type": {
                    "type": "string",
                    "description": "Artifact type (required for list_by_type)"
                },
                "status": {
                    "type": "string",
                    "description": "Status filter (optional for list_by_type)"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum results for list operations (default 10)",
                    "default": 10
                },
                "artifact": {
                    "type": "object",
                    "description": "Full artifact body (required for put)"
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
        assert_eq!(def["name"], "context_store");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["op"].is_object());
        assert!(schema["properties"]["artifact_id"].is_object());
        assert!(schema["properties"]["task_id"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("op")));
    }

    #[test]
    fn test_context_store_request_deserialization_get() {
        let json = r#"{"op": "get", "artifact_id": "art-001"}"#;
        let request: ContextStoreRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.op, "get");
        assert_eq!(request.artifact_id, Some("art-001".to_string()));
        assert_eq!(request.limit, 10);
    }

    #[test]
    fn test_context_store_request_deserialization_put() {
        let json = r#"{"op": "put", "artifact": {"id": "art-001", "task_id": "t1", "type": "prd", "producer": "p", "summary": {}, "payload": {}}}"#;
        let request: ContextStoreRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.op, "put");
        assert!(request.artifact.is_some());
    }

    #[test]
    fn test_format_artifact_list_empty() {
        let result = format_artifact_list(&[], "Test").unwrap();
        assert!(result.contains("No artifacts found"));
    }

    #[test]
    fn test_format_artifact_list_with_items() {
        let items = vec![serde_json::json!({
            "id": "art-001",
            "type": "prd",
            "producer": "planner",
            "status": "draft",
            "confidence": 0.9
        })];
        let result = format_artifact_list(&items, "Test").unwrap();
        assert!(result.contains("art-001"));
        assert!(result.contains("prd"));
    }
}
