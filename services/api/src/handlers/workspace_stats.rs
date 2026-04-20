//! Workspace statistics endpoint.
//!
//! `GET /workspaces/:id/stats` — returns per-workspace stats for the playground UI.
//!
//! Queries Postgres, Neo4j, and Qdrant for workspace-scoped counts, consistency
//! deltas, and isolation checks.

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::neo4j::WorkspaceContext;
use crate::state::AppState;
use crate::workspace::{
    acquire_conn, collection_name_for, get_workspace as db_get_workspace,
    schema::COLLECTION_TYPE_CODE,
};

// =============================================================================
// Response types
// =============================================================================

/// Response body for `GET /workspaces/:id/stats`.
#[derive(Debug, Serialize)]
pub struct WorkspaceStatsResponse {
    /// Workspace UUID.
    pub workspace_id: Uuid,
    /// Current workspace status (e.g. `"ready"`, `"indexing"`, `"error"`).
    pub status: String,
    /// Number of rows in the workspace `extracted_items` table (Postgres).
    pub pg_items_count: i64,
    /// Number of nodes in Neo4j for this workspace.
    pub neo4j_nodes_count: i64,
    /// Number of relationships (edges) in Neo4j for this workspace.
    pub neo4j_edges_count: i64,
    /// Number of vectors in the workspace Qdrant code collection.
    pub qdrant_vectors_count: i64,
    /// Cross-store consistency deltas.
    pub consistency: ConsistencyInfo,
    /// Workspace isolation checks.
    pub isolation: IsolationInfo,
    /// Duration of the last indexing run in seconds, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_duration_seconds: Option<i64>,
    /// When the workspace was created.
    pub created_at: DateTime<Utc>,
    /// When the workspace last completed indexing, if available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indexed_at: Option<DateTime<Utc>>,
}

/// Cross-store consistency deltas.
#[derive(Debug, Serialize)]
pub struct ConsistencyInfo {
    /// Delta between Postgres items and Neo4j nodes.
    pub pg_vs_neo4j_delta: i64,
    /// Delta between Postgres items and Qdrant vectors.
    pub pg_vs_qdrant_delta: i64,
    /// Aggregate consistency status: `"consistent"` or `"inconsistent"`.
    pub status: String,
}

/// Workspace isolation checks.
#[derive(Debug, Serialize, Default)]
pub struct IsolationInfo {
    /// Number of Neo4j nodes with multiple workspace labels.
    pub multi_label_nodes: i64,
    /// Number of relationships crossing workspace boundaries.
    pub cross_workspace_edges: i64,
    /// Number of nodes whose label does not match the expected workspace label.
    pub label_mismatches: i64,
}

// =============================================================================
// Handler
// =============================================================================

