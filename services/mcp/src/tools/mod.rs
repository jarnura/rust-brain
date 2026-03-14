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

use crate::client::ApiClient;
use crate::error::Result;
use serde_json::Value;

/// Get all tool definitions
pub fn all_definitions() -> Vec<Value> {
    vec![
        search_code::definition(),
        get_function::definition(),
        get_callers::definition(),
        get_trait_impls::definition(),
        find_type_usages::definition(),
        get_module_tree::definition(),
        query_graph::definition(),
    ]
}

/// Execute a tool by name
pub async fn execute_tool(
    client: &ApiClient,
    name: &str,
    arguments: Value,
) -> Result<String> {
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
        _ => Err(crate::error::McpError::InvalidRequest(format!(
            "Unknown tool: {}",
            name
        ))),
    }
}
