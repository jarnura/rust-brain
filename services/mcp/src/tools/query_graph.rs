//! MCP tool: query_graph
//!
//! Execute pre-approved named query templates against the code graph.
//! Arbitrary Cypher is not accepted; callers select from a fixed allowlist of
//! parameterized templates resolved server-side in resolve_named_query().

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use std::collections::HashMap;
use tracing::instrument;

/// Request for graph query — flat schema (no nested parameters object).
/// All template params are top-level fields alongside query_name.
#[derive(Debug, Deserialize)]
pub struct QueryGraphRequest {
    /// Name of a pre-approved query template.
    pub query_name: String,
    /// Nested parameters object (legacy format — still accepted)
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Flat params — preferred format, avoids nested JSON issues
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fqn: Option<String>,
    #[serde(default)]
    pub crate_name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub trait_name: Option<String>,
    #[serde(default)]
    pub type_name: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub depth: Option<i64>,
}

impl QueryGraphRequest {
    /// Merge flat params into the parameters HashMap.
    /// Flat params take precedence over nested ones.
    pub fn merged_parameters(&self) -> HashMap<String, serde_json::Value> {
        let mut params = self.parameters.clone();
        if let Some(ref v) = self.name {
            params.insert("name".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.fqn {
            params.insert("fqn".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.crate_name {
            params.insert("crate_name".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.label {
            params.insert("label".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.path {
            params.insert("path".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.trait_name {
            params.insert("trait_name".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.type_name {
            params.insert("type_name".into(), serde_json::json!(v));
        }
        if let Some(ref v) = self.prefix {
            params.insert("prefix".into(), serde_json::json!(v));
        }
        if let Some(v) = self.limit {
            params.insert("limit".into(), serde_json::json!(v));
        }
        if let Some(v) = self.depth {
            params.insert("depth".into(), serde_json::json!(v));
        }
        params
    }
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
    let params = request.merged_parameters();
    let response: GraphQueryResponse = client
        .post(
            "/tools/query_graph",
            &serde_json::json!({
                "query_name": request.query_name,
                "parameters": params,
            }),
        )
        .await?;

    if response.results.is_empty() {
        return Ok("Query returned no results.".to_string());
    }

    let mut output = format!("# Graph Query Results ({} rows)\n\n", response.row_count);

    output.push_str(&format!("**Query:** `{}`\n\n", response.query));

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
                output.push_str(&format!(
                    "{}\n",
                    serde_json::to_string_pretty(row).unwrap_or_default()
                ));
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
                    arr.iter().map(format_value).collect::<Vec<_>>().join(", ")
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
        "description": "Execute a named query template against the code knowledge graph. \
                        All parameters are flat (no nested objects). \
                        Example: {\"query_name\": \"find_crate_dependencies\", \"crate_name\": \"router\"}",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query_name": {
                    "type": "string",
                    "description": "Name of the query template to run.",
                    "enum": [
                        "find_functions_by_name",
                        "find_callers",
                        "find_callees",
                        "find_trait_implementations",
                        "find_by_fqn",
                        "find_neighbors",
                        "find_nodes_by_label",
                        "find_module_contents",
                        "count_by_label",
                        "find_crate_overview",
                        "find_crate_dependencies",
                        "find_crate_dependents",
                        "get_trait_impls_api",
                        "find_usages_of_type",
                        "get_module_tree",
                        "get_callers",
                        "get_callers_for_impl",
                        "get_callees_for_impl",
                        "get_callees",
                        "consistency_fqns",
                        "consistency_fqns_filtered",
                        "consistency_count"
                    ]
                },
                "name": {
                    "type": "string",
                    "description": "Function/trait/module name to search for"
                },
                "fqn": {
                    "type": "string",
                    "description": "Fully qualified name (e.g. router::routes::payments::payments_create)"
                },
                "crate_name": {
                    "type": "string",
                    "description": "Crate name (for find_crate_overview, find_crate_dependencies, find_crate_dependents)"
                },
                "label": {
                    "type": "string",
                    "description": "Node label filter",
                    "enum": ["Function", "Struct", "Enum", "Trait", "Impl", "Module", "Crate", "Type", "TypeAlias", "Const", "Static", "Macro"]
                },
                "path": {
                    "type": "string",
                    "description": "Module path (for find_module_contents)"
                },
                "trait_name": {
                    "type": "string",
                    "description": "Trait name (for get_trait_impls_api)"
                },
                "type_name": {
                    "type": "string",
                    "description": "Type name (for find_usages_of_type)"
                },
                "prefix": {
                    "type": "string",
                    "description": "FQN prefix (for get_callers_for_impl, get_callees_for_impl)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max results (1-100, default 25)"
                },
                "depth": {
                    "type": "integer",
                    "description": "Traversal depth (1-5, default 1, for find_callers/find_neighbors)"
                }
            },
            "required": ["query_name"]
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
        assert!(schema["properties"]["query_name"].is_object());
        assert!(schema["properties"]["name"].is_object());
        assert!(schema["properties"]["fqn"].is_object());
        assert!(schema["properties"]["crate_name"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query_name")));
    }

    #[test]
    fn test_definition_enum_values() {
        let schema = &definition()["inputSchema"];
        let enum_values = schema["properties"]["query_name"]["enum"]
            .as_array()
            .unwrap();
        assert!(enum_values.contains(&serde_json::json!("find_functions_by_name")));
        assert!(enum_values.contains(&serde_json::json!("find_callers")));
        assert!(enum_values.contains(&serde_json::json!("count_by_label")));
    }

    #[test]
    fn test_query_graph_request_deserialization() {
        let json = r#"{
            "query_name": "find_functions_by_name",
            "parameters": {"name": "my_func", "limit": 10}
        }"#;

        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query_name, "find_functions_by_name");
        assert_eq!(request.parameters.get("name").unwrap(), "my_func");
        assert_eq!(request.parameters.get("limit").unwrap(), 10);
    }

    #[test]
    fn test_query_graph_request_minimal() {
        let json = r#"{"query_name": "find_crate_overview"}"#;
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query_name, "find_crate_overview");
        assert!(request.parameters.is_empty());
    }

    #[test]
    fn test_query_graph_request_flat_format() {
        // This is the new preferred format — no nested parameters object
        let json = r#"{"query_name": "find_crate_dependencies", "crate_name": "router"}"#;
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query_name, "find_crate_dependencies");
        assert_eq!(request.crate_name, Some("router".to_string()));

        let merged = request.merged_parameters();
        assert_eq!(merged.get("crate_name").unwrap(), "router");
    }

    #[test]
    fn test_query_graph_request_flat_overrides_nested() {
        // Flat params take precedence over nested
        let json = r#"{"query_name": "find_callers", "name": "flat_name", "parameters": {"name": "nested_name"}}"#;
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();

        let merged = request.merged_parameters();
        assert_eq!(merged.get("name").unwrap(), "flat_name");
    }

    #[test]
    fn test_query_graph_request_flat_with_limit_depth() {
        let json = r#"{"query_name": "find_callers", "fqn": "router::payments::create", "limit": 50, "depth": 3}"#;
        let request: QueryGraphRequest = serde_json::from_str(json).unwrap();

        let merged = request.merged_parameters();
        assert_eq!(merged.get("fqn").unwrap(), "router::payments::create");
        assert_eq!(merged.get("limit").unwrap(), 50);
        assert_eq!(merged.get("depth").unwrap(), 3);
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
            results: vec![serde_json::json!({"name": "func"})],
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
        params.insert("limit".to_string(), serde_json::json!(10));

        let request = QueryGraphRequest {
            query_name: "find_functions_by_name".to_string(),
            parameters: params,
            name: None,
            fqn: None,
            crate_name: None,
            label: None,
            path: None,
            trait_name: None,
            type_name: None,
            prefix: None,
            limit: None,
            depth: None,
        };

        assert_eq!(request.query_name, "find_functions_by_name");
        assert_eq!(request.parameters.get("name").unwrap(), "test_func");
        assert_eq!(request.parameters.get("limit").unwrap(), 10);
    }
}
