//! API request handlers for the rust-brain API server.
//!
//! Each sub-module corresponds to a group of REST endpoints:
//!
//! | Module | Endpoints |
//! |--------|-----------|
//! | [`health`] | `GET /health`, `GET /metrics`, `GET /api/snapshot` |
//! | [`search`] | `POST /tools/search_semantic`, `POST /tools/aggregate_search` |
//! | [`items`] | `GET /tools/get_function`, `GET /tools/get_callers` |
//! | [`graph`] | `GET /tools/get_trait_impls`, `GET /tools/find_usages_of_type`, `GET /tools/get_module_tree`, `POST /tools/query_graph` |
//! | [`chat`] | `POST /tools/chat`, `GET /tools/chat/stream`, session CRUD |
//! | [`typecheck`] | `GET /tools/find_calls_with_type`, `GET /tools/find_trait_impls_for_type` |
//! | [`ingestion`] | `GET /api/ingestion/progress` |
//! | [`playground`] | Playground HTML serving |

pub mod artifacts;
pub mod benchmarker;
pub mod chat;
pub mod execution;
pub mod graph;
pub mod graph_templates;
pub mod health;
pub mod ingestion;
pub mod items;
pub mod pg_query;
pub mod playground;
pub mod search;
pub mod tasks;
pub mod typecheck;
pub mod validator;
pub mod workspace;
pub mod workspace_commit;
pub mod workspace_diff;
pub mod workspace_reset;
pub mod workspace_stream;

use serde::{Deserialize, Serialize};

// =============================================================================
// Shared Types
// =============================================================================

/// A function that calls the queried function (caller in the call graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerInfo {
    /// Fully qualified name of the calling function
    pub fqn: String,
    /// Short name of the calling function
    pub name: String,
    /// Source file path
    pub file_path: String,
    /// Line number of the call site
    pub line: u32,
}

/// A function called by the queried function (callee in the call graph).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalleeInfo {
    /// Fully qualified name of the called function
    pub fqn: String,
    /// Short name of the called function
    pub name: String,
}

/// Extended caller information returned by `get_callers`, including traversal depth.
#[derive(Debug, Serialize)]
pub struct CallerNode {
    /// Fully qualified name of the calling function
    pub fqn: String,
    /// Short name of the calling function
    pub name: String,
    /// Source file path
    pub file_path: String,
    /// Line number of the call site
    pub line: u32,
    /// Graph traversal depth from the target function
    pub depth: usize,
}

// =============================================================================
// Shared Defaults
// =============================================================================

/// Serde default for boolean fields that should default to `true`.
pub fn default_true() -> bool {
    true
}
/// Serde default for `limit` query parameters (10 results).
pub fn default_limit() -> usize {
    10
}
/// Serde default for `depth` query parameters (1 hop).
pub fn default_depth() -> usize {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }

    #[test]
    fn test_default_depth() {
        assert_eq!(default_depth(), 1);
    }
}
