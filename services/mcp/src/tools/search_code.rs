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
    /// Restrict results to a specific crate name
    #[serde(default)]
    pub crate_filter: Option<String>,
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
        "crate_filter": request.crate_filter,
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
                },
                "crate_filter": {
                    "type": "string",
                    "description": "Optional: restrict results to a specific crate name."
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
        
        assert_eq!(def["name"], "search_code");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];

        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert!(schema["properties"]["limit"].is_object());
        assert!(schema["properties"]["score_threshold"].is_object());
        assert!(schema["properties"]["crate_filter"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn test_search_code_request_deserialization() {
        let json = r#"{"query": "parse json", "limit": 20, "score_threshold": 0.5}"#;
        let request: SearchCodeRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "parse json");
        assert_eq!(request.limit, 20);
        assert_eq!(request.score_threshold, Some(0.5));
        assert_eq!(request.crate_filter, None);
    }

    #[test]
    fn test_search_code_request_with_crate_filter() {
        let json = r#"{"query": "parse json", "limit": 10, "crate_filter": "my_crate"}"#;
        let request: SearchCodeRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "parse json");
        assert_eq!(request.crate_filter, Some("my_crate".to_string()));
    }

    #[test]
    fn test_search_code_request_defaults() {
        let json = r#"{"query": "test"}"#;
        let request: SearchCodeRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "test");
        assert_eq!(request.limit, 10); // default
        assert_eq!(request.score_threshold, None);
        assert_eq!(request.crate_filter, None);
    }

    #[test]
    fn test_search_result_deserialization() {
        let json = r#"{
            "fqn": "crate::module::function",
            "name": "function",
            "kind": "function",
            "file_path": "src/module.rs",
            "start_line": 10,
            "end_line": 20,
            "score": 0.95,
            "snippet": "fn function() {}",
            "docstring": "A function"
        }"#;
        
        let result: SearchResult = serde_json::from_str(json).unwrap();
        
        assert_eq!(result.fqn, "crate::module::function");
        assert_eq!(result.name, "function");
        assert_eq!(result.kind, "function");
        assert_eq!(result.file_path, "src/module.rs");
        assert_eq!(result.start_line, 10);
        assert_eq!(result.end_line, 20);
        assert!((result.score - 0.95).abs() < 0.001);
        assert_eq!(result.snippet, Some("fn function() {}".to_string()));
        assert_eq!(result.docstring, Some("A function".to_string()));
    }

    #[test]
    fn test_search_result_minimal() {
        let json = r#"{
            "fqn": "crate::func",
            "name": "func",
            "kind": "function",
            "file_path": "src/lib.rs",
            "start_line": 1,
            "end_line": 5,
            "score": 0.5
        }"#;
        
        let result: SearchResult = serde_json::from_str(json).unwrap();
        
        assert_eq!(result.fqn, "crate::func");
        assert_eq!(result.snippet, None);
        assert_eq!(result.docstring, None);
    }

    #[test]
    fn test_search_code_response_deserialization() {
        let json = r#"{
            "results": [
                {
                    "fqn": "crate::func",
                    "name": "func",
                    "kind": "function",
                    "file_path": "src/lib.rs",
                    "start_line": 1,
                    "end_line": 5,
                    "score": 0.9
                }
            ],
            "query": "test",
            "total": 1
        }"#;
        
        let response: SearchCodeResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.query, "test");
        assert_eq!(response.total, 1);
    }

    #[test]
    fn test_search_code_response_empty() {
        let json = r#"{
            "results": [],
            "query": "nonexistent",
            "total": 0
        }"#;
        
        let response: SearchCodeResponse = serde_json::from_str(json).unwrap();
        
        assert!(response.results.is_empty());
        assert_eq!(response.total, 0);
    }

    #[test]
    fn test_search_result_serialization() {
        let result = SearchResult {
            fqn: "crate::func".to_string(),
            name: "func".to_string(),
            kind: "function".to_string(),
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 5,
            score: 0.9,
            snippet: None,
            docstring: None,
        };
        
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"fqn\":\"crate::func\""));
        assert!(json.contains("\"score\":0.9"));
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }
}
