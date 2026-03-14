//! MCP tool: search_code
//!
//! Semantic search over code using embeddings

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for semantic code search
#[derive(Debug, Deserialize)]
pub struct SearchCodeRequest {
    /// The search query (natural language or code)
    pub query: String,
    /// Maximum number of results (default: 10, max: 50)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Minimum similarity score threshold (0.0 to 1.0)
    #[serde(default)]
    pub score_threshold: Option<f32>,
}

fn default_limit() -> usize {
    10
}

/// A single search result
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SearchResult {
    /// Fully qualified name of the item
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Type of item (function, struct, enum, etc.)
    pub kind: String,
    /// File path
    pub file_path: String,
    /// Start line number
    pub start_line: u32,
    /// End line number
    pub end_line: u32,
    /// Similarity score
    pub score: f32,
    /// Code snippet (if available)
    pub snippet: Option<String>,
    /// Documentation comment (if available)
    pub docstring: Option<String>,
}

/// Response from semantic search
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SearchCodeResponse {
    /// Search results
    pub results: Vec<SearchResult>,
    /// Original query
    pub query: String,
    /// Total number of results
    pub total: usize,
}

/// Execute the search_code tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: SearchCodeRequest) -> Result<String> {
    let api_request = serde_json::json!({
        "query": request.query,
        "limit": request.limit.min(50),
        "score_threshold": request.score_threshold,
    });

    let response: SearchCodeResponse = client
        .post("/tools/search_semantic", &api_request)
        .await?;

    if response.results.is_empty() {
        return Ok("No results found for your query. Try using different keywords or lowering the score threshold.".to_string());
    }

    let mut output = format!(
        "Found {} results for '{}' (showing top {}):\n\n",
        response.total,
        response.query,
        response.results.len()
    );

    for (i, result) in response.results.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {} (score: {:.2})\n",
            i + 1,
            result.fqn,
            result.score
        ));
        output.push_str(&format!(
            "   **Type:** {} | **File:** {}:{}-{}\n",
            result.kind, result.file_path, result.start_line, result.end_line
        ));

        if let Some(ref docstring) = result.docstring {
            let doc_preview = if docstring.len() > 200 {
                format!("{}...", &docstring[..200])
            } else {
                docstring.clone()
            };
            output.push_str(&format!("   **Doc:** {}\n", doc_preview));
        }

        if let Some(ref snippet) = result.snippet {
            output.push_str(&format!("   ```rust\n   {}\n   ```\n", snippet));
        }

        output.push('\n');
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "search_code",
        "description": "Search for code using semantic similarity. Finds functions, structs, enums, and other items that match your query conceptually, not just by exact text match. Use natural language or code fragments to search.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Can be natural language (e.g., 'function that parses JSON') or code fragments (e.g., 'fn parse')."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 50)",
                    "default": 10,
                    "minimum": 1,
                    "maximum": 50
                },
                "score_threshold": {
                    "type": "number",
                    "description": "Minimum similarity score (0.0 to 1.0). Higher values return more relevant results.",
                    "minimum": 0.0,
                    "maximum": 1.0
                }
            },
            "required": ["query"]
        }
    })
}
