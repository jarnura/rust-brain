//! BFS call-graph traversal over Neo4j CALLS / CALL_INSTANTIATES edges.
//!
//! # Provenance mapping
//!
//! | Neo4j edge type   | CALLS.dispatch property | Provenance    |
//! |-------------------|-------------------------|---------------|
//! | CALLS             | "dynamic"               | dyn_candidate |
//! | CALLS             | any, non-empty types    | monomorph     |
//! | CALLS             | "static" / absent       | direct        |
//! | CALL_INSTANTIATES | —                       | monomorph     |
//!
//! # Multi-tenancy
//!
//! Every Cypher pattern matches against the composite workspace label
//! (e.g., `Workspace_550e8400e29b`) injected at query time.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use anyhow::{Context, Result};
use neo4rs::{query, Graph};
use tracing::debug;

use crate::{
    cursor,
    types::{EdgeProvenance, TraversalEdge, TraversalNode, TraversalOptions, TraversalResult},
};

/// Entry point for caller/callee graph traversal.
pub struct CallGraphTraverser {
    graph: Arc<Graph>,
    /// Neo4j composite workspace label, e.g. `Workspace_550e8400e29b`.
    workspace_label: String,
}

impl CallGraphTraverser {
    pub fn new(graph: Arc<Graph>, workspace_label: String) -> Self {
        Self {
            graph,
            workspace_label,
        }
    }

    /// Traverse callers of `root_fqn` via backward BFS over CALLS / CALL_INSTANTIATES.
    pub async fn get_callers(
        &self,
        root_fqn: &str,
        opts: TraversalOptions,
    ) -> Result<TraversalResult> {
        let opts = opts.clamp();
        let offset = cursor::decode(opts.cursor.as_deref())
            .context("invalid cursor in get_callers")?;

        let root = self.fetch_node(root_fqn).await?;

        let (edges, cycles_detected) = self
            .bfs_callers(root_fqn, opts.depth, opts.limit + offset)
            .await?;

        build_result(root, edges, cycles_detected, offset, opts.limit)
    }

    /// Traverse callees of `root_fqn` via forward BFS over CALLS / CALL_INSTANTIATES.
    pub async fn get_callees(
        &self,
        root_fqn: &str,
        opts: TraversalOptions,
    ) -> Result<TraversalResult> {
        let opts = opts.clamp();
        let offset = cursor::decode(opts.cursor.as_deref())
            .context("invalid cursor in get_callees")?;

        let root = self.fetch_node(root_fqn).await?;

        let (edges, cycles_detected) = self
            .bfs_callees(root_fqn, opts.depth, opts.limit + offset)
            .await?;

        build_result(root, edges, cycles_detected, offset, opts.limit)
    }

    // ── internals ─────────────────────────────────────────────────────────────

    /// Fetch the root node's metadata from Neo4j.
    async fn fetch_node(&self, fqn: &str) -> Result<TraversalNode> {
        let ws = &self.workspace_label;
        let cypher = format!(
            "MATCH (n:{ws}) WHERE n.fqn = $fqn \
             RETURN n.fqn AS fqn, n.name AS name, \
             labels(n)[0] AS kind, n.file_path AS file_path, n.start_line AS line \
             LIMIT 1"
        );

        let mut result = self
            .graph
            .execute(query(&cypher).param("fqn", fqn))
            .await
            .context("fetch_node query failed")?;

        if let Some(row) = result
            .next()
            .await
            .context("fetch_node row fetch failed")?
        {
            Ok(TraversalNode {
                fqn: row.get::<String>("fqn").unwrap_or_else(|_| fqn.to_string()),
                name: row
                    .get::<String>("name")
                    .unwrap_or_else(|_| fqn.split("::").last().unwrap_or(fqn).to_string()),
                kind: row.get::<String>("kind").ok(),
                file_path: row.get::<String>("file_path").ok(),
                line: row.get::<i64>("line").ok().map(|l| l as u32),
            })
        } else {
            // Root might not be in the graph yet; return minimal stub
            Ok(TraversalNode {
                fqn: fqn.to_string(),
                name: fqn.split("::").last().unwrap_or(fqn).to_string(),
                kind: None,
                file_path: None,
                line: None,
            })
        }
    }

