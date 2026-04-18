//! MCP tool: pg_query
//!
//! Execute a read-only parameterized SQL query against the code intelligence database.

use crate::client::ApiClient;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use tracing::instrument;

/// Request for pg_query
#[derive(Debug, Deserialize)]
pub struct PgQueryRequest {
    /// The SQL query to execute. Use $1, $2 for parameters.
    pub query: String,
    /// Bind parameters (optional)
    #[serde(default)]
    pub params: Vec<String>,
}

/// API request body sent to the REST endpoint
#[derive(Debug, Serialize)]
struct ApiPgQueryRequest {
    query: String,
    params: Vec<String>,
}

/// API response from the REST endpoint
#[derive(Debug, Deserialize)]
struct PgQueryResponse {
    rows: Vec<serde_json::Value>,
    row_count: usize,
}

/// Execute the pg_query tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: PgQueryRequest) -> Result<String> {
    let api_request = ApiPgQueryRequest {
        query: request.query,
        params: request.params,
    };

    let response: PgQueryResponse = client.post("/tools/pg_query", &api_request).await?;

    if response.rows.is_empty() {
        return Ok("No rows returned.".to_string());
    }

    // Format as markdown table
    let mut output = String::new();
    output.push_str(&format!("**{} row(s) returned**\n\n", response.row_count));

    // Get column names from first row
    let columns: Vec<String> = if let Some(first) = response.rows.first() {
        if let Some(obj) = first.as_object() {
            obj.keys().cloned().collect()
        } else {
            return Ok(format!("{} rows returned (non-object format)", response.row_count));
        }
    } else {
        return Ok("No rows returned.".to_string());
    };

    // Header
    output.push_str("| ");
    output.push_str(&columns.join(" | "));
    output.push_str(" |\n");

    // Separator
    output.push_str("| ");
    output.push_str(&columns.iter().map(|_| "---").collect::<Vec<_>>().join(" | "));
    output.push_str(" |\n");

    // Data rows (max 20)
    let display_count = response.rows.len().min(20);
    for row in response.rows.iter().take(20) {
        output.push_str("| ");
        let values: Vec<String> = columns
            .iter()
            .map(|col| {
                row.get(col)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => "NULL".to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_else(|| "NULL".to_string())
            })
            .collect();
        output.push_str(&values.join(" | "));
        output.push_str(" |\n");
    }

    if response.row_count > display_count {
        output.push_str(&format!(
            "\n... and {} more rows\n",
            response.row_count - display_count
        ));
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "pg_query",
        "description": "Execute a read-only parameterized SQL query against the code intelligence database. Use $1, $2 for parameters. Tables: extracted_items (code symbols), source_files, artifacts, tasks, call_sites, trait_implementations.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "SQL SELECT query. Use $1, $2, etc. for bind parameters."
                },
                "params": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Bind parameters for the query (optional)",
                    "default": []
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
        assert_eq!(def["name"], "pg_query");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["params"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn test_pg_query_request_deserialization() {
        let json = r#"{"query": "SELECT * FROM tasks WHERE status = $1", "params": ["pending"]}"#;
        let request: PgQueryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.query, "SELECT * FROM tasks WHERE status = $1");
        assert_eq!(request.params, vec!["pending"]);
    }

    #[test]
    fn test_pg_query_request_no_params() {
        let json = r#"{"query": "SELECT count(*) FROM tasks"}"#;
        let request: PgQueryRequest = serde_json::from_str(json).unwrap();
        assert!(request.params.is_empty());
    }
}
