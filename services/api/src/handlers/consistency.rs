//! Cross-store consistency checker endpoints.
//!
//! Provides endpoints for verifying data consistency across Postgres, Neo4j, and Qdrant:
//! - `GET /api/consistency` — detailed per-crate consistency report
//! - `GET /health/consistency` — aggregate health check for Prometheus

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::errors::AppError;
use crate::extractors::OptionalWorkspaceId;
use crate::neo4j::{execute_neo4j_query, WorkspaceContext, WorkspaceGraphClient}; // RUSA-194-EXEMPT: fallback for system-level checks without workspace
use crate::state::AppState;
use crate::workspace::acquire_conn;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Query parameters for `GET /api/consistency`.
#[derive(Debug, Deserialize)]
pub struct ConsistencyQuery {
    /// Crate name to check (optional - checks all crates if omitted)
    #[serde(rename = "crate")]
    pub crate_name: Option<String>,
    /// Detail level: "summary" (counts only) or "full" (FQN sets)
    #[serde(default = "default_detail")]
    pub detail: String,
}

fn default_detail() -> String {
    "summary".to_string()
}

/// Response for `GET /api/consistency`.
#[derive(Debug, Serialize)]
pub struct ConsistencyReport {
    /// Crate name that was checked (or "all" if no specific crate)
    pub crate_name: String,
    /// Timestamp of the check
    pub timestamp: String,
    /// Per-store item counts
    pub store_counts: StoreCounts,
    /// Detailed discrepancies (only populated when detail=full)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discrepancies: Option<Discrepancies>,
    /// Overall status: "consistent" or "inconsistent"
    pub status: String,
    /// Recommendation for remediation
    pub recommendation: String,
}

/// Item counts from each store.
#[derive(Debug, Serialize)]
pub struct StoreCounts {
    /// Postgres extracted_items count
    pub postgres: usize,
    /// Neo4j Function/Struct/etc. nodes count
    pub neo4j: usize,
    /// Qdrant points count
    pub qdrant: usize,
}

/// Detailed discrepancies between stores.
#[derive(Debug, Serialize, Default)]
pub struct Discrepancies {
    /// Items in Postgres but not in Neo4j
    pub in_postgres_not_neo4j: Vec<String>,
    /// Items in Postgres but not in Qdrant
    pub in_postgres_not_qdrant: Vec<String>,
    /// Items in Neo4j but not in Postgres
    pub in_neo4j_not_postgres: Vec<String>,
    /// Items in Qdrant but not in Postgres
    pub in_qdrant_not_postgres: Vec<String>,
}

/// Response for `GET /health/consistency`.
#[derive(Debug, Serialize)]
pub struct ConsistencyHealthResponse {
    /// Overall status: "healthy" or "unhealthy"
    pub status: String,
    /// Total crates checked
    pub total_crates: usize,
    /// Number of crates with inconsistencies
    pub inconsistent_crates: usize,
    /// Per-crate summary
    pub crates: Vec<CrateHealthSummary>,
}

/// Summary for a single crate in the health check.
#[derive(Debug, Serialize)]
pub struct CrateHealthSummary {
    /// Crate name
    pub crate_name: String,
    /// Whether this crate is consistent
    pub consistent: bool,
    /// Per-store counts
    pub counts: StoreCounts,
}

// =============================================================================
// Handlers
// =============================================================================