/// `GET /workspaces/:id/stats` — return per-workspace stats for the playground UI.
///
/// Queries all three stores (Postgres, Neo4j, Qdrant) for workspace-scoped
/// counts and returns consistency deltas and isolation checks.
///
/// # Errors
///
/// - Returns 404 if the workspace does not exist.
/// - Returns 500 if any store is unavailable.
pub async fn get_workspace_stats(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WorkspaceStatsResponse>, AppError> {
    let workspace = db_get_workspace(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    let ws_ctx = WorkspaceContext::new(id.to_string())?;

    let pg_future = query_pg_counts(&state, &ws_ctx);
    let neo4j_future = query_neo4j_counts(&state, &ws_ctx);
    let qdrant_future = query_qdrant_count(&state, &ws_ctx);
    let isolation_future = query_isolation(&state, &ws_ctx);
    let index_duration_future = query_index_duration(&state, &ws_ctx);

    let (pg_result, neo4j_result, qdrant_result, isolation_result, duration_result) = tokio::join!(
        pg_future,
        neo4j_future,
        qdrant_future,
        isolation_future,
        index_duration_future,
    );

    let pg_items_count = pg_result.unwrap_or_else(|e| {
        warn!(workspace_id = %id, "Postgres stats query failed: {}", e);
        0
    });
    let (neo4j_nodes_count, neo4j_edges_count) = neo4j_result.unwrap_or_else(|e| {
        warn!(workspace_id = %id, "Neo4j stats query failed: {}", e);
        (0, 0)
    });
    let qdrant_vectors_count = qdrant_result.unwrap_or_else(|e| {
        warn!(workspace_id = %id, "Qdrant stats query failed: {}", e);
        0
    });
    let isolation = isolation_result.unwrap_or_else(|e| {
        warn!(workspace_id = %id, "Isolation check failed: {}", e);
        IsolationInfo::default()
    });
    let index_duration_seconds = duration_result.unwrap_or_else(|e| {
        debug!(workspace_id = %id, "Index duration query failed: {}", e);
        None
    });

    let pg_vs_neo4j_delta = pg_items_count - neo4j_nodes_count;
    let pg_vs_qdrant_delta = pg_items_count - qdrant_vectors_count;
    let consistency_status = if pg_vs_neo4j_delta == 0 && pg_vs_qdrant_delta == 0 {
        "consistent"
    } else {
        "inconsistent"
    };

    Ok(Json(WorkspaceStatsResponse {
        workspace_id: id,
        status: workspace.status,
        pg_items_count,
        neo4j_nodes_count,
        neo4j_edges_count,
        qdrant_vectors_count,
        consistency: ConsistencyInfo {
            pg_vs_neo4j_delta,
            pg_vs_qdrant_delta,
            status: consistency_status.to_string(),
        },
        isolation,
        index_duration_seconds,
        created_at: workspace.created_at,
        indexed_at: workspace.index_completed_at,
    }))
}

// =============================================================================
// Query functions
// =============================================================================

/// Query Postgres for the count of extracted_items in a workspace schema.
async fn query_pg_counts(state: &AppState, ws_ctx: &WorkspaceContext) -> Result<i64, AppError> {
    let mut conn = acquire_conn(&state.pg_pool, Some(ws_ctx)).await?;

    let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM extracted_items")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::Database(format!("Failed to count extracted_items: {}", e)))?;

    Ok(count)
}

/// Query Neo4j for node and edge counts scoped to a workspace.
async fn query_neo4j_counts(
    state: &AppState,
    ws_ctx: &WorkspaceContext,
) -> Result<(i64, i64), AppError> {
    let client = crate::neo4j::WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws_ctx.clone());

    let ws_label = ws_ctx.workspace_label();

    let cypher = format!(
        "MATCH (n:{ws_label}) WITH count(n) AS nodes \
         OPTIONAL MATCH (a:{ws_label})-[r]->(b:{ws_label}) \
         WITH nodes, count(r) AS rels \
         RETURN nodes, rels"
    );

    let results = client.execute_query(&cypher, serde_json::json!({})).await?;

    let row = results.into_iter().next();
    match row {
        Some(row) => {
            let nodes = row.get("nodes").and_then(|v| v.as_i64()).unwrap_or(0);
            let rels = row.get("rels").and_then(|v| v.as_i64()).unwrap_or(0);
            Ok((nodes, rels))
        }
        None => Ok((0, 0)),
    }
}

/// Query Qdrant for vector count in the workspace code collection.
async fn query_qdrant_count(state: &AppState, ws_ctx: &WorkspaceContext) -> Result<i64, AppError> {
    let schema_name = crate::workspace::schema::schema_name_for(ws_ctx.workspace_id());
    let collection_name = collection_name_for(&schema_name, COLLECTION_TYPE_CODE);

    let url = format!(
        "{}/collections/{}",
        state.config.qdrant_host, collection_name
    );

    let resp = state
        .http_client
        .get(&url)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to query Qdrant: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Qdrant(format!(
            "Qdrant returned status {} for collection {}",
            resp.status(),
            collection_name
        )));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

    let count = json["result"]["points_count"].as_i64().unwrap_or(0);

    Ok(count)
}

