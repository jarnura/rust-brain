//! MCP tool: search_docs
//!
//! Semantic search over documentation using embeddings

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for documentation search
#[derive(Debug, Deserialize)]
pub struct SearchDocsRequest {
    /// The search query (natural language)
    pub query: String,
    /// Maximum number of results (default: 10, max: 50)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Minimum similarity score threshold (0.0 to 1.0)
    #[serde(default)]
    pub score_threshold: Option<f32>,
    /// Optional workspace ID to scope the search
    #[serde(default)]
    pub workspace_id: Option<String>,
}

fn default_limit() -> usize {
    10
}

/// A single document search result
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct DocResult {
    /// Source file path
    pub source_file: String,
    /// Content preview/snippet
    pub content_preview: String,
    /// Similarity score
    pub score: f32,
}

/// Response from documentation search
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct SearchDocsResponse {
    /// Search results
    pub results: Vec<DocResult>,
    /// Original query
    pub query: String,
    /// Total number of results
    pub total: usize,
}

/// Execute the search_docs tool
#[instrument(skip(client))]
pub async fn execute(
    client: &ApiClient,
    request: SearchDocsRequest,
    default_workspace_id: Option<&str>,
) -> Result<String> {
    let effective_ws = request.workspace_id.as_deref().or(default_workspace_id);
    let api_request = serde_json::json!({
        "query": request.query,
        "limit": request.limit.min(50),
        "score_threshold": request.score_threshold,
    });

    let response: SearchDocsResponse = client
        .post_with_workspace("/tools/search_docs", &api_request, effective_ws)
        .await?;

    if response.results.is_empty() {
        return Ok("No documentation found for your query. Try using different keywords or lowering the score threshold.".to_string());
    }

    let mut output = format!(
        "Found {} documentation results for '{}' (showing top {}):\n\n",
        response.total,
        response.query,
        response.results.len()
    );

    for (i, result) in response.results.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {} (score: {:.2})\n",
            i + 1,
            result.source_file,
            result.score
        ));
        output.push_str(&format!("{}\n\n", result.content_preview));
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "search_docs",
        "description": "Search for documentation using semantic similarity. Finds relevant documentation, guides, and README files that match your query conceptually. Use natural language to find docs about features, APIs, or concepts.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Use natural language (e.g., 'how to authenticate users', 'error handling patterns')."
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

        assert_eq!(def["name"], "search_docs");
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

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[test]
    fn test_search_docs_request_deserialization() {
        let json = r#"{"query": "authentication guide", "limit": 20, "score_threshold": 0.5}"#;
        let request: SearchDocsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "authentication guide");
        assert_eq!(request.limit, 20);
        assert_eq!(request.score_threshold, Some(0.5));
    }

    #[test]
    fn test_search_docs_request_defaults() {
        let json = r#"{"query": "test"}"#;
        let request: SearchDocsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "test");
        assert_eq!(request.limit, 10);
        assert_eq!(request.score_threshold, None);
        assert_eq!(request.workspace_id, None);
    }

    #[test]
    fn test_search_docs_request_with_workspace_id() {
        let json = r#"{"query": "documentation", "limit": 10, "workspace_id": "ws_abc"}"#;
        let request: SearchDocsRequest = serde_json::from_str(json).unwrap();

        assert_eq!(request.query, "documentation");
        assert_eq!(request.workspace_id, Some("ws_abc".to_string()));
    }

    #[test]
    fn test_doc_result_deserialization() {
        let json = r#"{
            "source_file": "docs/api/authentication.md",
            "content_preview": "Authentication is handled via JWT tokens...",
            "score": 0.95
        }"#;

        let result: DocResult = serde_json::from_str(json).unwrap();

        assert_eq!(result.source_file, "docs/api/authentication.md");
        assert_eq!(
            result.content_preview,
            "Authentication is handled via JWT tokens..."
        );
        assert!((result.score - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_search_docs_response_deserialization() {
        let json = r#"{
            "results": [
                {
                    "source_file": "docs/guide.md",
                    "content_preview": "Getting started guide",
                    "score": 0.9
                }
            ],
            "query": "test",
            "total": 1
        }"#;

        let response: SearchDocsResponse = serde_json::from_str(json).unwrap();

        assert_eq!(response.results.len(), 1);
        assert_eq!(response.query, "test");
        assert_eq!(response.total, 1);
    }

    #[test]
    fn test_search_docs_response_empty() {
        let json = r#"{
            "results": [],
            "query": "nonexistent",
            "total": 0
        }"#;

        let response: SearchDocsResponse = serde_json::from_str(json).unwrap();

        assert!(response.results.is_empty());
        assert_eq!(response.total, 0);
    }

    #[test]
    fn test_doc_result_serialization() {
        let result = DocResult {
            source_file: "docs/api.md".to_string(),
            content_preview: "API documentation".to_string(),
            score: 0.9,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"source_file\":\"docs/api.md\""));
        assert!(json.contains("\"score\":0.9"));
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }
}