/// Checks cross-store consistency for a specific crate or all crates.
///
/// Queries Postgres, Neo4j, and Qdrant to compare item counts and optionally
/// FQN sets. Returns a detailed report with recommendations.
///
/// # Errors
///
/// Returns [`AppError::Database`] if Postgres query fails.
/// Returns [`AppError::Neo4j`] if Neo4j query fails.
/// Returns [`AppError::Qdrant`] if Qdrant query fails.
pub async fn check_consistency(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Query(query): Query<ConsistencyQuery>,
) -> Result<Json<ConsistencyReport>, AppError> {
    let crate_name = query.crate_name.unwrap_or_else(|| "all".to_string());
    let detail_full = query.detail == "full";

    info!(
        "Checking consistency for crate: {} (detail={})",
        crate_name, query.detail
    );

    // Run all three store queries in parallel with timeout
    let timeout_duration = Duration::from_secs(30);

    let pg_future = async {
        tokio::time::timeout(
            timeout_duration,
            query_postgres_fqns(&state, &crate_name, ws.as_ref()),
        )
        .await
    };

    let neo4j_future = async {
        tokio::time::timeout(
            timeout_duration,
            query_neo4j_fqns(&state, &crate_name, ws.as_ref()),
        )
        .await
    };

    let collection_name = crate::workspace::resolve_code_collection(ws.as_ref(), &state.config);
    let qdrant_future = async {
        tokio::time::timeout(
            timeout_duration,
            query_qdrant_fqns(&state, &crate_name, &collection_name),
        )
        .await
    };

    let (pg_result, neo4j_result, qdrant_result) =
        tokio::join!(pg_future, neo4j_future, qdrant_future);

    // Collect results, handling timeouts and errors
    let pg_fqns = match pg_result {
        Ok(Ok(fqns)) => fqns,
        Ok(Err(e)) => {
            error!("Postgres consistency query failed: {}", e);
            return Err(e);
        }
        Err(_) => {
            return Err(AppError::Database(
                "Postgres query timed out after 30s".to_string(),
            ));
        }
    };

    let neo4j_fqns = match neo4j_result {
        Ok(Ok(fqns)) => fqns,
        Ok(Err(e)) => {
            error!("Neo4j consistency query failed: {}", e);
            return Err(e);
        }
        Err(_) => {
            return Err(AppError::Neo4j(
                "Neo4j query timed out after 30s".to_string(),
            ));
        }
    };

    let qdrant_fqns = match qdrant_result {
        Ok(Ok(fqns)) => fqns,
        Ok(Err(e)) => {
            error!("Qdrant consistency query failed: {}", e);
            return Err(e);
        }
        Err(_) => {
            return Err(AppError::Qdrant(
                "Qdrant query timed out after 30s".to_string(),
            ));
        }
    };

    // Build store counts
    let store_counts = StoreCounts {
        postgres: pg_fqns.len(),
        neo4j: neo4j_fqns.len(),
        qdrant: qdrant_fqns.len(),
    };

    // Calculate discrepancies if full detail requested
    let discrepancies = if detail_full {
        let pg_set: HashSet<_> = pg_fqns.iter().cloned().collect();
        let neo4j_set: HashSet<_> = neo4j_fqns.iter().cloned().collect();
        let qdrant_set: HashSet<_> = qdrant_fqns.iter().cloned().collect();

        let in_pg_not_neo4j: Vec<String> = pg_set.difference(&neo4j_set).cloned().collect();
        let in_pg_not_qdrant: Vec<String> = pg_set.difference(&qdrant_set).cloned().collect();
        let in_neo4j_not_pg: Vec<String> = neo4j_set.difference(&pg_set).cloned().collect();
        let in_qdrant_not_pg: Vec<String> = qdrant_set.difference(&pg_set).cloned().collect();

        Some(Discrepancies {
            in_postgres_not_neo4j: in_pg_not_neo4j,
            in_postgres_not_qdrant: in_pg_not_qdrant,
            in_neo4j_not_postgres: in_neo4j_not_pg,
            in_qdrant_not_postgres: in_qdrant_not_pg,
        })
    } else {
        None
    };

    // Determine overall status and recommendation
    let is_consistent =
        store_counts.postgres == store_counts.neo4j && store_counts.postgres == store_counts.qdrant;

    let status = if is_consistent {
        "consistent"
    } else {
        "inconsistent"
    };

    let recommendation = generate_recommendation(&store_counts, &discrepancies);

    let report = ConsistencyReport {
        crate_name: crate_name.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        store_counts,
        discrepancies,
        status: status.to_string(),
        recommendation,
    };

    Ok(Json(report))
}