    /// BFS backward: find all callers of `root_fqn` up to `max_depth` hops.
    ///
    /// Returns collected edges (in BFS order) and whether any cycle was detected.
    async fn bfs_callers(
        &self,
        root_fqn: &str,
        max_depth: u32,
        max_edges: usize,
    ) -> Result<(Vec<TraversalEdge>, bool)> {
        let ws = &self.workspace_label;
        let mut edges: Vec<TraversalEdge> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut cycles_detected = false;

        visited.insert(root_fqn.to_string());

        // BFS queue: (fqn, depth)
        let mut frontier: VecDeque<(String, u32)> = VecDeque::new();
        frontier.push_back((root_fqn.to_string(), 0));

        while let Some((current_fqn, depth)) = frontier.pop_front() {
            if depth >= max_depth || edges.len() >= max_edges {
                break;
            }

            let callers = self
                .query_callers_of(ws, &current_fqn)
                .await
                .with_context(|| format!("query_callers_of({current_fqn}) failed"))?;

            for (caller_node, provenance) in callers {
                if visited.contains(&caller_node.fqn) {
                    cycles_detected = true;
                    continue;
                }
                visited.insert(caller_node.fqn.clone());

                edges.push(TraversalEdge {
                    from_fqn: caller_node.fqn.clone(),
                    to_fqn: current_fqn.clone(),
                    depth: depth + 1,
                    provenance,
                });

                if edges.len() < max_edges {
                    frontier.push_back((caller_node.fqn, depth + 1));
                }
            }
        }

        Ok((edges, cycles_detected))
    }

    /// BFS forward: find all callees of `root_fqn` up to `max_depth` hops.
    async fn bfs_callees(
        &self,
        root_fqn: &str,
        max_depth: u32,
        max_edges: usize,
    ) -> Result<(Vec<TraversalEdge>, bool)> {
        let ws = &self.workspace_label;
        let mut edges: Vec<TraversalEdge> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut cycles_detected = false;

        visited.insert(root_fqn.to_string());

        let mut frontier: VecDeque<(String, u32)> = VecDeque::new();
        frontier.push_back((root_fqn.to_string(), 0));

        while let Some((current_fqn, depth)) = frontier.pop_front() {
            if depth >= max_depth || edges.len() >= max_edges {
                break;
            }

            let callees = self
                .query_callees_of(ws, &current_fqn)
                .await
                .with_context(|| format!("query_callees_of({current_fqn}) failed"))?;

            for (callee_node, provenance) in callees {
                if visited.contains(&callee_node.fqn) {
                    cycles_detected = true;
                    continue;
                }
                visited.insert(callee_node.fqn.clone());

                edges.push(TraversalEdge {
                    from_fqn: current_fqn.clone(),
                    to_fqn: callee_node.fqn.clone(),
                    depth: depth + 1,
                    provenance,
                });

                if edges.len() < max_edges {
                    frontier.push_back((callee_node.fqn, depth + 1));
                }
            }
        }

        Ok((edges, cycles_detected))
    }

