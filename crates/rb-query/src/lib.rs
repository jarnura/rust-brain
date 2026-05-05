//! Call-graph traversal engine for rust-brain data-plane queries.
//!
//! Implements REQ-DP-03: depth-limited BFS over `CALLS` and `CALL_INSTANTIATES`
//! edges in Neo4j with per-edge provenance, cycle detection, and cursor-based
//! pagination.
//!
//! # Usage
//!
//! ```ignore
//! use rb_query::{CallGraphTraverser, TraversalOptions};
//! use std::sync::Arc;
//!
//! let traverser = CallGraphTraverser::new(graph, "Workspace_abc123".to_string());
//! let opts = TraversalOptions::default();
//! let result = traverser.get_callers("crate::module::func", opts).await?;
//! ```

pub mod cursor;
pub mod traversal;
pub mod types;

pub use traversal::CallGraphTraverser;
pub use types::{EdgeProvenance, TraversalEdge, TraversalNode, TraversalOptions, TraversalResult};
