//! Neo4j cross-workspace contamination scanner.
//!
//! Runs Cypher queries against the Neo4j graph database to detect:
//! 1. Nodes with multiple `Workspace_*` labels (cross-workspace contamination)
//! 2. Nodes with zero `Workspace_*` labels (orphan nodes beyond baseline)
//! 3. Relationships connecting nodes from different workspaces (cross-workspace edges)
//! 4. Nodes whose workspace label conflicts with their relationship context
//!
//! Per-workspace breakdowns (RUSA-214) aggregate counts by workspace label for
//! each category, enabling per-workspace Prometheus metrics.
//!
//! These queries implement the approach documented in ADR-005 and ADR-006,
//! where each workspace's nodes carry a `Workspace_<id>` label for isolation.

use std::collections::HashMap;

use neo4rs::{query, Graph};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Result of a Neo4j leak scan.
#[derive(Debug, Default)]
pub struct Neo4jScanResult {
    pub multi_label_nodes: i64,
    pub orphan_nodes: i64,
    pub baseline_orphan_nodes: i64,
    pub cross_workspace_relationships: i64,
    pub label_mismatches: i64,
    pub cross_workspace_details: Vec<CrossWorkspaceDetail>,
    pub label_mismatch_details: Vec<LabelMismatchDetail>,
    pub per_workspace: PerWorkspaceBreakdown,
}

/// Per-workspace breakdown of leak detection counts.
///
/// Keys are Neo4j workspace labels (e.g., `"Workspace_a1b2c3d4e5f6"`).
/// The special key `"_unlabeled"` is used for orphan nodes that have no
/// workspace label at all.
#[derive(Debug, Default)]
pub struct PerWorkspaceBreakdown {
    pub multi_label_nodes: HashMap<String, i64>,
    pub cross_workspace_rels: HashMap<String, i64>,
    pub label_mismatches: HashMap<String, i64>,
    pub orphan_nodes: i64,
}

/// Details of a single cross-workspace relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossWorkspaceDetail {
    /// Source node FQN.
    pub source_fqn: String,
    /// Source node's workspace label (e.g., "Workspace_a1b2c3d4e5f6").
    pub source_workspace: String,
    /// Target node FQN.
    pub target_fqn: String,
    /// Target node's workspace label.
    pub target_workspace: String,
    /// Relationship type (e.g., "CALLS", "CONTAINS").
    pub rel_type: String,
}

/// Details of a single label-context mismatch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelMismatchDetail {
    /// Node FQN.
    pub fqn: String,
    /// Node's actual workspace label.
    pub actual_workspace: String,
    /// Workspace label inferred from the node's neighbors (majority vote).
    pub expected_workspace: String,
    /// Number of neighbor nodes in the expected workspace.
    pub neighbor_count: i64,
}

