//! MCP tool: find_type_usages
//!
//! Find all usages of a type in the codebase

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for type usages
#[derive(Debug, Deserialize)]
pub struct FindTypeUsagesRequest {
    /// Name of the type (e.g., "User", "HttpRequest")
    pub type_name: String,
    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// A type usage location
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct TypeUsage {
    /// Fully qualified name of the using item
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Type of item (function, struct, etc.)
    pub kind: String,
    /// File path
    pub file_path: String,
    /// Line number
    pub line: u32,
}

/// Response with type usages
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct UsagesResponse {
    /// The type that was queried
    pub type_name: String,
    /// List of usages
    pub usages: Vec<TypeUsage>,
}

/// Execute the find_type_usages tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: FindTypeUsagesRequest) -> Result<String> {
    let encoded_type = url::form_urlencoded::byte_serialize(request.type_name.as_bytes()).collect::<String>();
    let response: UsagesResponse = client
        .get(&format!(
            "/tools/find_usages_of_type?type_name={}&limit={}",
            encoded_type,
            request.limit.min(100)
        ))
        .await?;

    if response.usages.is_empty() {
        return Ok(format!(
            "No usages found for type `{}`. The type may not exist or is not used in the indexed codebase.",
            response.type_name
        ));
    }

    let mut output = format!(
        "# Usages of `{}` ({})\n\n",
        response.type_name,
        response.usages.len()
    );

    // Group by kind
    let mut by_kind: std::collections::BTreeMap<String, Vec<&TypeUsage>> =
        std::collections::BTreeMap::new();

    for usage in &response.usages {
        by_kind
            .entry(usage.kind.clone())
            .or_default()
            .push(usage);
    }

    for (kind, usages) in by_kind {
        output.push_str(&format!("## {} ({} usages)\n\n", kind, usages.len()));
        for usage in usages {
            output.push_str(&format!(
                "- `{}` at `{}:{}`\n",
                usage.fqn, usage.file_path, usage.line
            ));
        }
        output.push('\n');
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "find_type_usages",
        "description": "Find all places where a type is used in the codebase. This includes function parameters, return types, struct fields, and trait bounds. Useful for understanding the reach of a type change.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "type_name": {
                    "type": "string",
                    "description": "Name of the type to search for (e.g., 'User', 'HttpRequest'). Can be just the type name without the full path."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of usages to return (default: 20, max: 100)",
                    "default": 20,
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["type_name"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        
        assert_eq!(def["name"], "find_type_usages");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["type_name"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("type_name")));
    }

    #[test]
    fn test_find_type_usages_request_deserialization() {
        let json = r#"{"type_name": "User", "limit": 30}"#;
        let request: FindTypeUsagesRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.type_name, "User");
        assert_eq!(request.limit, 30);
    }

    #[test]
    fn test_find_type_usages_request_default_limit() {
        let json = r#"{"type_name": "HttpRequest"}"#;
        let request: FindTypeUsagesRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.type_name, "HttpRequest");
        assert_eq!(request.limit, 20); // default
    }

    #[test]
    fn test_default_limit_value() {
        assert_eq!(default_limit(), 20);
    }

    #[test]
    fn test_type_usage_deserialization() {
        let json = r#"{
            "fqn": "crate::module::function",
            "name": "function",
            "kind": "function",
            "file_path": "src/module.rs",
            "line": 42
        }"#;
        
        let usage: TypeUsage = serde_json::from_str(json).unwrap();
        
        assert_eq!(usage.fqn, "crate::module::function");
        assert_eq!(usage.name, "function");
        assert_eq!(usage.kind, "function");
        assert_eq!(usage.file_path, "src/module.rs");
        assert_eq!(usage.line, 42);
    }

    #[test]
    fn test_usages_response_deserialization() {
        let json = r#"{
            "type_name": "User",
            "usages": [
                {
                    "fqn": "crate::api::get_user",
                    "name": "get_user",
                    "kind": "function",
                    "file_path": "src/api.rs",
                    "line": 10
                },
                {
                    "fqn": "crate::models::User",
                    "name": "User",
                    "kind": "struct",
                    "file_path": "src/models.rs",
                    "line": 5
                }
            ]
        }"#;
        
        let response: UsagesResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.type_name, "User");
        assert_eq!(response.usages.len(), 2);
        assert_eq!(response.usages[0].kind, "function");
        assert_eq!(response.usages[1].kind, "struct");
    }

    #[test]
    fn test_usages_response_empty() {
        let json = r#"{
            "type_name": "NonExistentType",
            "usages": []
        }"#;
        
        let response: UsagesResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.type_name, "NonExistentType");
        assert!(response.usages.is_empty());
    }

    #[test]
    fn test_type_usage_serialization() {
        let usage = TypeUsage {
            fqn: "crate::func".to_string(),
            name: "func".to_string(),
            kind: "function".to_string(),
            file_path: "src/lib.rs".to_string(),
            line: 10,
        };
        
        let json = serde_json::to_string(&usage).unwrap();
        assert!(json.contains("\"fqn\":\"crate::func\""));
        assert!(json.contains("\"kind\":\"function\""));
        assert!(json.contains("\"line\":10"));
    }

    #[test]
    fn test_usages_response_serialization() {
        let response = UsagesResponse {
            type_name: "User".to_string(),
            usages: vec![TypeUsage {
                fqn: "crate::func".to_string(),
                name: "func".to_string(),
                kind: "function".to_string(),
                file_path: "src/lib.rs".to_string(),
                line: 5,
            }],
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"type_name\":\"User\""));
        assert!(json.contains("\"usages\""));
    }
}
