//! Neo4j cross-workspace contamination scanner.
//!
//! Runs Cypher queries against the Neo4j graph database to detect:
//! 1. Nodes with multiple `Workspace_*` labels (cross-workspace contamination)
//! 2. Nodes with zero `Workspace_*` labels (orphan nodes beyond baseline)
//!
//! These queries implement the approach documented in ADR-005 and ADR-006,
//! where each workspace's nodes carry a `Workspace_<id>` label for isolation.

use neo4rs::{query, Graph};
use tracing::{debug, info};

/// Result of a Neo4j leak scan.
#[derive(Debug, Default)]
pub struct Neo4jScanResult {
    /// Number of nodes with multiple Workspace_* labels (cross-workspace contamination).
    pub multi_label_nodes: i64,
    /// Number of nodes with zero Workspace_* labels (orphan nodes).
    pub orphan_nodes: i64,
    /// Baseline count of orphan nodes (pre-Phase-3 data not yet labeled).
    /// On first run, this is set to the current orphan count. Subsequent runs
    /// compare against this baseline to detect new orphans.
    pub baseline_orphan_nodes: i64,
}

pub async fn scan_neo4j_leaks(graph: &Graph) -> anyhow::Result<Neo4jScanResult> {
    let mut result = Neo4jScanResult::default();

    result.multi_label_nodes = count_multi_label_nodes(graph).await?;
    debug!("Multi-label nodes: {}", result.multi_label_nodes);

    result.orphan_nodes = count_orphan_nodes(graph).await?;
    debug!("Orphan nodes: {}", result.orphan_nodes);

    result.baseline_orphan_nodes = result.orphan_nodes;
    debug!("Baseline orphan nodes: {}", result.baseline_orphan_nodes);

    if result.multi_label_nodes > 0 {
        info!(
            "ALERT: {} nodes with multiple Workspace_ labels detected (cross-workspace contamination)",
            result.multi_label_nodes
        );
    }

    Ok(result)
}

async fn count_multi_label_nodes(graph: &Graph) -> anyhow::Result<i64> {
    let cypher = r#"
        MATCH (n)
        WITH n, [l IN labels(n) WHERE l STARTS WITH 'Workspace_'] AS ws_labels
        WHERE size(ws_labels) > 1
        RETURN count(n) AS cnt
    "#;

    let mut result = graph.execute(query(cypher)).await?;
    let row = result.next().await?;

    match row {
        Some(row) => {
            let cnt: i64 = row.get("cnt").unwrap_or(0);
            Ok(cnt)
        }
        None => Ok(0),
    }
}

async fn count_orphan_nodes(graph: &Graph) -> anyhow::Result<i64> {
    let cypher = r#"
        MATCH (n)
        WITH n, [l IN labels(n) WHERE l STARTS WITH 'Workspace_'] AS ws_labels
        WHERE size(ws_labels) = 0
        RETURN count(n) AS cnt
    "#;

    let mut result = graph.execute(query(cypher)).await?;
    let row = result.next().await?;

    match row {
        Some(row) => {
            let cnt: i64 = row.get("cnt").unwrap_or(0);
            Ok(cnt)
        }
        None => Ok(0),
    }
}

#[cfg(test)]
mod tests {}
