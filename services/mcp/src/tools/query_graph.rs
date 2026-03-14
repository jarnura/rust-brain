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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        
        assert_eq!(def["name"], "query_graph");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["parameters"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn test_query_graph_request_deserialization() {
        let json = r#"{
            "query": "MATCH (f:Function) RETURN f.name",
            "parameters": {"limit": 10},
            "limit": 50
        }"#;
        
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.query, "MATCH (f:Function) RETURN f.name");
        assert_eq!(request.parameters.get("limit").unwrap(), 10);
        assert_eq!(request.limit, 50);
    }

    #[test]
    fn test_query_graph_request_minimal() {
        let json = r#"{"query": "MATCH (n) RETURN n"}"#;
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.query, "MATCH (n) RETURN n");
        assert!(request.parameters.is_empty());
        assert_eq!(request.limit, 50); // default
    }

    #[test]
    fn test_default_limit_value() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn test_graph_query_response_deserialization() {
        let json = r#"{
            "results": [
                {"name": "func1", "kind": "function"},
                {"name": "func2", "kind": "function"}
            ],
            "query": "MATCH (f:Function) RETURN f.name",
            "row_count": 2
        }"#;
        
        let response: GraphQueryResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.query, "MATCH (f:Function) RETURN f.name");
        assert_eq!(response.row_count, 2);
    }

    #[test]
    fn test_graph_query_response_empty() {
        let json = r#"{
            "results": [],
            "query": "MATCH (n:NonExistent) RETURN n",
            "row_count": 0
        }"#;
        
        let response: GraphQueryResponse = serde_json::from_str(json).unwrap();
        
        assert!(response.results.is_empty());
        assert_eq!(response.row_count, 0);
    }

    #[test]
    fn test_graph_query_response_serialization() {
        let response = GraphQueryResponse {
            results: vec![
                serde_json::json!({"name": "func"}),
            ],
            query: "MATCH (f:Function) RETURN f.name".to_string(),
            row_count: 1,
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"query\":\"MATCH (f:Function) RETURN f.name\""));
        assert!(json.contains("\"row_count\":1"));
    }

    #[test]
    fn test_format_value_string() {
        let value = serde_json::json!("test_string");
        let formatted = format_value(&value);
        assert_eq!(formatted, "`test_string`");
    }

    #[test]
    fn test_format_value_number() {
        let value = serde_json::json!(42);
        let formatted = format_value(&value);
        assert_eq!(formatted, "42");
    }

    #[test]
    fn test_format_value_bool() {
        let value = serde_json::json!(true);
        let formatted = format_value(&value);
        assert_eq!(formatted, "true");
    }

    #[test]
    fn test_format_value_null() {
        let value = serde_json::json!(null);
        let formatted = format_value(&value);
        assert_eq!(formatted, "null");
    }

    #[test]
    fn test_format_value_empty_array() {
        let value = serde_json::json!([]);
        let formatted = format_value(&value);
        assert_eq!(formatted, "[]");
    }

    #[test]
    fn test_format_value_small_array() {
        let value = serde_json::json!([1, 2, 3]);
        let formatted = format_value(&value);
        assert_eq!(formatted, "[1, 2, 3]");
    }

    #[test]
    fn test_format_value_large_array() {
        let value = serde_json::json!([1, 2, 3, 4, 5, 6, 7]);
        let formatted = format_value(&value);
        assert_eq!(formatted, "[7 items]");
    }

    #[test]
    fn test_format_value_empty_object() {
        let value = serde_json::json!({});
        let formatted = format_value(&value);
        assert_eq!(formatted, "{}");
    }

    #[test]
    fn test_format_value_small_object() {
        let value = serde_json::json!({"a": 1, "b": 2});
        let formatted = format_value(&value);
        // Keys may be in any order
        assert!(formatted.contains("a: 1"));
        assert!(formatted.contains("b: 2"));
    }

    #[test]
    fn test_format_value_large_object() {
        let value = serde_json::json!({"a": 1, "b": 2, "c": 3, "d": 4});
        let formatted = format_value(&value);
        assert_eq!(formatted, "{ 4 keys }");
    }

    #[test]
    fn test_query_graph_request_with_parameters() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), serde_json::json!("test_func"));
        
        let request = QueryGraphRequest {
            query: "MATCH (f:Function {name: $name}) RETURN f".to_string(),
            parameters: params,
            limit: 10,
        };
        
        assert_eq!(request.query, "MATCH (f:Function {name: $name}) RETURN f");
        assert_eq!(request.parameters.get("name").unwrap(), "test_func");
        assert_eq!(request.limit, 10);
    }
}
