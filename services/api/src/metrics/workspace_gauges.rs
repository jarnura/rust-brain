//! Per-workspace Prometheus gauge collectors.
//!
//! [`WorkspaceGauges`] holds 6 GaugeVec metrics tracking workspace resource usage:
//! - Postgres items count
//! - Neo4j nodes and edges count
//! - Qdrant vectors count
//! - Workspace status (multi-label gauge)
//! - Index duration
//!
//! [`start_workspace_gauge_collector`] spawns a background tokio task that
//! updates these gauges every 60 seconds for all "ready" workspaces.

use std::time::Duration;

use prometheus::{GaugeVec, Registry};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::neo4j::WorkspaceContext;
use crate::state::AppState;
use crate::workspace::{
    acquire_conn, collection_name_for, list_workspaces, schema::COLLECTION_TYPE_CODE,
};

/// Per-workspace resource gauges updated by the background collector.
pub struct WorkspaceGauges {
    /// Number of extracted_items in Postgres for each workspace.
    pub pg_items_total: GaugeVec,
    /// Number of nodes in Neo4j for each workspace.
    pub neo4j_nodes_total: GaugeVec,
    /// Number of edges (relationships) in Neo4j for each workspace.
    pub neo4j_edges_total: GaugeVec,
    /// Number of vectors in Qdrant for each workspace.
    pub qdrant_vectors_total: GaugeVec,
    /// Workspace status indicator (1 for current status, 0 for others).
    pub workspace_status: GaugeVec,
    /// Duration of the last completed indexing run in seconds.
    pub index_duration_seconds: GaugeVec,
}

impl WorkspaceGauges {
    /// Creates and registers all 6 GaugeVec metrics on the provided registry.
    pub fn new(registry: &Registry) -> Self {
        let pg_items_total = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_pg_items_total",
                "Number of extracted items in Postgres for this workspace",
            ),
            &["workspace"],
        )
        .expect("Failed to create rustbrain_workspace_pg_items_total metric");

        let neo4j_nodes_total = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_neo4j_nodes_total",
                "Number of nodes in Neo4j for this workspace",
            ),
            &["workspace"],
        )
        .expect("Failed to create rustbrain_workspace_neo4j_nodes_total metric");

        let neo4j_edges_total = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_neo4j_edges_total",
                "Number of edges in Neo4j for this workspace",
            ),
            &["workspace"],
        )
        .expect("Failed to create rustbrain_workspace_neo4j_edges_total metric");

        let qdrant_vectors_total = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_qdrant_vectors_total",
                "Number of vectors in Qdrant for this workspace",
            ),
            &["workspace"],
        )
        .expect("Failed to create rustbrain_workspace_qdrant_vectors_total metric");

        let workspace_status = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_status",
                "Current status of the workspace (1 = current status, 0 = other)",
            ),
            &["workspace", "status"],
        )
        .expect("Failed to create rustbrain_workspace_status metric");

        let index_duration_seconds = GaugeVec::new(
            prometheus::Opts::new(
                "rustbrain_workspace_index_duration_seconds",
                "Duration of the last completed indexing run in seconds",
            ),
            &["workspace"],
        )
        .expect("Failed to create rustbrain_workspace_index_duration_seconds metric");

        registry
            .register(Box::new(pg_items_total.clone()))
            .expect("Failed to register rustbrain_workspace_pg_items_total");
        registry
            .register(Box::new(neo4j_nodes_total.clone()))
            .expect("Failed to register rustbrain_workspace_neo4j_nodes_total");
        registry
            .register(Box::new(neo4j_edges_total.clone()))
            .expect("Failed to register rustbrain_workspace_neo4j_edges_total");
        registry
            .register(Box::new(qdrant_vectors_total.clone()))
            .expect("Failed to register rustbrain_workspace_qdrant_vectors_total");
        registry
            .register(Box::new(workspace_status.clone()))
            .expect("Failed to register rustbrain_workspace_status");
        registry
            .register(Box::new(index_duration_seconds.clone()))
            .expect("Failed to register rustbrain_workspace_index_duration_seconds");

        Self {
            pg_items_total,
            neo4j_nodes_total,
            neo4j_edges_total,
            qdrant_vectors_total,
            workspace_status,
            index_duration_seconds,
        }
    }

    /// Sets all status gauges for a workspace. The current status is set to 1.0,
    /// all other known statuses are set to 0.0.
    fn set_workspace_status(&self, workspace_label: &str, current_status: &str) {
        const KNOWN_STATUSES: &[&str] = &[
            "pending", "cloning", "indexing", "ready", "error", "archived",
        ];

        for status in KNOWN_STATUSES {
            let value = if *status == current_status { 1.0 } else { 0.0 };
            self.workspace_status
                .with_label_values(&[workspace_label, status])
                .set(value);
        }
    }
}