/// Aggregate health check for all ingested crates.
///
/// Returns HTTP 200 if all crates are consistent, HTTP 503 if any
/// inconsistencies are found. The response body includes counts suitable
/// for Prometheus scraping.
pub async fn health_consistency(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
) -> Result<Response, AppError> {
    debug!("Running aggregate consistency health check");

    // Get list of all crates from Postgres
    let crates = get_all_crate_names(&state, ws.as_ref()).await?;

    if crates.is_empty() {
        // No crates ingested - return healthy with empty stats
        let response = ConsistencyHealthResponse {
            status: "healthy".to_string(),
            total_crates: 0,
            inconsistent_crates: 0,
            crates: vec![],
        };
        return Ok((StatusCode::OK, Json(response)).into_response());
    }

    // Check each crate with a shorter timeout
    let timeout_duration = Duration::from_secs(10);
    let mut crate_summaries: Vec<CrateHealthSummary> = Vec::new();

    let collection_name = crate::workspace::resolve_code_collection(ws.as_ref(), &state.config);
    for crate_name in &crates {
        // Run quick count-only check for each crate
        let pg_future = tokio::time::timeout(
            timeout_duration,
            query_postgres_count(&state, crate_name, ws.as_ref()),
        );
        let neo4j_future = tokio::time::timeout(
            timeout_duration,
            query_neo4j_count(&state, crate_name, ws.as_ref()),
        );
        let qdrant_future = tokio::time::timeout(
            timeout_duration,
            query_qdrant_count(&state, crate_name, &collection_name),
        );

        let (pg_result, neo4j_result, qdrant_result) =
            tokio::join!(pg_future, neo4j_future, qdrant_future);

        // Get counts, defaulting to 0 on error
        let pg_count = pg_result.ok().and_then(|r| r.ok()).unwrap_or(0);
        let neo4j_count = neo4j_result.ok().and_then(|r| r.ok()).unwrap_or(0);
        let qdrant_count = qdrant_result.ok().and_then(|r| r.ok()).unwrap_or(0);

        let consistent = pg_count == neo4j_count && pg_count == qdrant_count;

        crate_summaries.push(CrateHealthSummary {
            crate_name: crate_name.clone(),
            consistent,
            counts: StoreCounts {
                postgres: pg_count,
                neo4j: neo4j_count,
                qdrant: qdrant_count,
            },
        });
    }

    let inconsistent_crates = crate_summaries.iter().filter(|s| !s.consistent).count();
    let all_consistent = inconsistent_crates == 0;

    let response = ConsistencyHealthResponse {
        status: if all_consistent {
            "healthy"
        } else {
            "unhealthy"
        }
        .to_string(),
        total_crates: crates.len(),
        inconsistent_crates,
        crates: crate_summaries,
    };

    let status_code = if all_consistent {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    Ok((status_code, Json(response)).into_response())
}

// =============================================================================
// Query Functions
// =============================================================================

/// Query Postgres for FQNs matching a crate.
async fn query_postgres_fqns(
    state: &AppState,
    crate_name: &str,
    ws: Option<&WorkspaceContext>,
) -> Result<Vec<String>, AppError> {
    let mut conn = acquire_conn(&state.pg_pool, ws).await?;

    let query = if crate_name == "all" {
        // Get all FQNs from extracted_items
        sqlx::query_as::<_, (String,)>("SELECT fqn FROM extracted_items WHERE fqn IS NOT NULL")
            .fetch_all(&mut *conn)
            .await
    } else {
        // Filter by crate name extracted from FQN
        sqlx::query_as::<_, (String,)>(
            "SELECT fqn FROM extracted_items WHERE split_part(fqn, '::', 1) = $1",
        )
        .bind(crate_name)
        .fetch_all(&mut *conn)
        .await
    };

    query
        .map(|rows| rows.into_iter().map(|(fqn,)| fqn).collect())
        .map_err(|e| AppError::Database(format!("Failed to query Postgres FQNs: {}", e)))
}

/// Query Postgres for count of items in a crate.
async fn query_postgres_count(
    state: &AppState,
    crate_name: &str,
    ws: Option<&WorkspaceContext>,
) -> Result<usize, AppError> {
    let mut conn = acquire_conn(&state.pg_pool, ws).await?;

    let result = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM extracted_items WHERE split_part(fqn, '::', 1) = $1",
    )
    .bind(crate_name)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query Postgres count: {}", e)))?;

    Ok(result.0 as usize)
}