    /// Single-level backward neighbor query: nodes that have CALLS or
    /// CALL_INSTANTIATES edges pointing TO `target_fqn`.
    async fn query_callers_of(
        &self,
        ws: &str,
        target_fqn: &str,
    ) -> Result<Vec<(TraversalNode, EdgeProvenance)>> {
        // Query CALLS edges
        let calls_cypher = format!(
            "MATCH (caller:{ws})-[r:CALLS]->(target:{ws} {{fqn: $fqn}}) \
             RETURN caller.fqn AS fqn, caller.name AS name, \
             labels(caller)[0] AS kind, caller.file_path AS file_path, \
             caller.start_line AS line, \
             r.dispatch AS dispatch, \
             size(coalesce(r.concrete_types, [])) AS type_count \
             LIMIT 500"
        );

        // Query CALL_INSTANTIATES edges (may return 0 rows if edge type not yet populated)
        let ci_cypher = format!(
            "MATCH (caller:{ws})-[r:CALL_INSTANTIATES]->(target:{ws} {{fqn: $fqn}}) \
             RETURN caller.fqn AS fqn, caller.name AS name, \
             labels(caller)[0] AS kind, caller.file_path AS file_path, \
             caller.start_line AS line, \
             null AS dispatch, \
             0 AS type_count \
             LIMIT 500"
        );

        let mut results = Vec::new();
        for (cypher, is_instantiates) in [(&calls_cypher, false), (&ci_cypher, true)] {
            let mut rows = self
                .graph
                .execute(query(cypher).param("fqn", target_fqn))
                .await
                .context("caller query failed")?;

            while let Some(row) = rows.next().await.context("caller row failed")? {
                let fqn = match row.get::<String>("fqn") {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let node = TraversalNode {
                    name: row
                        .get::<String>("name")
                        .unwrap_or_else(|_| fqn.split("::").last().unwrap_or(&fqn).to_string()),
                    kind: row.get::<String>("kind").ok(),
                    file_path: row.get::<String>("file_path").ok(),
                    line: row.get::<i64>("line").ok().map(|l| l as u32),
                    fqn,
                };
                let provenance = if is_instantiates {
                    EdgeProvenance::Monomorph
                } else {
                    provenance_from_row(
                        row.get::<String>("dispatch").ok().as_deref(),
                        row.get::<i64>("type_count").unwrap_or(0),
                    )
                };
                debug!(fqn = %node.fqn, ?provenance, "caller discovered");
                results.push((node, provenance));
            }
        }

        Ok(results)
    }

    /// Single-level forward neighbor query: nodes called by `source_fqn`.
    async fn query_callees_of(
        &self,
        ws: &str,
        source_fqn: &str,
    ) -> Result<Vec<(TraversalNode, EdgeProvenance)>> {
        let calls_cypher = format!(
            "MATCH (source:{ws} {{fqn: $fqn}})-[r:CALLS]->(callee:{ws}) \
             RETURN callee.fqn AS fqn, callee.name AS name, \
             labels(callee)[0] AS kind, callee.file_path AS file_path, \
             callee.start_line AS line, \
             r.dispatch AS dispatch, \
             size(coalesce(r.concrete_types, [])) AS type_count \
             LIMIT 500"
        );

        let ci_cypher = format!(
            "MATCH (source:{ws} {{fqn: $fqn}})-[r:CALL_INSTANTIATES]->(callee:{ws}) \
             RETURN callee.fqn AS fqn, callee.name AS name, \
             labels(callee)[0] AS kind, callee.file_path AS file_path, \
             callee.start_line AS line, \
             null AS dispatch, \
             0 AS type_count \
             LIMIT 500"
        );

        let mut results = Vec::new();
        for (cypher, is_instantiates) in [(&calls_cypher, false), (&ci_cypher, true)] {
            let mut rows = self
                .graph
                .execute(query(cypher).param("fqn", source_fqn))
                .await
                .context("callee query failed")?;

            while let Some(row) = rows.next().await.context("callee row failed")? {
                let fqn = match row.get::<String>("fqn") {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let node = TraversalNode {
                    name: row
                        .get::<String>("name")
                        .unwrap_or_else(|_| fqn.split("::").last().unwrap_or(&fqn).to_string()),
                    kind: row.get::<String>("kind").ok(),
                    file_path: row.get::<String>("file_path").ok(),
                    line: row.get::<i64>("line").ok().map(|l| l as u32),
                    fqn,
                };
                let provenance = if is_instantiates {
                    EdgeProvenance::Monomorph
                } else {
                    provenance_from_row(
                        row.get::<String>("dispatch").ok().as_deref(),
                        row.get::<i64>("type_count").unwrap_or(0),
                    )
                };
                debug!(fqn = %node.fqn, ?provenance, "callee discovered");
                results.push((node, provenance));
            }
        }

        Ok(results)
    }
}

/// Determine provenance from CALLS relationship properties.
fn provenance_from_row(dispatch: Option<&str>, type_count: i64) -> EdgeProvenance {
    match dispatch {
        Some("dynamic") => EdgeProvenance::DynCandidate,
        _ if type_count > 0 => EdgeProvenance::Monomorph,
        _ => EdgeProvenance::Direct,
    }
}

/// Slice collected edges using cursor offset and limit, build TraversalResult.
fn build_result(
    root: TraversalNode,
    all_edges: Vec<TraversalEdge>,
    cycles_detected: bool,
    offset: usize,
    limit: usize,
) -> Result<TraversalResult> {
    let total = all_edges.len();
    let page: Vec<TraversalEdge> = all_edges
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();

    let has_more = offset + page.len() < total;
    let next_cursor = if has_more {
        Some(cursor::encode(offset + page.len()))
    } else {
        None
    };

    // Collect unique non-root nodes referenced by the page edges
    let mut seen_fqns: HashSet<String> = HashSet::new();
    seen_fqns.insert(root.fqn.clone());

    let mut nodes: Vec<TraversalNode> = Vec::new();
    // We only have FQN + name in edges; build stub nodes from edge data
    let mut node_index: HashMap<String, TraversalNode> = HashMap::new();
    for edge in &page {
        for fqn in [&edge.from_fqn, &edge.to_fqn] {
            if !seen_fqns.contains(fqn) {
                seen_fqns.insert(fqn.clone());
                node_index.entry(fqn.clone()).or_insert_with(|| TraversalNode {
                    fqn: fqn.clone(),
                    name: fqn.split("::").last().unwrap_or(fqn).to_string(),
                    kind: None,
                    file_path: None,
                    line: None,
                });
            }
        }
    }
    nodes.extend(node_index.into_values());
    nodes.sort_by(|a, b| a.fqn.cmp(&b.fqn));

    Ok(TraversalResult {
        root,
        nodes,
        edges: page,
        cycles_detected,
        next_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DEFAULT_LIMIT;

    fn make_edge(from: &str, to: &str, depth: u32) -> TraversalEdge {
        TraversalEdge {
            from_fqn: from.to_string(),
            to_fqn: to.to_string(),
            depth,
            provenance: EdgeProvenance::Direct,
        }
    }

    fn make_root(fqn: &str) -> TraversalNode {
        TraversalNode {
            fqn: fqn.to_string(),
            name: fqn.to_string(),
            kind: None,
            file_path: None,
            line: None,
        }
    }

    #[test]
    fn provenance_dynamic_dispatch() {
        assert_eq!(
            provenance_from_row(Some("dynamic"), 0),
            EdgeProvenance::DynCandidate
        );
    }

    #[test]
    fn provenance_with_concrete_types() {
        assert_eq!(
            provenance_from_row(Some("static"), 3),
            EdgeProvenance::Monomorph
        );
    }

    #[test]
    fn provenance_plain_static() {
        assert_eq!(provenance_from_row(Some("static"), 0), EdgeProvenance::Direct);
    }

    #[test]
    fn provenance_absent_dispatch() {
        assert_eq!(provenance_from_row(None, 0), EdgeProvenance::Direct);
    }

    #[test]
    fn build_result_first_page() {
        let root = make_root("root");
        let edges = vec![
            make_edge("a", "root", 1),
            make_edge("b", "root", 1),
            make_edge("c", "a", 2),
        ];
        let result = build_result(root, edges, false, 0, 2).unwrap();
        assert_eq!(result.edges.len(), 2);
        assert!(result.next_cursor.is_some());
        assert!(!result.cycles_detected);
    }

    #[test]
    fn build_result_last_page() {
        let root = make_root("root");
        let edges = vec![make_edge("a", "root", 1), make_edge("b", "root", 1)];
        let result = build_result(root, edges, false, 1, DEFAULT_LIMIT).unwrap();
        assert_eq!(result.edges.len(), 1);
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn build_result_cycles_flag_propagated() {
        let root = make_root("root");
        let result = build_result(root, vec![], true, 0, DEFAULT_LIMIT).unwrap();
        assert!(result.cycles_detected);
    }

    #[test]
    fn build_result_nodes_exclude_root() {
        let root = make_root("root::fn");
        let edges = vec![make_edge("caller::fn", "root::fn", 1)];
        let result = build_result(root, edges, false, 0, DEFAULT_LIMIT).unwrap();
        // nodes should contain "caller::fn" but not "root::fn"
        assert!(result.nodes.iter().any(|n| n.fqn == "caller::fn"));
        assert!(!result.nodes.iter().any(|n| n.fqn == "root::fn"));
    }

    #[test]
    fn build_result_nodes_deduped() {
        let root = make_root("root");
        let edges = vec![
            make_edge("a", "root", 1),
            make_edge("b", "a", 2),
            make_edge("a", "b", 3), // 'a' appears again
        ];
        let result = build_result(root, edges, false, 0, DEFAULT_LIMIT).unwrap();
        let fqns: Vec<&str> = result.nodes.iter().map(|n| n.fqn.as_str()).collect();
        let unique: HashSet<&str> = fqns.iter().copied().collect();
        assert_eq!(fqns.len(), unique.len(), "nodes must be unique");
    }

    #[test]
    fn build_result_empty_edges_no_cursor() {
        let root = make_root("root");
        let result = build_result(root, vec![], false, 0, DEFAULT_LIMIT).unwrap();
        assert!(result.next_cursor.is_none());
        assert!(result.nodes.is_empty());
        assert!(result.edges.is_empty());
    }
}