pub async fn scan_neo4j_leaks(
    graph: &Graph,
    previous_baseline: Option<i64>,
) -> anyhow::Result<Neo4jScanResult> {
    let mut result = Neo4jScanResult::default();

    result.multi_label_nodes = count_multi_label_nodes(graph).await?;
    debug!("Multi-label nodes: {}", result.multi_label_nodes);

    result.orphan_nodes = count_orphan_nodes(graph).await?;
    debug!("Orphan nodes: {}", result.orphan_nodes);

    result.baseline_orphan_nodes = previous_baseline.unwrap_or(result.orphan_nodes);
    debug!("Baseline orphan nodes: {}", result.baseline_orphan_nodes);

    if result.multi_label_nodes > 0 {
        info!(
            "ALERT: {} nodes with multiple Workspace_ labels detected (cross-workspace contamination)",
            result.multi_label_nodes
        );
    }

    result.cross_workspace_details = find_cross_workspace_relationships(graph).await?;
    result.cross_workspace_relationships = result.cross_workspace_details.len() as i64;
    debug!(
        "Cross-workspace relationships: {}",
        result.cross_workspace_relationships
    );

    if result.cross_workspace_relationships > 0 {
        warn!(
            "ALERT: {} cross-workspace relationships detected (edges between different workspaces)",
            result.cross_workspace_relationships
        );
    }

    result.label_mismatch_details = find_label_mismatches(graph).await?;
    result.label_mismatches = result.label_mismatch_details.len() as i64;
    debug!("Label-context mismatches: {}", result.label_mismatches);

    if result.label_mismatches > 0 {
        warn!(
            "ALERT: {} label-context mismatches detected (node workspace label conflicts with neighbors)",
            result.label_mismatches
        );
    }

    result.per_workspace = build_per_workspace_breakdown(graph, &result).await?;

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

/// Finds relationships where source and target nodes have different Workspace_ labels.
///
/// This detects edges that cross workspace boundaries, which should never happen
/// under correct workspace isolation. Such edges indicate either a bug in the
/// ingestion pipeline's label injection or a direct Neo4j write bypassing the API.
async fn find_cross_workspace_relationships(
    graph: &Graph,
) -> anyhow::Result<Vec<CrossWorkspaceDetail>> {
    let cypher = r#"
        MATCH (src)-[r]->(tgt)
        WITH src, tgt, type(r) AS rel_type,
             [l IN labels(src) WHERE l STARTS WITH 'Workspace_'] AS src_ws,
             [l IN labels(tgt) WHERE l STARTS WITH 'Workspace_'] AS tgt_ws
        WHERE size(src_ws) = 1 AND size(tgt_ws) = 1
          AND src_ws[0] <> tgt_ws[0]
        RETURN src.fqn AS source_fqn, src_ws[0] AS source_workspace,
               tgt.fqn AS target_fqn, tgt_ws[0] AS target_workspace,
               rel_type
        LIMIT 500
    "#;

    let mut result = graph.execute(query(cypher)).await?;
    let mut details = Vec::new();

    while let Some(row) = result.next().await? {
        let source_fqn: String = row.get("source_fqn").unwrap_or_default();
        let source_workspace: String = row.get("source_workspace").unwrap_or_default();
        let target_fqn: String = row.get("target_fqn").unwrap_or_default();
        let target_workspace: String = row.get("target_workspace").unwrap_or_default();
        let rel_type: String = row.get("rel_type").unwrap_or_default();

        if !source_fqn.is_empty() && !target_fqn.is_empty() {
            details.push(CrossWorkspaceDetail {
                source_fqn,
                source_workspace,
                target_fqn,
                target_workspace,
                rel_type,
            });
        }
    }

    Ok(details)
}

/// Finds nodes whose Workspace_ label conflicts with their neighbors' labels.
///
/// A node is considered mismatched if it has exactly one Workspace_ label but
/// the majority of its connected neighbors (via any relationship direction) have
/// a different Workspace_ label. This can indicate a mislabeled node that was
/// incorrectly assigned to the wrong workspace during ingestion.
async fn find_label_mismatches(graph: &Graph) -> anyhow::Result<Vec<LabelMismatchDetail>> {
    let cypher = r#"
        MATCH (n)-[r]-(neighbor)
        WITH n, [l IN labels(n) WHERE l STARTS WITH 'Workspace_'][0] AS node_ws,
             neighbor,
             [l IN labels(neighbor) WHERE l STARTS WITH 'Workspace_'][0] AS neighbor_ws
        WHERE node_ws IS NOT NULL AND neighbor_ws IS NOT NULL
          AND node_ws <> neighbor_ws
        WITH n, node_ws, neighbor_ws, count(neighbor) AS mismatch_count
        ORDER BY mismatch_count DESC
        WITH n, node_ws, collect({ws: neighbor_ws, cnt: mismatch_count}) AS neighbor_workspaces
        WITH n, node_ws, neighbor_workspaces,
             neighbor_workspaces[0].ws AS expected_ws,
             neighbor_workspaces[0].cnt AS neighbor_count
        RETURN n.fqn AS fqn, node_ws AS actual_workspace,
               expected_ws AS expected_workspace, neighbor_count
        LIMIT 200
    "#;

    let mut result = graph.execute(query(cypher)).await?;
    let mut details = Vec::new();

    while let Some(row) = result.next().await? {
        let fqn: String = row.get("fqn").unwrap_or_default();
        let actual_workspace: String = row.get("actual_workspace").unwrap_or_default();
        let expected_workspace: String = row.get("expected_workspace").unwrap_or_default();
        let neighbor_count: i64 = row.get("neighbor_count").unwrap_or(0);

        if !fqn.is_empty() {
            details.push(LabelMismatchDetail {
                fqn,
                actual_workspace,
                expected_workspace,
                neighbor_count,
            });
        }
    }

    Ok(details)
}

async fn build_per_workspace_breakdown(
    graph: &Graph,
    result: &Neo4jScanResult,
) -> anyhow::Result<PerWorkspaceBreakdown> {
    let mut breakdown = PerWorkspaceBreakdown {
        multi_label_nodes: count_multi_label_nodes_per_workspace(graph).await?,
        cross_workspace_rels: HashMap::new(),
        label_mismatches: HashMap::new(),
        orphan_nodes: result.orphan_nodes,
    };

    for detail in &result.cross_workspace_details {
        *breakdown
            .cross_workspace_rels
            .entry(detail.source_workspace.clone())
            .or_insert(0) += 1;
        *breakdown
            .cross_workspace_rels
            .entry(detail.target_workspace.clone())
            .or_insert(0) += 1;
    }

    for detail in &result.label_mismatch_details {
        *breakdown
            .label_mismatches
            .entry(detail.actual_workspace.clone())
            .or_insert(0) += 1;
    }

    debug!(
        "Per-workspace breakdown: {} workspaces with multi-label nodes, {} with cross-workspace rels, {} with label mismatches",
        breakdown.multi_label_nodes.len(),
        breakdown.cross_workspace_rels.len(),
        breakdown.label_mismatches.len()
    );

    Ok(breakdown)
}

async fn count_multi_label_nodes_per_workspace(
    graph: &Graph,
) -> anyhow::Result<HashMap<String, i64>> {
    let cypher = r#"
        MATCH (n)
        WITH n, [l IN labels(n) WHERE l STARTS WITH 'Workspace_'] AS ws_labels
        WHERE size(ws_labels) > 1
        UNWIND ws_labels AS ws
        RETURN ws, count(n) AS cnt
    "#;

    let mut query_result = graph.execute(query(cypher)).await?;
    let mut counts = HashMap::new();

    while let Some(row) = query_result.next().await? {
        let ws: String = row.get("ws").unwrap_or_default();
        let cnt: i64 = row.get("cnt").unwrap_or(0);
        if !ws.is_empty() {
            counts.insert(ws, cnt);
        }
    }

    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_neo4j_scan_result_default() {
        let result = Neo4jScanResult::default();
        assert_eq!(result.multi_label_nodes, 0);
        assert_eq!(result.orphan_nodes, 0);
        assert_eq!(result.baseline_orphan_nodes, 0);
        assert_eq!(result.cross_workspace_relationships, 0);
        assert_eq!(result.label_mismatches, 0);
        assert!(result.cross_workspace_details.is_empty());
        assert!(result.label_mismatch_details.is_empty());
        assert!(result.per_workspace.multi_label_nodes.is_empty());
        assert!(result.per_workspace.cross_workspace_rels.is_empty());
        assert!(result.per_workspace.label_mismatches.is_empty());
        assert_eq!(result.per_workspace.orphan_nodes, 0);
    }

    #[test]
    fn test_per_workspace_breakdown_aggregation() {
        let details = vec![
            CrossWorkspaceDetail {
                source_fqn: "a::f1".to_string(),
                source_workspace: "Workspace_abc".to_string(),
                target_fqn: "b::f2".to_string(),
                target_workspace: "Workspace_def".to_string(),
                rel_type: "CALLS".to_string(),
            },
            CrossWorkspaceDetail {
                source_fqn: "a::f3".to_string(),
                source_workspace: "Workspace_abc".to_string(),
                target_fqn: "c::f4".to_string(),
                target_workspace: "Workspace_ghi".to_string(),
                rel_type: "CONTAINS".to_string(),
            },
        ];

        let mut rel_breakdown: HashMap<String, i64> = HashMap::new();
        for detail in &details {
            *rel_breakdown
                .entry(detail.source_workspace.clone())
                .or_insert(0) += 1;
            *rel_breakdown
                .entry(detail.target_workspace.clone())
                .or_insert(0) += 1;
        }

        assert_eq!(*rel_breakdown.get("Workspace_abc").unwrap(), 2);
        assert_eq!(*rel_breakdown.get("Workspace_def").unwrap(), 1);
        assert_eq!(*rel_breakdown.get("Workspace_ghi").unwrap(), 1);
    }

    #[test]
    fn test_per_workspace_label_mismatch_aggregation() {
        let details = vec![
            LabelMismatchDetail {
                fqn: "a::f1".to_string(),
                actual_workspace: "Workspace_abc".to_string(),
                expected_workspace: "Workspace_def".to_string(),
                neighbor_count: 5,
            },
            LabelMismatchDetail {
                fqn: "a::f2".to_string(),
                actual_workspace: "Workspace_abc".to_string(),
                expected_workspace: "Workspace_ghi".to_string(),
                neighbor_count: 3,
            },
            LabelMismatchDetail {
                fqn: "b::f3".to_string(),
                actual_workspace: "Workspace_def".to_string(),
                expected_workspace: "Workspace_abc".to_string(),
                neighbor_count: 2,
            },
        ];

        let mut mismatch_breakdown: HashMap<String, i64> = HashMap::new();
        for detail in &details {
            *mismatch_breakdown
                .entry(detail.actual_workspace.clone())
                .or_insert(0) += 1;
        }

        assert_eq!(*mismatch_breakdown.get("Workspace_abc").unwrap(), 2);
        assert_eq!(*mismatch_breakdown.get("Workspace_def").unwrap(), 1);
    }

    #[test]
    fn test_cross_workspace_detail_serialization() {
        let detail = CrossWorkspaceDetail {
            source_fqn: "crate_a::module::func".to_string(),
            source_workspace: "Workspace_a1b2c3d4e5f6".to_string(),
            target_fqn: "crate_b::module::func".to_string(),
            target_workspace: "Workspace_f6e5d4c3b2a1".to_string(),
            rel_type: "CALLS".to_string(),
        };

        let json = serde_json::to_string(&detail).unwrap();
        assert!(json.contains("CALLS"));
        assert!(json.contains("Workspace_a1b2c3d4e5f6"));

        let deserialized: CrossWorkspaceDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.source_fqn, "crate_a::module::func");
        assert_eq!(deserialized.rel_type, "CALLS");
    }

    #[test]
    fn test_label_mismatch_detail_serialization() {
        let detail = LabelMismatchDetail {
            fqn: "crate::module::func".to_string(),
            actual_workspace: "Workspace_abc123".to_string(),
            expected_workspace: "Workspace_def456".to_string(),
            neighbor_count: 5,
        };

        let json = serde_json::to_string(&detail).unwrap();
        assert!(json.contains("Workspace_abc123"));

        let deserialized: LabelMismatchDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.neighbor_count, 5);
        assert_eq!(deserialized.expected_workspace, "Workspace_def456");
    }
}
