//! MCP tool: get_callers
//!
//! Get all functions that call a given function, with configurable depth

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for callers
#[derive(Debug, Deserialize)]
pub struct GetCallersRequest {
    /// Fully qualified name of the function
    pub fqn: String,
    /// Depth of the call graph to explore (default: 1)
    #[serde(default = "default_depth")]
    pub depth: usize,
}

fn default_depth() -> usize {
    1
}

/// A node in the call graph
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CallerNode {
    /// Fully qualified name
    pub fqn: String,
    /// Short name
    pub name: String,
    /// File path
    pub file_path: String,
    /// Line number
    pub line: u32,
    /// Depth in the call graph
    pub depth: usize,
}

/// Response with callers
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CallersResponse {
    /// The function that was queried
    pub fqn: String,
    /// List of callers at various depths
    pub callers: Vec<CallerNode>,
    /// Maximum depth explored
    pub depth: usize,
}

/// Execute the get_callers tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: GetCallersRequest) -> Result<String> {
    let depth = request.depth.min(5).max(1);
    let encoded_fqn = url::form_urlencoded::byte_serialize(request.fqn.as_bytes()).collect::<String>();
    
    let response: CallersResponse = client
        .get(&format!(
            "/tools/get_callers?fqn={}&depth={}",
            encoded_fqn,
            depth
        ))
        .await?;

    if response.callers.is_empty() {
        return Ok(format!(
            "No callers found for `{}`. This function may not be called anywhere, or may be an entry point.",
            response.fqn
        ));
    }

    let mut output = format!(
        "# Callers of `{}` (depth: {})\n\n",
        response.fqn, response.depth
    );

    // Group by depth
    let max_depth = response.callers.iter().map(|c| c.depth).max().unwrap_or(1);

    for d in 1..=max_depth {
        let callers_at_depth: Vec<_> = response
            .callers
            .iter()
            .filter(|c| c.depth == d)
            .collect();

        if !callers_at_depth.is_empty() {
            output.push_str(&format!("## Depth {} ({})\n\n", d, callers_at_depth.len()));
            for caller in callers_at_depth {
                output.push_str(&format!(
                    "- `{}` at `{}:{}`\n",
                    caller.fqn, caller.file_path, caller.line
                ));
            }
            output.push('\n');
        }
    }

    output.push_str(&format!(
        "\n**Total callers:** {}\n",
        response.callers.len()
    ));

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "get_callers",
        "description": "Find all functions that call a given function. Use this to understand the impact of changes or trace execution paths through the codebase. Supports call graph traversal up to 5 levels deep.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "fqn": {
                    "type": "string",
                    "description": "Fully qualified name of the function to analyze"
                },
                "depth": {
                    "type": "integer",
                    "description": "How many levels of the call graph to explore (default: 1, max: 5)",
                    "default": 1,
                    "minimum": 1,
                    "maximum": 5
                }
            },
            "required": ["fqn"]
        }
    })
}
