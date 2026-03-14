//! MCP tool: query_graph
//!
//! Execute custom Cypher queries against the code graph

use crate::client::ApiClient;
use crate::error::{McpError, Result};
use serde::Deserialize;
use std::collections::HashMap;
use tracing::instrument;

/// Request for graph query
#[derive(Debug, Deserialize)]
pub struct QueryGraphRequest {
    /// Cypher query (read-only)
    pub query: String,
    /// Query parameters
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Maximum number of results
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

/// Response from graph query
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct GraphQueryResponse {
    /// Query results
    pub results: Vec<serde_json::Value>,
    /// Original query
    pub query: String,
    /// Number of rows returned
    pub row_count: usize,
}

/// Execute the query_graph tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: QueryGraphRequest) -> Result<String> {
    // Validate query is read-only
    let query_lower = request.query.to_lowercase();
    let forbidden_keywords = ["create", "delete", "set ", "remove", "merge"];
    
    for keyword in &forbidden_keywords {
        if query_lower.contains(keyword) {
            return Err(McpError::InvalidRequest(
                "Only read-only queries are allowed. CREATE, DELETE, SET, REMOVE, and MERGE are not permitted.".to_string()
            ));
        }
    }

    let response: GraphQueryResponse = client
        .post("/tools/query_graph", &serde_json::json!({
            "query": request.query,
            "parameters": request.parameters,
            "limit": request.limit.min(100),
        }))
        .await?;

    if response.results.is_empty() {
        return Ok("Query returned no results.".to_string());
    }

    let mut output = format!(
        "# Graph Query Results ({} rows)\n\n",
        response.row_count
    );

    output.push_str(&format!("**Query:**\n```cypher\n{}\n```\n\n", response.query));

    output.push_str("## Results\n\n");

    for (i, row) in response.results.iter().enumerate() {
        output.push_str(&format!("### Row {}\n", i + 1));
        
        // Format the JSON value nicely
        match row {
            serde_json::Value::Object(map) => {
                for (key, value) in map {
                    let formatted_value = format_value(value);
                    output.push_str(&format!("- **{}:** {}\n", key, formatted_value));
                }
            }
            _ => {
                output.push_str(&format!("{}\n", serde_json::to_string_pretty(row).unwrap_or_default()));
            }
        }
        output.push('\n');
    }

    Ok(output)
}

/// Format a JSON value for human-readable output
fn format_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => format!("`{}`", s),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else if arr.len() <= 5 {
                format!(
                    "[{}]",
                    arr.iter()
                        .map(format_value)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!("[{} items]", arr.len())
            }
        }
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                "{}".to_string()
            } else if map.len() <= 3 {
                format!(
                    "{{ {} }}",
                    map.iter()
                        .map(|(k, v)| format!("{}: {}", k, format_value(v)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                format!("{{ {} keys }}", map.len())
            }
        }
        serde_json::Value::Null => "null".to_string(),
    }
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "query_graph",
        "description": "Execute a custom Cypher query against the code knowledge graph. This is an advanced tool for exploring relationships that aren't covered by other tools. Only read-only queries are allowed.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Cypher query (read-only). Example: 'MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.name, g.name LIMIT 10'"
                },
                "parameters": {
                    "type": "object",
                    "description": "Query parameters (optional)",
                    "additionalProperties": true
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 50, max: 100)",
                    "default": 50,
                    "minimum": 1,
                    "maximum": 100
                }
            },
            "required": ["query"]
        }
    })
}