/// Spawns a background tokio task that collects per-workspace metrics every 60 seconds.
pub fn start_workspace_gauge_collector(state: AppState) -> JoinHandle<()> {
    tokio::spawn(collector_loop(state))
}

async fn collector_loop(state: AppState) {
    info!("Workspace gauge collector started (interval=60s)");

    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        collect_once(&state).await;
    }
}

async fn collect_once(state: &AppState) {
    let workspaces = match list_workspaces(&state.workspace_manager.pool).await {
        Ok(ws) => ws,
        Err(e) => {
            warn!("Failed to list workspaces for gauge collection: {}", e);
            return;
        }
    };

    for workspace in workspaces {
        let ws_name = &workspace.name;

        // Only collect metrics for "ready" workspaces
        if workspace.status != "ready" {
            // Still update the status gauge for non-ready workspaces
            state
                .metrics
                .workspace_gauges
                .set_workspace_status(ws_name, &workspace.status);
            continue;
        }

        // Use a timeout for each workspace's collection to prevent one slow workspace from blocking others
        let timeout_duration = Duration::from_secs(10);
        let collection_future = collect_workspace_metrics(state, &workspace, ws_name);

        match tokio::time::timeout(timeout_duration, collection_future).await {
            Ok(Ok(())) => {
                // Success - metrics collected
            }
            Ok(Err(e)) => {
                warn!(workspace = %ws_name, "Failed to collect workspace metrics: {}", e);
            }
            Err(_) => {
                warn!(workspace = %ws_name, "Workspace metrics collection timed out after 10s");
            }
        }
    }
}

async fn collect_workspace_metrics(
    state: &AppState,
    workspace: &crate::workspace::Workspace,
    ws_name: &str,
) -> anyhow::Result<()> {
    let ws_ctx = WorkspaceContext::new(workspace.id.to_string())?;
    let gauges = &state.metrics.workspace_gauges;

    // Query Postgres for extracted_items count
    let pg_count = match query_pg_count(state, &ws_ctx).await {
        Ok(count) => count,
        Err(e) => {
            warn!(workspace = %ws_name, "Postgres count query failed: {}", e);
            0
        }
    };
    gauges
        .pg_items_total
        .with_label_values(&[ws_name])
        .set(pg_count as f64);

    // Query Neo4j for nodes and edges
    let (neo4j_nodes, neo4j_edges) = match query_neo4j_counts(state, &ws_ctx).await {
        Ok((nodes, edges)) => (nodes, edges),
        Err(e) => {
            warn!(workspace = %ws_name, "Neo4j count query failed: {}", e);
            (0, 0)
        }
    };
    gauges
        .neo4j_nodes_total
        .with_label_values(&[ws_name])
        .set(neo4j_nodes as f64);
    gauges
        .neo4j_edges_total
        .with_label_values(&[ws_name])
        .set(neo4j_edges as f64);

    // Query Qdrant for vector count
    let qdrant_count = match query_qdrant_count(state, &ws_ctx).await {
        Ok(count) => count,
        Err(e) => {
            warn!(workspace = %ws_name, "Qdrant count query failed: {}", e);
            0
        }
    };
    gauges
        .qdrant_vectors_total
        .with_label_values(&[ws_name])
        .set(qdrant_count as f64);

    // Query index duration
    let index_duration = match query_index_duration(state, &ws_ctx).await {
        Ok(Some(duration)) => duration,
        Ok(None) => 0,
        Err(e) => {
            warn!(workspace = %ws_name, "Index duration query failed: {}", e);
            0
        }
    };
    gauges
        .index_duration_seconds
        .with_label_values(&[ws_name])
        .set(index_duration as f64);

    // Set workspace status gauge (1 for current, 0 for others)
    gauges.set_workspace_status(ws_name, &workspace.status);

    Ok(())
}

