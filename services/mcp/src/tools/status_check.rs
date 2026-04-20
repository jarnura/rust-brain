//! MCP tool: status_check
//!
//! Check task status. Query by task_id, status, or agent.

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for status_check
#[derive(Debug, Deserialize)]
pub struct StatusCheckRequest {
    /// Specific task ID to check
    pub task_id: Option<String>,
    /// Filter by status
    pub status: Option<String>,
    /// Filter by agent
    pub agent: Option<String>,
}

/// Execute the status_check tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: StatusCheckRequest) -> Result<String> {
    if let Some(task_id) = &request.task_id {
        // Get specific task
        let result: serde_json::Value = client.get(&format!("/api/tasks/{}", task_id)).await?;
        Ok(format!(
            "# Task: {}\n\n- **Phase:** {}\n- **Class:** {}\n- **Agent:** {}\n- **Status:** {}\n- **Retry Count:** {}\n- **Error:** {}\n- **Updated:** {}",
            result["id"].as_str().unwrap_or("?"),
            result["phase"].as_str().unwrap_or("?"),
            result["class"].as_str().unwrap_or("?"),
            result["agent"].as_str().unwrap_or("?"),
            result["status"].as_str().unwrap_or("?"),
            result["retry_count"],
            result["error"].as_str().unwrap_or("none"),
            result["updated_at"].as_str().unwrap_or("?"),
        ))
    } else {
        // List tasks with filters
        let mut url = "/api/tasks?".to_string();
        let mut params = vec![];
        if let Some(status) = &request.status {
            params.push(format!("status={}", status));
        }
        if let Some(agent) = &request.agent {
            params.push(format!("agent={}", agent));
        }
        url.push_str(&params.join("&"));

        let tasks: Vec<serde_json::Value> = client.get(&url).await?;

        let mut output = String::from("# Task Status\n\n");

        if tasks.is_empty() {
            output.push_str("No tasks found.\n");
            return Ok(output);
        }

        output.push_str(&format!("**{} task(s)**\n\n", tasks.len()));
        output.push_str("| ID | Phase | Agent | Status | Updated |\n");
        output.push_str("| --- | --- | --- | --- | --- |\n");

        for task in &tasks {
            output.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                task["id"].as_str().unwrap_or("?"),
                task["phase"].as_str().unwrap_or("?"),
                task["agent"].as_str().unwrap_or("?"),
                task["status"].as_str().unwrap_or("?"),
                task["updated_at"].as_str().unwrap_or("?"),
            ));
        }

        Ok(output)
    }
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "status_check",
        "description": "Check task status. Query by task_id, status, or agent.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Specific task ID to check"
                },
                "status": {
                    "type": "string",
                    "description": "Filter tasks by status (pending, dispatched, in_progress, review, completed, rejected, blocked, escalated)"
                },
                "agent": {
                    "type": "string",
                    "description": "Filter tasks by assigned agent"
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        assert_eq!(def["name"], "status_check");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["task_id"].is_object());
        assert!(schema["properties"]["status"].is_object());
        assert!(schema["properties"]["agent"].is_object());
    }

    #[test]
    fn test_status_check_request_deserialization_by_id() {
        let json = r#"{"task_id": "task-001"}"#;
        let request: StatusCheckRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.task_id, Some("task-001".to_string()));
        assert!(request.status.is_none());
        assert!(request.agent.is_none());
    }

    #[test]
    fn test_status_check_request_deserialization_by_status() {
        let json = r#"{"status": "pending"}"#;
        let request: StatusCheckRequest = serde_json::from_str(json).unwrap();
        assert!(request.task_id.is_none());
        assert_eq!(request.status, Some("pending".to_string()));
    }

    #[test]
    fn test_status_check_request_deserialization_empty() {
        let json = r#"{}"#;
        let request: StatusCheckRequest = serde_json::from_str(json).unwrap();
        assert!(request.task_id.is_none());
        assert!(request.status.is_none());
        assert!(request.agent.is_none());
    }
}