/// Query Neo4j for FQNs matching a crate.
async fn query_neo4j_fqns(
    state: &AppState,
    crate_name: &str,
    ws: Option<&WorkspaceContext>,
) -> Result<Vec<String>, AppError> {
    if let Some(ctx) = ws {
        let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ctx.clone());

        let template_name = if crate_name == "all" {
            "consistency_fqns"
        } else {
            "consistency_fqns_filtered"
        };

        let mut params = std::collections::HashMap::new();
        if crate_name != "all" {
            params.insert("crate_name".to_string(), serde_json::json!(crate_name));
        }

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            template_name,
            &params,
            &ctx.workspace_label(),
        )?;

        let results = client.execute_query(&cypher, query_params).await?;
        Ok(results
            .into_iter()
            .filter_map(|r| r.get("fqn").and_then(|v| v.as_str()).map(String::from))
            .collect())
    } else {
        let template_name = if crate_name == "all" {
            "consistency_fqns_system"
        } else {
            "consistency_fqns_filtered_system"
        };

        let mut params = std::collections::HashMap::new();
        if crate_name != "all" {
            params.insert("crate_name".to_string(), serde_json::json!(crate_name));
        }

        let (cypher, query_params) =
            crate::handlers::graph_templates::resolve_system(template_name, &params)?;

        let results = execute_neo4j_query(state, &cypher, query_params).await?; // RUSA-194-EXEMPT: system-level fallback when no workspace context
        Ok(results
            .into_iter()
            .filter_map(|r| r.get("fqn").and_then(|v| v.as_str()).map(String::from))
            .collect())
    }
}

/// Query Neo4j for count of nodes matching a crate.
async fn query_neo4j_count(
    state: &AppState,
    crate_name: &str,
    ws: Option<&WorkspaceContext>,
) -> Result<usize, AppError> {
    if let Some(ctx) = ws {
        let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ctx.clone());

        let mut params = std::collections::HashMap::new();
        params.insert("crate_name".to_string(), serde_json::json!(crate_name));

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            "consistency_count",
            &params,
            &ctx.workspace_label(),
        )?;

        let results = client.execute_query(&cypher, query_params).await?;

        let total: i64 = results
            .iter()
            .filter_map(|r| r.get("count").and_then(|v| v.as_i64()))
            .sum();
        Ok(total as usize)
    } else {
        let mut params = std::collections::HashMap::new();
        params.insert("crate_name".to_string(), serde_json::json!(crate_name));

        let (cypher, query_params) =
            crate::handlers::graph_templates::resolve_system("consistency_count_system", &params)?;

        let results = execute_neo4j_query(state, &cypher, query_params).await?; // RUSA-194-EXEMPT: system-level fallback when no workspace context

        Ok(results
            .first()
            .and_then(|r| r.get("count"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as usize)
    }
}

/// Query Qdrant for FQNs matching a crate.
async fn query_qdrant_fqns(
    state: &AppState,
    crate_name: &str,
    collection_name: &str,
) -> Result<Vec<String>, AppError> {
    let url = format!(
        "{}/collections/{}/points/scroll",
        state.config.qdrant_host, collection_name
    );

    // Build filter for crate_name if specified
    let filter = if crate_name == "all" {
        serde_json::json!(null)
    } else {
        serde_json::json!({
            "must": [
                {
                    "key": "crate_name",
                    "match": { "value": crate_name }
                }
            ]
        })
    };

    let mut all_fqns = Vec::new();
    let mut offset: Option<String> = None;
    let batch_size = 1000u32;

    // Paginate through all points
    loop {
        let body = serde_json::json!({
            "limit": batch_size,
            "with_payload": ["fqn"],
            "filter": filter,
            "offset": offset
        });

        let response = state
            .http_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::Qdrant(format!("Failed to query Qdrant: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::Qdrant(format!(
                "Qdrant scroll failed with status {}: {}",
                status, text
            )));
        }

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

        let points = json["result"]["points"]
            .as_array()
            .ok_or_else(|| AppError::Qdrant("Invalid Qdrant response format".to_string()))?;

        if points.is_empty() {
            break;
        }

        for point in points {
            if let Some(fqn) = point["payload"]["fqn"].as_str() {
                all_fqns.push(fqn.to_string());
            }
        }

        // Check for next page offset
        offset = json["result"]["next_page_offset"]
            .as_str()
            .map(String::from);

        if offset.is_none() {
            break;
        }
    }

    Ok(all_fqns)
}

