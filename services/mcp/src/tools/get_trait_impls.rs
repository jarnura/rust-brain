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
    let encoded_trait = url::form_urlencoded::byte_serialize(request.trait_name.as_bytes()).collect::<String>();
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
        output.push_str(&format!(
            "- **`{}`**\n",
            impl_info.type_name
        ));
        output.push_str(&format!(
            "  - FQN: `{}`\n",
            impl_info.impl_fqn
        ));
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
