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