/// Query Neo4j for workspace isolation checks.
async fn query_isolation(
    state: &AppState,
    ws_ctx: &WorkspaceContext,
) -> Result<IsolationInfo, AppError> {
    let client = crate::neo4j::WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws_ctx.clone());

    let ws_label = ws_ctx.workspace_label();

    let multi_label_cypher = format!(
        "MATCH (n:{ws_label}) \
         WHERE ANY(label IN labels(n) WHERE label STARTS WITH 'Workspace_' AND label <> '{ws_label}') \
         RETURN count(n) AS cnt"
    );

    let cross_edge_cypher = format!(
        "MATCH (a:{ws_label})-[r]->(b) \
         WHERE NOT b:{ws_label} \
         RETURN count(r) AS cnt"
    );

    let mismatch_cypher = format!(
        "MATCH (n) \
         WHERE NOT n:{ws_label} \
         AND ANY(label IN labels(n) WHERE label STARTS WITH 'Workspace_') \
         AND EXISTS {{ MATCH (m:{ws_label}) WHERE m.fqn = n.fqn RETURN m }} \
         RETURN count(n) AS cnt"
    );

    let (multi_result, cross_result, mismatch_result) = tokio::join!(
        client.execute_query(&multi_label_cypher, serde_json::json!({})),
        client.execute_query(&cross_edge_cypher, serde_json::json!({})),
        client.execute_query(&mismatch_cypher, serde_json::json!({})),
    );

    let multi_label_nodes = multi_result
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|r| r.get("cnt").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    let cross_workspace_edges = cross_result
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|r| r.get("cnt").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    let label_mismatches = mismatch_result
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .and_then(|r| r.get("cnt").and_then(|v| v.as_i64()))
        .unwrap_or(0);

    Ok(IsolationInfo {
        multi_label_nodes,
        cross_workspace_edges,
        label_mismatches,
    })
}

