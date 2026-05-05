//! REQ-DP-03: caller/callee traversal handlers.
//!
//! Endpoints:
//! - `GET /v1/repos/{repo_id}/items/{fqn_b64}/callers`
//! - `GET /v1/repos/{repo_id}/items/{fqn_b64}/callees`
//!
//! `repo_id` maps to the workspace identifier used for Neo4j tenant isolation.
//! `fqn_b64` is the URL-safe base64-encoded fully qualified name of the target item.
//!
//! # Query parameters
//!
//! | Parameter | Default | Max   | Description                                  |
//! |-----------|---------|-------|----------------------------------------------|
//! | `depth`   | 3       | 10    | BFS traversal depth                          |
//! | `limit`   | 50      | 200   | Max edges to return per page                 |
//! | `cursor`  | —       | —     | Opaque continuation token from prior response|

use axum::{
    extract::{Path, Query, State},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rb_query::{
    CallGraphTraverser, TraversalOptions, TraversalResult,
    types::{DEFAULT_DEPTH, DEFAULT_LIMIT, MAX_DEPTH, MAX_LIMIT},
};
use serde::Deserialize;
use tracing::debug;

use crate::errors::AppError;
use crate::neo4j::WorkspaceContext;
use crate::state::AppState;

// =============================================================================
// Request types
// =============================================================================

/// Path parameters for the v1 caller/callee endpoints.
#[derive(Debug, Deserialize)]
pub struct RepoItemPath {
    /// Workspace / repository identifier (used as Neo4j workspace label).
    pub repo_id: String,
    /// URL-safe base64-encoded fully qualified name.
    pub fqn_b64: String,
}

/// Query parameters common to both callers and callees endpoints.
#[derive(Debug, Deserialize)]
pub struct TraversalQuery {
    /// BFS depth (1–10, default 3).
    #[serde(default = "default_depth")]
    pub depth: u32,
    /// Max edges per page (1–200, default 50).
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Opaque cursor returned by a previous response.
    pub cursor: Option<String>,
}

fn default_depth() -> u32 {
    DEFAULT_DEPTH
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

// =============================================================================
// Helpers
// =============================================================================

/// Decode a URL-safe base64 FQN from the path segment.
fn decode_fqn(fqn_b64: &str) -> Result<String, AppError> {
    let bytes = URL_SAFE_NO_PAD.decode(fqn_b64).map_err(|_| {
        AppError::BadRequest(format!(
            "fqn_b64 is not valid URL-safe base64: '{fqn_b64}'"
        ))
    })?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::BadRequest("fqn_b64 decoded bytes are not valid UTF-8".to_string()))
}

/// Validate traversal query parameters and build `TraversalOptions`.
fn build_opts(q: TraversalQuery) -> Result<TraversalOptions, AppError> {
    if q.depth > MAX_DEPTH as u32 {
        return Err(AppError::BadRequest(format!(
            "depth must be <= {MAX_DEPTH}"
        )));
    }
    if q.limit > MAX_LIMIT {
        return Err(AppError::BadRequest(format!(
            "limit must be <= {MAX_LIMIT}"
        )));
    }
    Ok(TraversalOptions {
        depth: q.depth,
        limit: q.limit,
        cursor: q.cursor,
    })
}

/// Build a `CallGraphTraverser` scoped to the given repo/workspace.
fn make_traverser(
    state: &AppState,
    repo_id: &str,
) -> Result<CallGraphTraverser, AppError> {
    let ctx = WorkspaceContext::new(repo_id.to_string())?;
    Ok(CallGraphTraverser::new(
        state.neo4j_graph.clone(),
        ctx.workspace_label(),
    ))
}

/// Map `anyhow::Error` from the traversal engine to `AppError`.
fn traversal_err(err: anyhow::Error) -> AppError {
    let msg = err.to_string();
    if msg.contains("invalid cursor") {
        AppError::BadRequest(msg)
    } else {
        AppError::Neo4j(msg)
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// Returns all functions that call the target item, up to `depth` hops.
///
/// BFS traverses `CALLS` and `CALL_INSTANTIATES` edges backward (toward callers).
/// Cycle detection prevents infinite loops in recursive call graphs.
/// Results are paginated; use `next_cursor` to fetch the next page.
///
/// # Errors
///
/// - `400 BAD_REQUEST` — invalid `repo_id`, malformed `fqn_b64`, or invalid query params.
/// - `500 NEO4J_ERROR` — graph query failed.
pub async fn get_callers(
    State(state): State<AppState>,
    Path(path): Path<RepoItemPath>,
    Query(query): Query<TraversalQuery>,
) -> Result<Json<TraversalResult>, AppError> {
    let fqn = decode_fqn(&path.fqn_b64)?;
    let opts = build_opts(query)?;

    debug!(repo_id = %path.repo_id, fqn = %fqn, depth = opts.depth, "get_callers");

    let traverser = make_traverser(&state, &path.repo_id)?;
    let result = traverser
        .get_callers(&fqn, opts)
        .await
        .map_err(traversal_err)?;

    Ok(Json(result))
}

/// Returns all functions called by the target item, up to `depth` hops.
///
/// BFS traverses `CALLS` and `CALL_INSTANTIATES` edges forward (toward callees).
/// Cycle detection prevents infinite loops in recursive call graphs.
/// Results are paginated; use `next_cursor` to fetch the next page.
///
/// # Errors
///
/// - `400 BAD_REQUEST` — invalid `repo_id`, malformed `fqn_b64`, or invalid query params.
/// - `500 NEO4J_ERROR` — graph query failed.
pub async fn get_callees(
    State(state): State<AppState>,
    Path(path): Path<RepoItemPath>,
    Query(query): Query<TraversalQuery>,
) -> Result<Json<TraversalResult>, AppError> {
    let fqn = decode_fqn(&path.fqn_b64)?;
    let opts = build_opts(query)?;

    debug!(repo_id = %path.repo_id, fqn = %fqn, depth = opts.depth, "get_callees");

    let traverser = make_traverser(&state, &path.repo_id)?;
    let result = traverser
        .get_callees(&fqn, opts)
        .await
        .map_err(traversal_err)?;

    Ok(Json(result))
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_fqn_roundtrip() {
        let fqn = "crate::module::my_function";
        let encoded = URL_SAFE_NO_PAD.encode(fqn.as_bytes());
        assert_eq!(decode_fqn(&encoded).unwrap(), fqn);
    }

    #[test]
    fn decode_fqn_rejects_bad_base64() {
        let err = decode_fqn("not!valid!base64").unwrap_err();
        assert!(err.to_string().contains("not valid URL-safe base64"));
    }

    #[test]
    fn decode_fqn_rejects_non_utf8() {
        // Valid base64 of 0xFF bytes (invalid UTF-8)
        let encoded = URL_SAFE_NO_PAD.encode([0xFF, 0xFE]);
        let err = decode_fqn(&encoded).unwrap_err();
        assert!(err.to_string().contains("not valid UTF-8"));
    }

    #[test]
    fn build_opts_defaults() {
        let q = TraversalQuery {
            depth: DEFAULT_DEPTH,
            limit: DEFAULT_LIMIT,
            cursor: None,
        };
        let opts = build_opts(q).unwrap();
        assert_eq!(opts.depth, DEFAULT_DEPTH);
        assert_eq!(opts.limit, DEFAULT_LIMIT);
        assert!(opts.cursor.is_none());
    }

    #[test]
    fn build_opts_rejects_depth_over_max() {
        let q = TraversalQuery {
            depth: MAX_DEPTH as u32 + 1,
            limit: DEFAULT_LIMIT,
            cursor: None,
        };
        let err = build_opts(q).unwrap_err();
        assert!(err.to_string().contains("depth must be"));
    }

    #[test]
    fn build_opts_rejects_limit_over_max() {
        let q = TraversalQuery {
            depth: DEFAULT_DEPTH,
            limit: MAX_LIMIT + 1,
            cursor: None,
        };
        let err = build_opts(q).unwrap_err();
        assert!(err.to_string().contains("limit must be"));
    }

    #[test]
    fn traversal_err_maps_cursor_to_bad_request() {
        let err = anyhow::anyhow!("invalid cursor: base64 decode failed");
        match traversal_err(err) {
            AppError::BadRequest(_) => {}
            other => panic!("expected BadRequest, got {:?}", other),
        }
    }

    #[test]
    fn traversal_err_maps_other_to_neo4j() {
        let err = anyhow::anyhow!("connection refused");
        match traversal_err(err) {
            AppError::Neo4j(_) => {}
            other => panic!("expected Neo4j, got {:?}", other),
        }
    }
}
