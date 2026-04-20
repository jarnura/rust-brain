//! MCP tool: get_trait_impls
//!
//! Find all implementations of a trait

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for trait implementations
#[derive(Debug, Deserialize)]
pub struct GetTraitImplsRequest {
    /// Name of the trait (e.g., "Clone", "Serialize")
    pub trait_name: String,
    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// A trait implementation
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct TraitImpl {
    /// Fully qualified name of the implementation
    pub impl_fqn: String,
    /// Name of the implementing type
    pub type_name: String,
    /// File path
    pub file_path: String,
    /// Start line
    pub start_line: u32,
}

/// Response with trait implementations
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct TraitImplsResponse {
    /// The trait that was queried
    pub trait_name: String,
    /// List of implementations
    pub implementations: Vec<TraitImpl>,
}

/// Execute the get_trait_impls tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: GetTraitImplsRequest) -> Result<String> {
    let encoded_trait =
        url::form_urlencoded::byte_serialize(request.trait_name.as_bytes()).collect::<String>();
    let response: TraitImplsResponse = client
        .get(&format!(
            "/tools/get_trait_impls?trait_name={}&limit={}",
            encoded_trait,
            request.limit.min(100)
        ))
        .await?;

    if response.implementations.is_empty() {
        return Ok(format!(
            "No implementations found for trait `{}`. The trait may not exist or has no implementors in the indexed codebase.",
            response.trait_name
        ));
    }

    let mut output = format!(
        "# Implementations of `{}` ({})\n\n",
        response.trait_name,
        response.implementations.len()
    );

    for impl_info in &response.implementations {
        output.push_str(&format!("- **`{}`**\n", impl_info.type_name));
        output.push_str(&format!("  - FQN: `{}`\n", impl_info.impl_fqn));
        output.push_str(&format!(
            "  - Location: `{}:{}`\n\n",
            impl_info.file_path, impl_info.start_line
        ));
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "get_trait_impls",
        "description": "Find all types that implement a given trait. Useful for understanding polymorphism, finding all handlers for a trait, or exploring the design patterns in a codebase.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "trait_name": {
                    "type": "string",
                    "description": "Name of the trait (e.g., 'Clone', 'Serialize', 'Handler'). Can be just the trait name without the full path."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of implementations to return (default: 20, max: 100)",
                    "default": 20,
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["trait_name"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();

        assert_eq!(def["name"], "get_trait_impls");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["trait_name"].is_object());
        assert!(schema["properties"]["limit"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("trait_name")));
    }

    #[test]
    fn test_get_trait_impls_request_deserialization() {
        let json = r#"{"trait_name": "Clone", "limit": 50}"#;
        let request: GetTraitImplsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.trait_name, "Clone");
        assert_eq!(request.limit, 50);
    }

    #[test]
    fn test_get_trait_impls_request_default_limit() {
        let json = r#"{"trait_name": "Serialize"}"#;
        let request: GetTraitImplsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.trait_name, "Serialize");
        assert_eq!(request.limit, 20); // default
    }

    #[test]
    fn test_default_limit_value() {
        assert_eq!(default_limit(), 20);
    }

    #[test]
    fn test_trait_impl_deserialization() {
        let json = r#"{
            "impl_fqn": "crate::MyStruct",
            "type_name": "MyStruct",
            "file_path": "src/my_struct.rs",
            "start_line": 10
        }"#;

        let impl_info: TraitImpl = serde_json::from_str(json).unwrap();

        assert_eq!(impl_info.impl_fqn, "crate::MyStruct");
        assert_eq!(impl_info.type_name, "MyStruct");
        assert_eq!(impl_info.file_path, "src/my_struct.rs");
        assert_eq!(impl_info.start_line, 10);
    }

    #[test]
    fn test_trait_impls_response_deserialization() {
        let json = r#"{
            "trait_name": "Clone",
            "implementations": [
                {
                    "impl_fqn": "crate::MyStruct",
                    "type_name": "MyStruct",
                    "file_path": "src/my_struct.rs",
                    "start_line": 10
                },
                {
                    "impl_fqn": "crate::OtherStruct",
                    "type_name": "OtherStruct",
                    "file_path": "src/other.rs",
                    "start_line": 20
                }
            ]
        }"#;

        let response: TraitImplsResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.trait_name, "Clone");
        assert_eq!(response.implementations.len(), 2);
        assert_eq!(response.implementations[0].type_name, "MyStruct");
        assert_eq!(response.implementations[1].type_name, "OtherStruct");
    }

    #[test]
    fn test_trait_impls_response_empty() {
        let json = r#"{
            "trait_name": "NonExistentTrait",
            "implementations": []
        }"#;

        let response: TraitImplsResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.trait_name, "NonExistentTrait");
        assert!(response.implementations.is_empty());
    }

    #[test]
    fn test_trait_impl_serialization() {
        let impl_info = TraitImpl {
            impl_fqn: "crate::MyStruct".to_string(),
            type_name: "MyStruct".to_string(),
            file_path: "src/lib.rs".to_string(),
            start_line: 5,
        };

        let json = serde_json::to_string(&impl_info).unwrap();
        assert!(json.contains("\"impl_fqn\":\"crate::MyStruct\""));
        assert!(json.contains("\"start_line\":5"));
    }

    #[test]
    fn test_trait_impls_response_serialization() {
        let response = TraitImplsResponse {
            trait_name: "Clone".to_string(),
            implementations: vec![TraitImpl {
                impl_fqn: "crate::MyStruct".to_string(),
                type_name: "MyStruct".to_string(),
                file_path: "src/lib.rs".to_string(),
                start_line: 5,
            }],
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"trait_name\":\"Clone\""));
        assert!(json.contains("\"implementations\""));
    }
}
