//! Public types for the caller/callee traversal API (REQ-DP-03).

use serde::{Deserialize, Serialize};

pub const DEFAULT_DEPTH: u32 = 3;
pub const MAX_DEPTH: u32 = 10;
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;

/// Per-edge dispatch provenance returned in traversal results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeProvenance {
    /// Static direct call — CALLS edge with static dispatch.
    Direct,
    /// Monomorphized generic instantiation — CALL_INSTANTIATES edge or CALLS
    /// edge carrying concrete type arguments.
    Monomorph,
    /// Dynamic dispatch candidate — CALLS edge with `dispatch = "dynamic"`.
    DynCandidate,
}

/// A node discovered during BFS traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalNode {
    pub fqn: String,
    pub name: String,
    pub kind: Option<String>,
    pub file_path: Option<String>,
    pub line: Option<u32>,
}

/// A directed edge discovered during BFS traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalEdge {
    /// FQN of the calling node (for callers traversal) or source node (for callees traversal).
    pub from_fqn: String,
    /// FQN of the called node (target).
    pub to_fqn: String,
    /// BFS depth at which this edge was discovered (1-indexed from root).
    pub depth: u32,
    pub provenance: EdgeProvenance,
}

/// Options controlling BFS traversal behaviour.
#[derive(Debug, Clone)]
pub struct TraversalOptions {
    /// Maximum traversal depth (1–10, default 3).
    pub depth: u32,
    /// Maximum edges to return per page (1–200, default 50).
    pub limit: usize,
    /// Opaque cursor for continuation (base64-encoded offset).
    pub cursor: Option<String>,
}

impl Default for TraversalOptions {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            limit: DEFAULT_LIMIT,
            cursor: None,
        }
    }
}

impl TraversalOptions {
    /// Clamp depth and limit to valid ranges.
    pub fn clamp(mut self) -> Self {
        self.depth = self.depth.clamp(1, MAX_DEPTH);
        self.limit = self.limit.clamp(1, MAX_LIMIT);
        self
    }
}

/// Result of a caller/callee BFS traversal.
#[derive(Debug, Serialize)]
pub struct TraversalResult {
    /// The root node from which traversal started.
    pub root: TraversalNode,
    /// All unique nodes encountered (excluding root).
    pub nodes: Vec<TraversalNode>,
    /// All edges traversed, in BFS order.
    pub edges: Vec<TraversalEdge>,
    /// Whether any cycle was detected during BFS.
    pub cycles_detected: bool,
    /// Cursor for the next page, absent if this is the last page.
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_opts_are_valid() {
        let opts = TraversalOptions::default().clamp();
        assert_eq!(opts.depth, DEFAULT_DEPTH);
        assert_eq!(opts.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn clamp_caps_depth_and_limit() {
        let opts = TraversalOptions {
            depth: 999,
            limit: 9999,
            cursor: None,
        }
        .clamp();
        assert_eq!(opts.depth, MAX_DEPTH);
        assert_eq!(opts.limit, MAX_LIMIT);
    }

    #[test]
    fn provenance_serializes_snake_case() {
        let j = serde_json::to_string(&EdgeProvenance::DynCandidate).unwrap();
        assert_eq!(j, "\"dyn_candidate\"");
        let j = serde_json::to_string(&EdgeProvenance::Monomorph).unwrap();
        assert_eq!(j, "\"monomorph\"");
        let j = serde_json::to_string(&EdgeProvenance::Direct).unwrap();
        assert_eq!(j, "\"direct\"");
    }
}
