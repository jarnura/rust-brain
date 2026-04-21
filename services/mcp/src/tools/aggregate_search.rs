//! MCP tool: aggregate_search
//!
//! Cross-database semantic search. Searches Qdrant embeddings, enriches with
//! Postgres metadata and Neo4j graph context.

use crate::client::ApiClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use tracing::instrument;

/// Request for aggregate_search
#[derive(Debug, Deserialize)]
pub struct AggregateSearchRequest {
    /// Natural language query to search for
    pub query: String,
    /// Maximum results to return
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Optional workspace ID to scope the search
    #[serde(default)]
    pub workspace_id: Option<String>,
}

fn default_limit() -> u32 {
    10
}

/// API request body
#[derive(Debug, Serialize)]
struct ApiAggregateSearchRequest {
    query: String,
    limit: u32,
}

/// A search result item
#[derive(Debug, Deserialize)]
struct SearchResult {
    fqn: Option<String>,
    #[allow(dead_code)]
    name: Option<String>,
    kind: Option<String>,
    file_path: Option<String>,
    score: Option<f64>,
    snippet: Option<String>,
    callers: Option<Vec<serde_json::Value>>,
    callees: Option<Vec<serde_json::Value>>,
}

/// API response
#[derive(Debug, Deserialize)]
struct AggregateSearchResponse {
    results: Vec<SearchResult>,
    #[allow(dead_code)]
    total: Option<usize>,
}

/// Execute the aggregate_search tool
#[instrument(skip(client))]
pub async fn execute(
    client: &ApiClient,
    request: AggregateSearchRequest,
    default_workspace_id: Option<&str>,
) -> Result<String> {
    let effective_ws = request.workspace_id.as_deref().or(default_workspace_id);
    let api_request = ApiAggregateSearchRequest {
        query: request.query.clone(),
        limit: request.limit,
    };

    let response: AggregateSearchResponse = client
        .post_with_workspace("/tools/aggregate_search", &api_request, effective_ws)
        .await?;

    if response.results.is_empty() {
        return Ok(format!("No results found for query: \"{}\"", request.query));
    }

    let mut output = format!(
        "# Search Results for \"{}\"\n\n**{} result(s)**\n\n",
        request.query,
        response.results.len()
    );

    for (i, result) in response.results.iter().enumerate() {
        output.push_str(&format!(
            "## {}. `{}`\n\n",
            i + 1,
            result.fqn.as_deref().unwrap_or("unknown")
        ));

        if let Some(kind) = &result.kind {
            output.push_str(&format!("**Kind:** {}\n", kind));
        }
        if let Some(file_path) = &result.file_path {
            output.push_str(&format!("**File:** `{}`\n", file_path));
        }
        if let Some(score) = result.score {
            output.push_str(&format!("**Relevance:** {:.2}\n", score));
        }
        if let Some(snippet) = &result.snippet {
            if !snippet.is_empty() {
                output.push_str(&format!("\n```rust\n{}\n```\n", snippet));
            }
        }
        if let Some(callers) = &result.callers {
            if !callers.is_empty() {
                let caller_names: Vec<&str> = callers
                    .iter()
                    .filter_map(|c| c.get("fqn").and_then(|v| v.as_str()).or_else(|| c.as_str()))
                    .collect();
                if !caller_names.is_empty() {
                    output.push_str(&format!("\n**Callers:** {}\n", caller_names.join(", ")));
                }
            }
        }
        if let Some(callees) = &result.callees {
            if !callees.is_empty() {
                let callee_names: Vec<&str> = callees
                    .iter()
                    .filter_map(|c| c.get("fqn").and_then(|v| v.as_str()).or_else(|| c.as_str()))
                    .collect();
                if !callee_names.is_empty() {
                    output.push_str(&format!("**Callees:** {}\n", callee_names.join(", ")));
                }
            }
        }
        output.push('\n');
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "aggregate_search",
        "description": "Cross-database semantic search. Searches Qdrant embeddings, enriches with Postgres metadata and Neo4j graph context.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural language query to search for"
                },
                "limit": {
                    "type": "number",
                    "description": "Maximum number of results to return (default 10)",
                    "default": 10
                },
                "workspace_id": {
                    "type": "string",
                    "description": "Optional: workspace ID to scope the search to a specific workspace. Use when searching within an isolated workspace."
                }
            },
            "required": ["query"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        assert_eq!(def["name"], "aggregate_search");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["limit"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn test_aggregate_search_request_deserialization() {
        let json = r#"{"query": "error handling patterns", "limit": 5}"#;
        let request: AggregateSearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.query, "error handling patterns");
        assert_eq!(request.limit, 5);
    }

    #[test]
    fn test_aggregate_search_request_default_limit() {
        let json = r#"{"query": "database connection"}"#;
        let request: AggregateSearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.limit, 10);
        assert_eq!(request.workspace_id, None);
    }

    #[test]
    fn test_aggregate_search_request_with_workspace_id() {
        let json = r#"{"query": "error handling", "limit": 5, "workspace_id": "ws_xyz"}"#;
        let request: AggregateSearchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.query, "error handling");
        assert_eq!(request.limit, 5);
        assert_eq!(request.workspace_id, Some("ws_xyz".to_string()));
    }
}