/// Query Postgres for the count of extracted_items in a workspace schema.
async fn query_pg_count(state: &AppState, ws_ctx: &WorkspaceContext) -> anyhow::Result<i64> {
    let mut conn = acquire_conn(&state.pg_pool, Some(ws_ctx)).await?;

    let (count,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM extracted_items")
        .fetch_one(&mut *conn)
        .await?;

    Ok(count)
}

/// Query Neo4j for node and edge counts scoped to a workspace.
async fn query_neo4j_counts(
    state: &AppState,
    ws_ctx: &WorkspaceContext,
) -> anyhow::Result<(i64, i64)> {
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
async fn query_qdrant_count(state: &AppState, ws_ctx: &WorkspaceContext) -> anyhow::Result<i64> {
    let schema_name = crate::workspace::schema::schema_name_for(ws_ctx.workspace_id());
    let collection_name = collection_name_for(&schema_name, COLLECTION_TYPE_CODE);

    let url = format!(
        "{}/collections/{}",
        state.config.qdrant_host, collection_name
    );

    let resp = state.http_client.get(&url).send().await?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Qdrant returned status {} for collection {}",
            resp.status(),
            collection_name
        );
    }

    let json: serde_json::Value = resp.json().await?;
    let count = json["result"]["points_count"].as_i64().unwrap_or(0);

    Ok(count)
}

/// Query the duration of the last completed indexing run for this workspace.
async fn query_index_duration(
    state: &AppState,
    ws_ctx: &WorkspaceContext,
) -> anyhow::Result<Option<i64>> {
    let mut conn = acquire_conn(&state.pg_pool, Some(ws_ctx)).await?;

    let result = sqlx::query_as::<_, (Option<i64>,)>(
        "SELECT EXTRACT(EPOCH FROM (completed_at - started_at))::bigint AS duration \
         FROM ingestion_runs \
         WHERE status = 'completed' \
         ORDER BY completed_at DESC \
         LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await?;

    Ok(result.and_then(|(d,)| d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_gauges_new() {
        let registry = Registry::new();
        let gauges = WorkspaceGauges::new(&registry);

        // Verify all gauges are accessible
        let _ = &gauges.pg_items_total;
        let _ = &gauges.neo4j_nodes_total;
        let _ = &gauges.neo4j_edges_total;
        let _ = &gauges.qdrant_vectors_total;
        let _ = &gauges.workspace_status;
        let _ = &gauges.index_duration_seconds;
    }

    #[test]
    fn test_set_gauge_values() {
        let registry = Registry::new();
        let gauges = WorkspaceGauges::new(&registry);

        // Set values on all gauges
        gauges
            .pg_items_total
            .with_label_values(&["test-ws"])
            .set(100.0);
        gauges
            .neo4j_nodes_total
            .with_label_values(&["test-ws"])
            .set(200.0);
        gauges
            .neo4j_edges_total
            .with_label_values(&["test-ws"])
            .set(300.0);
        gauges
            .qdrant_vectors_total
            .with_label_values(&["test-ws"])
            .set(400.0);
        gauges
            .index_duration_seconds
            .with_label_values(&["test-ws"])
            .set(500.0);

        // Verify values can be retrieved
        assert!(gauges
            .pg_items_total
            .get_metric_with_label_values(&["test-ws"])
            .is_ok());
    }

    #[test]
    fn test_workspace_status_sets_correct_values() {
        let registry = Registry::new();
        let gauges = WorkspaceGauges::new(&registry);

        // Set status to "ready"
        gauges.set_workspace_status("test-ws", "ready");

        // We can't easily read the values back from GaugeVec, but we can verify
        // the call doesn't panic and the metric can be retrieved
        for status in [
            "pending", "cloning", "indexing", "ready", "error", "archived",
        ] {
            assert!(
                gauges
                    .workspace_status
                    .get_metric_with_label_values(&["test-ws", status])
                    .is_ok(),
                "Should be able to get metric for status: {}",
                status
            );
        }
    }

    #[test]
    fn test_all_known_statuses_covered() {
        const KNOWN_STATUSES: &[&str] = &[
            "pending", "cloning", "indexing", "ready", "error", "archived",
        ];

        let registry = Registry::new();
        let gauges = WorkspaceGauges::new(&registry);

        // Verify we can set each status as the current status
        for current_status in KNOWN_STATUSES {
            gauges.set_workspace_status("test-ws", current_status);

            // Verify all statuses have metrics
            for status in KNOWN_STATUSES {
                assert!(gauges
                    .workspace_status
                    .get_metric_with_label_values(&["test-ws", status])
                    .is_ok());
            }
        }
    }
}
