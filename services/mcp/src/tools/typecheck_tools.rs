//! MCP tools for type check queries.
//!
//! Tools for querying call_sites and trait_implementations tables.

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

// =============================================================================
// find_calls_with_type
// =============================================================================

/// Request for finding calls with a specific type argument
#[derive(Debug, Deserialize)]
pub struct FindCallsWithTypeRequest {
    /// Name of the type to search for in concrete_type_args
    pub type_name: String,
    /// Optional callee name filter (e.g., "parse" to find parse::<T>())
    pub callee_name: Option<String>,
    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// A call site with type information
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CallSiteInfo {
    pub caller_fqn: String,
    pub callee_fqn: String,
    pub file_path: String,
    pub line_number: u32,
    pub concrete_type_args: Vec<String>,
    pub is_monomorphized: bool,
    pub quality: String,
}

/// Response with call sites
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CallsWithTypeResponse {
    pub type_name: String,
    pub calls: Vec<CallSiteInfo>,
}

/// Execute the find_calls_with_type tool
#[instrument(skip(client))]
pub async fn execute_find_calls_with_type(
    client: &ApiClient,
    request: FindCallsWithTypeRequest,
) -> Result<String> {
    let encoded_type =
        url::form_urlencoded::byte_serialize(request.type_name.as_bytes()).collect::<String>();
    let mut url = format!(
        "/tools/find_calls_with_type?type_name={}&limit={}",
        encoded_type,
        request.limit.min(100)
    );

    if let Some(callee_name) = &request.callee_name {
        let encoded_callee =
            url::form_urlencoded::byte_serialize(callee_name.as_bytes()).collect::<String>();
        url.push_str(&format!("&callee_name={}", encoded_callee));
    }

    let response: CallsWithTypeResponse = client.get(&url).await?;

    if response.calls.is_empty() {
        let mut msg = format!(
            "No call sites found with type argument `{}`",
            response.type_name
        );
        if let Some(callee) = &request.callee_name {
            msg.push_str(&format!(" for callee matching `{}`", callee));
        }
        msg.push_str(". The type may not be used in any generic calls in the indexed codebase.");
        return Ok(msg);
    }

    let mut output = format!(
        "# Call Sites with Type `{}` ({})\n\n",
        response.type_name,
        response.calls.len()
    );

    // Group by callee
    let mut by_callee: std::collections::BTreeMap<String, Vec<&CallSiteInfo>> =
        std::collections::BTreeMap::new();

    for call in &response.calls {
        by_callee
            .entry(call.callee_fqn.clone())
            .or_default()
            .push(call);
    }

    for (callee_fqn, calls) in by_callee {
        output.push_str(&format!("## `{}` ({} calls)\n\n", callee_fqn, calls.len()));
        for call in calls {
            let type_args = call.concrete_type_args.join(", ");
            output.push_str(&format!(
                "- **`{}`** at `{}:{}`\n",
                call.caller_fqn, call.file_path, call.line_number
            ));
            output.push_str(&format!("  - Type args: `{}`\n", type_args));
            output.push_str(&format!(
                "  - Monomorphized: {}, Quality: {}\n\n",
                call.is_monomorphized, call.quality
            ));
        }
    }

    Ok(output)
}

/// Get the MCP tool definition for find_calls_with_type
pub fn definition_find_calls_with_type() -> serde_json::Value {
    serde_json::json!({
        "name": "find_calls_with_type",
        "description": "Find all call sites where a specific type is used as a type argument. Useful for finding concrete usages of generic functions like parse::<String>() or collect::<Vec<_>>().",
        "inputSchema": {
            "type": "object",
            "properties": {
                "type_name": {
                    "type": "string",
                    "description": "Name of the type to search for (e.g., 'String', 'Vec', 'i32'). Finds calls where this type appears in concrete type arguments."
                },
                "callee_name": {
                    "type": "string",
                    "description": "Optional name of the callee function to filter by (e.g., 'parse', 'collect'). Use to narrow down to specific generic functions."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 20, max: 100)",
                    "default": 20,
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["type_name"]
        }
    })
}

// =============================================================================
// find_trait_impls_for_type
// =============================================================================

/// Request for trait implementations for a type
#[derive(Debug, Deserialize)]
pub struct FindTraitImplsForTypeRequest {
    /// Name of the type to search for in self_type
    pub type_name: String,
    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// A trait implementation
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct TraitImplInfo {
    pub trait_fqn: String,
    pub self_type: String,
    pub impl_fqn: String,
    pub file_path: String,
    pub line_number: u32,
    pub generic_params: Vec<String>,
    pub quality: String,
}

/// Response with trait implementations
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct TraitImplsForTypeResponse {
    pub type_name: String,
    pub implementations: Vec<TraitImplInfo>,
}

/// Execute the find_trait_impls_for_type tool
#[instrument(skip(client))]
pub async fn execute_find_trait_impls_for_type(
    client: &ApiClient,
    request: FindTraitImplsForTypeRequest,
) -> Result<String> {
    let encoded_type =
        url::form_urlencoded::byte_serialize(request.type_name.as_bytes()).collect::<String>();
    let response: TraitImplsForTypeResponse = client
        .get(&format!(
            "/tools/find_trait_impls_for_type?type_name={}&limit={}",
            encoded_type,
            request.limit.min(100)
        ))
        .await?;

    if response.implementations.is_empty() {
        return Ok(format!(
            "No trait implementations found for type `{}`. The type may not exist or doesn't implement any indexed traits.",
            response.type_name
        ));
    }

    let mut output = format!(
        "# Trait Implementations for `{}` ({})\n\n",
        response.type_name,
        response.implementations.len()
    );

    for impl_info in &response.implementations {
        output.push_str(&format!(
            "- **`{}`** for `{}`\n",
            impl_info.trait_fqn, impl_info.self_type
        ));
        output.push_str(&format!("  - Impl FQN: `{}`\n", impl_info.impl_fqn));
        output.push_str(&format!(
            "  - Location: `{}:{}`\n",
            impl_info.file_path, impl_info.line_number
        ));
        if !impl_info.generic_params.is_empty() {
            output.push_str(&format!(
                "  - Generic params: `{}`\n",
                impl_info.generic_params.join(", ")
            ));
        }
        output.push_str(&format!("  - Quality: {}\n\n", impl_info.quality));
    }

    Ok(output)
}

/// Get the MCP tool definition for find_trait_impls_for_type
pub fn definition_find_trait_impls_for_type() -> serde_json::Value {
    serde_json::json!({
        "name": "find_trait_impls_for_type",
        "description": "Find all trait implementations for a specific type. Useful for understanding what traits a type implements, like finding all traits implemented by String or Vec.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "type_name": {
                    "type": "string",
                    "description": "Name of the type to search for (e.g., 'String', 'Vec', 'MyStruct'). Finds all trait implementations where this type is the implementing type."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of implementations to return (default: 20, max: 100)",
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
    fn test_find_calls_with_type_definition() {
        let def = definition_find_calls_with_type();
        assert_eq!(def["name"], "find_calls_with_type");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"]["properties"]["type_name"].is_object());
        assert!(def["inputSchema"]["properties"]["callee_name"].is_object());
    }

    #[test]
    fn test_find_trait_impls_for_type_definition() {
        let def = definition_find_trait_impls_for_type();
        assert_eq!(def["name"], "find_trait_impls_for_type");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"]["properties"]["type_name"].is_object());
    }

    #[test]
    fn test_find_calls_with_type_request_deserialization() {
        let json = r#"{"type_name": "String", "callee_name": "parse", "limit": 50}"#;
        let request: FindCallsWithTypeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.type_name, "String");
        assert_eq!(request.callee_name, Some("parse".to_string()));
        assert_eq!(request.limit, 50);
    }

    #[test]
    fn test_find_calls_with_type_request_optional_callee() {
        let json = r#"{"type_name": "Vec"}"#;
        let request: FindCallsWithTypeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.type_name, "Vec");
        assert_eq!(request.callee_name, None);
        assert_eq!(request.limit, 20);
    }

    #[test]
    fn test_find_trait_impls_for_type_request_deserialization() {
        let json = r#"{"type_name": "MyStruct", "limit": 30}"#;
        let request: FindTraitImplsForTypeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.type_name, "MyStruct");
        assert_eq!(request.limit, 30);
    }

    #[test]
    fn test_call_site_info_serialization() {
        let info = CallSiteInfo {
            caller_fqn: "crate::func".to_string(),
            callee_fqn: "core::str::parse".to_string(),
            file_path: "src/lib.rs".to_string(),
            line_number: 42,
            concrete_type_args: vec!["String".to_string()],
            is_monomorphized: true,
            quality: "analyzed".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("parse"));
        assert!(json.contains("String"));
    }

    #[test]
    fn test_trait_impl_info_serialization() {
        let info = TraitImplInfo {
            trait_fqn: "std::clone::Clone".to_string(),
            self_type: "MyStruct".to_string(),
            impl_fqn: "crate::MyStruct".to_string(),
            file_path: "src/lib.rs".to_string(),
            line_number: 10,
            generic_params: vec![],
            quality: "analyzed".to_string(),
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("Clone"));
        assert!(json.contains("MyStruct"));
    }
}