/// Query Qdrant for count of points matching a crate.
async fn query_qdrant_count(
    state: &AppState,
    crate_name: &str,
    collection_name: &str,
) -> Result<usize, AppError> {
    // Use the count API for faster count-only queries
    let url = format!(
        "{}/collections/{}/points/count",
        state.config.qdrant_host, collection_name
    );

    let filter = if crate_name == "all" {
        serde_json::json!(null)
    } else {
        serde_json::json!({
            "must": [
                {
                    "key": "crate_name",
                    "match": { "value": crate_name }
                }
            ]
        })
    };

    let body = serde_json::json!({
        "filter": filter
    });

    let response = state
        .http_client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to query Qdrant count: {}", e)))?;

    if !response.status().is_success() {
        warn!("Qdrant count endpoint failed, falling back to scroll");
        let fqns = query_qdrant_fqns(state, crate_name, collection_name).await?;
        return Ok(fqns.len());
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant count response: {}", e)))?;

    Ok(json["result"]["count"].as_u64().unwrap_or(0) as usize)
}

/// Get all unique crate names from Postgres.
async fn get_all_crate_names(
    state: &AppState,
    ws: Option<&WorkspaceContext>,
) -> Result<Vec<String>, AppError> {
    let mut conn = acquire_conn(&state.pg_pool, ws).await?;

    let result = sqlx::query_as::<_, (String,)>(
        "SELECT DISTINCT split_part(fqn, '::', 1) as crate_name FROM extracted_items ORDER BY crate_name",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get crate names: {}", e)))?;

    Ok(result.into_iter().map(|(name,)| name).collect())
}

/// Generate a recommendation based on the consistency state.
fn generate_recommendation(counts: &StoreCounts, discrepancies: &Option<Discrepancies>) -> String {
    // Check for complete absence of data
    if counts.postgres == 0 && counts.neo4j == 0 && counts.qdrant == 0 {
        return "No data found. Run ingestion first.".to_string();
    }

    // Check for missing stages
    let mut issues: Vec<String> = Vec::new();

    if counts.neo4j == 0 && counts.postgres > 0 {
        issues.push("Graph stage has not been run".to_string());
    }
    if counts.qdrant == 0 && counts.postgres > 0 {
        issues.push("Embed stage has not been run".to_string());
    }

    // Check for partial data
    if let Some(disc) = discrepancies {
        if !disc.in_postgres_not_neo4j.is_empty() {
            issues.push(format!(
                "{} items missing from Neo4j - re-run Graph stage",
                disc.in_postgres_not_neo4j.len()
            ));
        }
        if !disc.in_postgres_not_qdrant.is_empty() {
            issues.push(format!(
                "{} items missing from Qdrant - re-run Embed stage",
                disc.in_postgres_not_qdrant.len()
            ));
        }
        if !disc.in_neo4j_not_postgres.is_empty() {
            issues.push(format!(
                "{} orphaned nodes in Neo4j - data may need cleanup",
                disc.in_neo4j_not_postgres.len()
            ));
        }
        if !disc.in_qdrant_not_postgres.is_empty() {
            issues.push(format!(
                "{} orphaned points in Qdrant - data may need cleanup",
                disc.in_qdrant_not_postgres.len()
            ));
        }
    } else {
        // Summary mode - give count-based recommendation
        if counts.neo4j < counts.postgres {
            issues.push(
                "Neo4j has fewer items than Postgres - Graph stage may be incomplete".to_string(),
            );
        }
        if counts.qdrant < counts.postgres {
            issues.push(
                "Qdrant has fewer items than Postgres - Embed stage may be incomplete".to_string(),
            );
        }
    }

    if issues.is_empty() {
        "All stores are consistent. No action required.".to_string()
    } else {
        issues.join(". ") + "."
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_counts_serialization() {
        let counts = StoreCounts {
            postgres: 100,
            neo4j: 98,
            qdrant: 100,
        };
        let json = serde_json::to_value(&counts).unwrap();
        assert_eq!(json["postgres"], 100);
        assert_eq!(json["neo4j"], 98);
        assert_eq!(json["qdrant"], 100);
    }

    #[test]
    fn test_discrepancies_serialization() {
        let disc = Discrepancies {
            in_postgres_not_neo4j: vec!["func_a".to_string()],
            in_postgres_not_qdrant: vec![],
            in_neo4j_not_postgres: vec!["func_b".to_string()],
            in_qdrant_not_postgres: vec![],
        };
        let json = serde_json::to_value(&disc).unwrap();
        assert_eq!(json["in_postgres_not_neo4j"], serde_json::json!(["func_a"]));
        assert_eq!(json["in_postgres_not_qdrant"], serde_json::json!([]));
    }

    #[test]
    fn test_generate_recommendation_empty() {
        let counts = StoreCounts {
            postgres: 0,
            neo4j: 0,
            qdrant: 0,
        };
        let rec = generate_recommendation(&counts, &None);
        assert!(rec.contains("No data found"));
    }

    #[test]
    fn test_generate_recommendation_consistent() {
        let counts = StoreCounts {
            postgres: 100,
            neo4j: 100,
            qdrant: 100,
        };
        let rec = generate_recommendation(&counts, &None);
        assert!(rec.contains("consistent"));
    }

    #[test]
    fn test_generate_recommendation_missing_graph() {
        let counts = StoreCounts {
            postgres: 100,
            neo4j: 0,
            qdrant: 100,
        };
        let rec = generate_recommendation(&counts, &None);
        assert!(rec.contains("Graph stage"));
    }

    #[test]
    fn test_generate_recommendation_missing_embed() {
        let counts = StoreCounts {
            postgres: 100,
            neo4j: 100,
            qdrant: 0,
        };
        let rec = generate_recommendation(&counts, &None);
        assert!(rec.contains("Embed stage"));
    }

    #[test]
    fn test_generate_recommendation_with_discrepancies() {
        let counts = StoreCounts {
            postgres: 100,
            neo4j: 98,
            qdrant: 100,
        };
        let disc = Discrepancies {
            in_postgres_not_neo4j: vec!["func_a".to_string(), "func_b".to_string()],
            in_postgres_not_qdrant: vec![],
            in_neo4j_not_postgres: vec![],
            in_qdrant_not_postgres: vec![],
        };
        let rec = generate_recommendation(&counts, &Some(disc));
        assert!(rec.contains("2 items missing from Neo4j"));
        assert!(rec.contains("Graph stage"));
    }

    #[test]
    fn test_consistency_query_deserialization() {
        let json = r#"{"crate": "my_crate", "detail": "full"}"#;
        let query: ConsistencyQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.crate_name, Some("my_crate".to_string()));
        assert_eq!(query.detail, "full");
    }

    #[test]
    fn test_consistency_query_defaults() {
        let json = r#"{}"#;
        let query: ConsistencyQuery = serde_json::from_str(json).unwrap();
        assert!(query.crate_name.is_none());
        assert_eq!(query.detail, "summary");
    }
}