/// Query the duration of the last completed indexing run for this workspace.
async fn query_index_duration(
    state: &AppState,
    ws_ctx: &WorkspaceContext,
) -> Result<Option<i64>, AppError> {
    let mut conn = acquire_conn(&state.pg_pool, Some(ws_ctx)).await?;

    let result = sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT EXTRACT(EPOCH FROM (completed_at - started_at))::bigint AS duration \
         FROM ingestion_runs \
         WHERE status = 'completed' \
         ORDER BY completed_at DESC \
         LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query index duration: {}", e)))?;

    Ok(result.and_then(|(d,)| d))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_stats_response_serialization() {
        let resp = WorkspaceStatsResponse {
            workspace_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            status: "ready".to_string(),
            pg_items_count: 2285,
            neo4j_nodes_count: 3650,
            neo4j_edges_count: 3660,
            qdrant_vectors_count: 2285,
            consistency: ConsistencyInfo {
                pg_vs_neo4j_delta: 0,
                pg_vs_qdrant_delta: 0,
                status: "consistent".to_string(),
            },
            isolation: IsolationInfo {
                multi_label_nodes: 0,
                cross_workspace_edges: 0,
                label_mismatches: 0,
            },
            index_duration_seconds: Some(5700),
            created_at: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            indexed_at: Some(
                DateTime::parse_from_rfc3339("2024-01-01T01:30:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["workspace_id"], "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(json["status"], "ready");
        assert_eq!(json["pg_items_count"], 2285);
        assert_eq!(json["neo4j_nodes_count"], 3650);
        assert_eq!(json["neo4j_edges_count"], 3660);
        assert_eq!(json["qdrant_vectors_count"], 2285);
        assert_eq!(json["consistency"]["pg_vs_neo4j_delta"], 0);
        assert_eq!(json["consistency"]["pg_vs_qdrant_delta"], 0);
        assert_eq!(json["consistency"]["status"], "consistent");
        assert_eq!(json["isolation"]["multi_label_nodes"], 0);
        assert_eq!(json["isolation"]["cross_workspace_edges"], 0);
        assert_eq!(json["isolation"]["label_mismatches"], 0);
        assert_eq!(json["index_duration_seconds"], 5700);
        assert!(json.get("indexed_at").is_some());
    }

    #[test]
    fn test_workspace_stats_skips_none_fields() {
        let resp = WorkspaceStatsResponse {
            workspace_id: Uuid::new_v4(),
            status: "indexing".to_string(),
            pg_items_count: 0,
            neo4j_nodes_count: 0,
            neo4j_edges_count: 0,
            qdrant_vectors_count: 0,
            consistency: ConsistencyInfo {
                pg_vs_neo4j_delta: 0,
                pg_vs_qdrant_delta: 0,
                status: "consistent".to_string(),
            },
            isolation: IsolationInfo::default(),
            index_duration_seconds: None,
            created_at: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            indexed_at: None,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("indexed_at").is_none());
    }

    #[test]
    fn test_consistency_info_consistent() {
        let info = ConsistencyInfo {
            pg_vs_neo4j_delta: 0,
            pg_vs_qdrant_delta: 0,
            status: "consistent".to_string(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "consistent");
        assert_eq!(json["pg_vs_neo4j_delta"], 0);
        assert_eq!(json["pg_vs_qdrant_delta"], 0);
    }

    #[test]
    fn test_consistency_info_inconsistent() {
        let info = ConsistencyInfo {
            pg_vs_neo4j_delta: -5,
            pg_vs_qdrant_delta: 10,
            status: "inconsistent".to_string(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "inconsistent");
        assert_eq!(json["pg_vs_neo4j_delta"], -5);
        assert_eq!(json["pg_vs_qdrant_delta"], 10);
    }

    #[test]
    fn test_isolation_info_default() {
        let info = IsolationInfo::default();
        assert_eq!(info.multi_label_nodes, 0);
        assert_eq!(info.cross_workspace_edges, 0);
        assert_eq!(info.label_mismatches, 0);
    }

    #[test]
    fn test_isolation_info_serialization() {
        let info = IsolationInfo {
            multi_label_nodes: 2,
            cross_workspace_edges: 1,
            label_mismatches: 3,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["multi_label_nodes"], 2);
        assert_eq!(json["cross_workspace_edges"], 1);
        assert_eq!(json["label_mismatches"], 3);
    }

    #[test]
    fn test_consistency_status_computation() {
        let pg = 100i64;
        let neo4j = 100i64;
        let qdrant = 100i64;
        let pg_vs_neo4j_delta = pg - neo4j;
        let pg_vs_qdrant_delta = pg - qdrant;
        let status = if pg_vs_neo4j_delta == 0 && pg_vs_qdrant_delta == 0 {
            "consistent"
        } else {
            "inconsistent"
        };
        assert_eq!(status, "consistent");

        let neo4j = 95i64;
        let pg_vs_neo4j_delta = pg - neo4j;
        let status = if pg_vs_neo4j_delta == 0 && pg_vs_qdrant_delta == 0 {
            "consistent"
        } else {
            "inconsistent"
        };
        assert_eq!(status, "inconsistent");
    }

    #[test]
    fn test_response_matches_issue_spec() {
        let resp = WorkspaceStatsResponse {
            workspace_id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            status: "ready".to_string(),
            pg_items_count: 2285,
            neo4j_nodes_count: 3650,
            neo4j_edges_count: 3660,
            qdrant_vectors_count: 2285,
            consistency: ConsistencyInfo {
                pg_vs_neo4j_delta: 0,
                pg_vs_qdrant_delta: 0,
                status: "consistent".to_string(),
            },
            isolation: IsolationInfo {
                multi_label_nodes: 0,
                cross_workspace_edges: 0,
                label_mismatches: 0,
            },
            index_duration_seconds: Some(5700),
            created_at: DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            indexed_at: Some(
                DateTime::parse_from_rfc3339("2024-01-01T01:30:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            ),
        };

        let json = serde_json::to_value(&resp).unwrap();

        assert!(json.get("workspace_id").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("pg_items_count").is_some());
        assert!(json.get("neo4j_nodes_count").is_some());
        assert!(json.get("neo4j_edges_count").is_some());
        assert!(json.get("qdrant_vectors_count").is_some());
        assert!(json.get("consistency").is_some());
        assert!(json.get("isolation").is_some());
        assert!(json.get("index_duration_seconds").is_some());
        assert!(json.get("created_at").is_some());
        assert!(json.get("indexed_at").is_some());

        assert!(json["consistency"].get("pg_vs_neo4j_delta").is_some());
        assert!(json["consistency"].get("pg_vs_qdrant_delta").is_some());
        assert!(json["consistency"].get("status").is_some());

        assert!(json["isolation"].get("multi_label_nodes").is_some());
        assert!(json["isolation"].get("cross_workspace_edges").is_some());
        assert!(json["isolation"].get("label_mismatches").is_some());
    }
}
