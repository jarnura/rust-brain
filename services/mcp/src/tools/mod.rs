//! MCP Tools for rust-brain
//!
//! This module contains all the MCP tools that wrap the rust-brain API.

pub mod find_type_usages;
pub mod get_callers;
pub mod get_function;
pub mod get_module_tree;
pub mod get_trait_impls;
pub mod query_graph;
pub mod search_code;
pub mod typecheck_tools;

use crate::client::ApiClient;
use crate::error::Result;
use serde_json::Value;
use tracing::{debug, info, instrument, warn};

/// Get all tool definitions
pub fn all_definitions() -> Vec<Value> {
    let definitions = vec![
        search_code::definition(),
        get_function::definition(),
        get_callers::definition(),
        get_trait_impls::definition(),
        find_type_usages::definition(),
        get_module_tree::definition(),
        query_graph::definition(),
        typecheck_tools::definition_find_calls_with_type(),
        typecheck_tools::definition_find_trait_impls_for_type(),
    ];
    debug!(count = definitions.len(), "Returning tool definitions");
    definitions
}

/// Execute a tool by name
#[instrument(skip(client), fields(tool = %name))]
pub async fn execute_tool(
    client: &ApiClient,
    name: &str,
    arguments: Value,
) -> Result<String> {
    info!("Executing tool: {}", name);
    match name {
        "search_code" => {
            let request: search_code::SearchCodeRequest = serde_json::from_value(arguments)?;
            search_code::execute(client, request).await
        }
        "get_function" => {
            let request: get_function::GetFunctionRequest = serde_json::from_value(arguments)?;
            get_function::execute(client, request).await
        }
        "get_callers" => {
            let request: get_callers::GetCallersRequest = serde_json::from_value(arguments)?;
            get_callers::execute(client, request).await
        }
        "get_trait_impls" => {
            let request: get_trait_impls::GetTraitImplsRequest = serde_json::from_value(arguments)?;
            get_trait_impls::execute(client, request).await
        }
        "find_type_usages" => {
            let request: find_type_usages::FindTypeUsagesRequest = serde_json::from_value(arguments)?;
            find_type_usages::execute(client, request).await
        }
        "get_module_tree" => {
            let request: get_module_tree::GetModuleTreeRequest = serde_json::from_value(arguments)?;
            get_module_tree::execute(client, request).await
        }
        "query_graph" => {
            let request: query_graph::QueryGraphRequest = serde_json::from_value(arguments)?;
            query_graph::execute(client, request).await
        }
        "find_calls_with_type" => {
            let request: typecheck_tools::FindCallsWithTypeRequest = serde_json::from_value(arguments)?;
            typecheck_tools::execute_find_calls_with_type(client, request).await
        }
        "find_trait_impls_for_type" => {
            let request: typecheck_tools::FindTraitImplsForTypeRequest = serde_json::from_value(arguments)?;
            typecheck_tools::execute_find_trait_impls_for_type(client, request).await
        }
        unknown => {
            warn!(tool = %unknown, "Unknown tool requested");
            Err(crate::error::McpError::InvalidRequest(format!(
                "Unknown tool: {}",
                unknown
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_definitions_count() {
        let definitions = all_definitions();
        assert_eq!(definitions.len(), 9);
    }

    #[test]
    fn test_all_definitions_have_name() {
        let definitions = all_definitions();
        
        for def in &definitions {
            assert!(def.get("name").is_some());
            assert!(!def["name"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn test_all_definitions_have_description() {
        let definitions = all_definitions();
        
        for def in &definitions {
            assert!(def.get("description").is_some());
            assert!(!def["description"].as_str().unwrap().is_empty());
        }
    }

    #[test]
    fn test_all_definitions_have_input_schema() {
        let definitions = all_definitions();
        
        for def in &definitions {
            assert!(def.get("inputSchema").is_some());
            assert!(def["inputSchema"].is_object());
        }
    }

    #[test]
    fn test_definition_names_are_correct() {
        let definitions = all_definitions();
        let names: Vec<&str> = definitions
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        
        assert!(names.contains(&"search_code"));
        assert!(names.contains(&"get_function"));
        assert!(names.contains(&"get_callers"));
        assert!(names.contains(&"get_trait_impls"));
        assert!(names.contains(&"find_type_usages"));
        assert!(names.contains(&"get_module_tree"));
        assert!(names.contains(&"query_graph"));
        assert!(names.contains(&"find_calls_with_type"));
        assert!(names.contains(&"find_trait_impls_for_type"));
    }

    #[test]
    fn test_all_definitions_have_required_fields() {
        let definitions = all_definitions();
        
        for def in &definitions {
            let schema = &def["inputSchema"];
            assert!(schema.get("type").is_some());
            assert!(schema.get("properties").is_some());
        }
    }

    // Test that unknown tool returns error
    #[test]
    fn test_unknown_tool_error_message() {
        let err = crate::error::McpError::InvalidRequest("Unknown tool: unknown_tool".to_string());
        assert!(err.to_string().contains("Unknown tool"));
        assert!(err.to_string().contains("unknown_tool"));
    }
}
