//! MCP tool: consistency_check
//!
//! Check cross-store data consistency across Postgres, Neo4j, and Qdrant.

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for consistency_check
#[derive(Debug, Deserialize)]
pub struct ConsistencyCheckRequest {
    /// Crate name to check (optional - checks all crates if omitted)
    #[serde(rename = "crate")]
    pub crate_name: Option<String>,
    /// Detail level: "summary" (counts only) or "full" (FQN sets)
    pub detail: Option<String>,
}

/// Execute the consistency_check tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: ConsistencyCheckRequest) -> Result<String> {
    let mut url = "/api/consistency?".to_string();
    let mut params = vec![];

    if let Some(crate_name) = &request.crate_name {
        params.push(format!("crate={}", crate_name));
    }

    let detail = request.detail.as_deref().unwrap_or("summary");
    params.push(format!("detail={}", detail));

    url.push_str(&params.join("&"));

    let result: serde_json::Value = client.get(&url).await?;

    let crate_name = result["crate_name"].as_str().unwrap_or("unknown");
    let status = result["status"].as_str().unwrap_or("unknown");
    let counts = &result["store_counts"];
    let recommendation = result["recommendation"]
        .as_str()
        .unwrap_or("No recommendation");

    let mut output = format!(
        "# Consistency Report: {}\n\n**Status:** {}\n\n",
        crate_name,
        status.to_uppercase()
    );

    output.push_str("## Store Counts\n\n");
    output.push_str("| Store | Count |\n");
    output.push_str("| --- | --- |\n");
    output.push_str(&format!(
        "| Postgres | {} |\n",
        counts["postgres"].as_u64().unwrap_or(0)
    ));
    output.push_str(&format!(
        "| Neo4j | {} |\n",
        counts["neo4j"].as_u64().unwrap_or(0)
    ));
    output.push_str(&format!(
        "| Qdrant | {} |\n",
        counts["qdrant"].as_u64().unwrap_or(0)
    ));

    output.push_str(&format!("\n**Recommendation:** {}\n", recommendation));

    if let Some(discrepancies) = result.get("discrepancies") {
        output.push_str("\n## Discrepancies\n\n");

        let in_pg_not_neo4j = discrepancies["in_postgres_not_neo4j"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let in_pg_not_qdrant = discrepancies["in_postgres_not_qdrant"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let in_neo4j_not_pg = discrepancies["in_neo4j_not_postgres"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let in_qdrant_not_pg = discrepancies["in_qdrant_not_postgres"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);

        if in_pg_not_neo4j > 0
            || in_pg_not_qdrant > 0
            || in_neo4j_not_pg > 0
            || in_qdrant_not_pg > 0
        {
            output.push_str("| Discrepancy Type | Count |\n");
            output.push_str("| --- | --- |\n");
            if in_pg_not_neo4j > 0 {
                output.push_str(&format!(
                    "| In Postgres, not Neo4j | {} |\n",
                    in_pg_not_neo4j
                ));
            }
            if in_pg_not_qdrant > 0 {
                output.push_str(&format!(
                    "| In Postgres, not Qdrant | {} |\n",
                    in_pg_not_qdrant
                ));
            }
            if in_neo4j_not_pg > 0 {
                output.push_str(&format!(
                    "| In Neo4j, not Postgres | {} |\n",
                    in_neo4j_not_pg
                ));
            }
            if in_qdrant_not_pg > 0 {
                output.push_str(&format!(
                    "| In Qdrant, not Postgres | {} |\n",
                    in_qdrant_not_pg
                ));
            }
        } else {
            output.push_str("No discrepancies found.\n");
        }
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "consistency_check",
        "description": "Check cross-store data consistency across Postgres, Neo4j, and Qdrant. Use detail='full' to get FQN-level discrepancies.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "crate": {
                    "type": "string",
                    "description": "Crate name to check (optional - checks all crates if omitted)"
                },
                "detail": {
                    "type": "string",
                    "description": "Detail level: 'summary' (counts only, fast) or 'full' (FQN sets, slower but precise)",
                    "enum": ["summary", "full"]
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        assert_eq!(def["name"], "consistency_check");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["crate"].is_object());
        assert!(schema["properties"]["detail"].is_object());
    }

    #[test]
    fn test_consistency_check_request_deserialization_with_crate() {
        let json = r#"{"crate": "my_crate", "detail": "full"}"#;
        let request: ConsistencyCheckRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.crate_name, Some("my_crate".to_string()));
        assert_eq!(request.detail, Some("full".to_string()));
    }

    #[test]
    fn test_consistency_check_request_deserialization_empty() {
        let json = r#"{}"#;
        let request: ConsistencyCheckRequest = serde_json::from_str(json).unwrap();
        assert!(request.crate_name.is_none());
        assert!(request.detail.is_none());
    }
}
