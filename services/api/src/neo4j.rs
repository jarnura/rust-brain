//! Neo4j helper functions for the rust-brain API server.
//!
//! Provides:
//! - [`WorkspaceContext`] — validated workspace identity required for all graph queries
//! - [`WorkspaceGraphClient`] — the ONLY entry point for Neo4j operations in handlers;
//!   enforces workspace isolation at the type level
//! - [`execute_neo4j_query`] — low-level query execution for internal/system use
//! - Conversion utilities for Bolt types → JSON
//!
//! # Workspace Isolation
//!
//! Every node in Neo4j carries a composite label `Workspace_<id>` (e.g.,
//! `Workspace_550e8400e29b`). The `WorkspaceGraphClient` injects this label
//! into all Cypher queries so that a request scoped to workspace A can never
//! read nodes belonging to workspace B.
//!
//! **Trust boundary**: User-provided Cypher in `POST /tools/query_graph` has
//! workspace labels injected server-side before execution. Users cannot bypass
//! isolation by crafting Cypher.

use crate::errors::AppError;
use crate::handlers::{CalleeInfo, CallerNode};
use crate::state::AppState;
use std::sync::Arc;
use tracing::{debug, info, info_span, Instrument};

// =============================================================================
// WorkspaceContext
// =============================================================================

/// Validated workspace identity required for all graph operations.
///
/// Constructed via [`WorkspaceContext::new`] which validates the workspace ID
/// format. Once constructed, it cannot be empty or contain invalid characters.
///
/// # Examples
///
/// ```
/// # use rustbrain_api::neo4j::WorkspaceContext;
/// let ctx = WorkspaceContext::new("550e8400e29b".to_string()).unwrap();
/// assert_eq!(ctx.workspace_label(), "Workspace_550e8400e29b");
/// ```
#[derive(Debug, Clone)]
pub struct WorkspaceContext {
    workspace_id: String,
}

impl WorkspaceContext {
    /// Creates a new `WorkspaceContext`, validating the workspace ID.
    ///
    /// Valid IDs are non-empty, contain only alphanumeric characters,
    /// underscores, and hyphens, and are at most 128 characters long.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BadRequest`] if the workspace ID is empty,
    /// contains invalid characters, or exceeds 128 characters.
    pub fn new(workspace_id: String) -> Result<Self, AppError> {
        if workspace_id.is_empty() {
            return Err(AppError::BadRequest(
                "workspace_id must not be empty".to_string(),
            ));
        }
        if workspace_id.len() > 128 {
            return Err(AppError::BadRequest(
                "workspace_id must not exceed 128 characters".to_string(),
            ));
        }
        if !workspace_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(AppError::BadRequest(
                "workspace_id must contain only alphanumeric characters, underscores, and hyphens"
                    .to_string(),
            ));
        }
        Ok(Self { workspace_id })
    }

    /// Returns the Neo4j composite label for this workspace.
    ///
    /// Format: `Workspace_<12hex>` (e.g., `Workspace_550e8400e29b`).
    ///
    /// Derives the 12-character hex short ID from the workspace UUID by
    /// stripping hyphens and taking the first 12 characters, matching the
    /// Postgres schema naming convention (`ws_<12hex>`).
    pub fn workspace_label(&self) -> String {
        let hex: String = self.workspace_id.chars().filter(|c| *c != '-').collect();
        let short = &hex[..12.min(hex.len())];
        format!("Workspace_{short}")
    }

    /// Returns the 12-character hex short ID derived from the workspace UUID.
    pub fn short_id(&self) -> String {
        let hex: String = self.workspace_id.chars().filter(|c| *c != '-').collect();
        hex[..12.min(hex.len())].to_string()
    }

    /// Returns the raw workspace ID string.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

// =============================================================================
// WorkspaceGraphClient
// =============================================================================

/// The ONLY entry point for Neo4j graph operations in API handlers.
///
/// Requires a [`WorkspaceContext`] at construction — it is impossible to
/// construct a `WorkspaceGraphClient` without a valid workspace scope.
/// All query methods automatically inject workspace labels into Cypher,
/// ensuring multi-tenancy isolation at the type level.
///
/// # Construction
///
/// ```ignore
/// let ctx = WorkspaceContext::new(workspace_id)?;
/// let client = WorkspaceGraphClient::new(state.neo4j_graph_pool.clone(), ctx);
/// let callers = client.get_callers("crate::func", 1).await?;
/// ```
///
/// # Audit Logging
///
/// Every query execution is logged at DEBUG level with workspace_id, query
/// template name (where applicable), and execution time.
#[derive(Clone)]
pub struct WorkspaceGraphClient {
    graph: Arc<neo4rs::Graph>,
    ctx: WorkspaceContext,
}

impl WorkspaceGraphClient {
    /// Constructs a new workspace-scoped graph client.
    ///
    /// The `graph` parameter is the shared Bolt connection pool.
    /// The `ctx` parameter provides the workspace identity that scopes all queries.
    pub fn new(graph: Arc<neo4rs::Graph>, ctx: WorkspaceContext) -> Self {
        Self { graph, ctx }
    }

    /// Returns a reference to the workspace context.
    pub fn workspace(&self) -> &WorkspaceContext {
        &self.ctx
    }

    /// Executes a Cypher query that already contains workspace labels.
    ///
    /// This is the core execution method. The caller is responsible for
    /// ensuring that the Cypher string contains appropriate workspace label
    /// predicates (e.g., `:Workspace_abc123`).
    ///
    /// For user-provided Cypher that needs label injection, use
    /// [`Self::execute_user_cypher`] instead.
    pub async fn execute_query(
        &self,
        query: &str,
        params: serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let start = std::time::Instant::now();
        debug!(
            workspace_id = %self.ctx.workspace_id(),
            query_len = query.len(),
            "Executing workspace-scoped Neo4j query"
        );

        let result = execute_on_graph(&self.graph, query, params).await?;

        let elapsed = start.elapsed();
        debug!(
            workspace_id = %self.ctx.workspace_id(),
            elapsed_ms = elapsed.as_millis(),
            row_count = result.len(),
            "Neo4j query completed"
        );

        Ok(result)
    }

    /// Executes user-provided Cypher with workspace label injection.
    ///
    /// **Trust boundary**: This method injects `:Workspace_<id>` labels into
    /// all node patterns in MATCH and OPTIONAL MATCH clauses. Users cannot
    /// bypass workspace isolation by crafting Cypher — the label predicate
    /// is prepended server-side.
    ///
    /// # Label Injection Strategy (v1)
    ///
    /// Finds all node patterns `(var:Label1:Label2 ...)` in MATCH/OPTIONAL
    /// MATCH clauses and appends `:Workspace_<id>` after the last label.
    /// Bare node patterns `(var)` become `(var:Workspace_<id>)`.
    ///
    /// This is a regex-free, heuristic-based approach. A future v2 should
    /// use AST-based Cypher parsing for complete coverage.
    pub async fn execute_user_cypher(
        &self,
        cypher: &str,
        params: serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let start = std::time::Instant::now();
        let short_id = self.ctx.short_id();
        let injected = crate::handlers::workspace_label::inject_workspace_label(cypher, &short_id)?;
        debug!(
            workspace_id = %self.ctx.workspace_id(),
            original_len = cypher.len(),
            injected_len = injected.len(),
            "Injected workspace labels into user Cypher"
        );
        let span = info_span!(
            "neo4j_query",
            workspace.id = %self.ctx.workspace_id(),
            query_name = "user_cypher"
        );
        let result = self
            .execute_query(&injected, params)
            .instrument(span.clone())
            .await;
        let elapsed = start.elapsed();
        let row_count = result.as_ref().map(|r| r.len()).unwrap_or(0);
        info!(
            workspace_id = %self.ctx.workspace_id(),
            query_name = "user_cypher",
            duration_ms = elapsed.as_millis(),
            row_count = row_count,
            "Neo4j query executed"
        );
        result
    }

    /// Resolves a named query template and executes it with workspace labels.
    ///
    /// Delegates to [`crate::handlers::graph_templates::resolve_with_workspace`]
    /// which generates Cypher with `:Workspace_<id>` labels already included.
    pub async fn resolve_and_execute_template(
        &self,
        query_name: &str,
        parameters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Vec<serde_json::Value>, AppError> {
        let start = std::time::Instant::now();
        let ws_label = self.ctx.workspace_label();
        let (cypher, params) = crate::handlers::graph_templates::resolve_with_workspace(
            query_name, parameters, &ws_label,
        )?;
        let span = info_span!("neo4j_query", workspace.id = %self.ctx.workspace_id(), query_name = %query_name);
        let result = self
            .execute_query(&cypher, params)
            .instrument(span.clone())
            .await;
        let elapsed = start.elapsed();
        let row_count = result.as_ref().map(|r| r.len()).unwrap_or(0);
        info!(
            workspace_id = %self.ctx.workspace_id(),
            query_name = %query_name,
            duration_ms = elapsed.as_millis(),
            row_count = row_count,
            "Neo4j query executed"
        );
        result
    }

    /// Finds functions that call the function identified by `fqn`.
    ///
    /// Workspace-scoped: only returns callers within the same workspace.
    pub async fn get_callers(&self, fqn: &str, depth: usize) -> Result<Vec<CallerNode>, AppError> {
        let depth = depth.clamp(1, 5);

        let mut params = std::collections::HashMap::new();
        params.insert("fqn".to_string(), serde_json::json!(fqn));
        params.insert("depth".to_string(), serde_json::json!(depth));
        params.insert("limit".to_string(), serde_json::json!(50i64));

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            "get_callers",
            &params,
            &self.ctx.workspace_label(),
        )?;

        let results = self.execute_query(&cypher, query_params).await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                Some(CallerNode {
                    fqn: r.get("fqn")?.as_str()?.to_string(),
                    name: r.get("name")?.as_str()?.to_string(),
                    file_path: r
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    line: r.get("line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                    depth: 1,
                })
            })
            .collect())
    }

    /// Finds callers for an impl block by aggregating callers of its child methods.
    ///
    /// Workspace-scoped: only returns callers within the same workspace.
    pub async fn get_callers_for_impl(
        &self,
        method_prefix: &str,
        depth: usize,
    ) -> Result<Vec<CallerNode>, AppError> {
        let depth = depth.clamp(1, 5);

        let mut params = std::collections::HashMap::new();
        params.insert("prefix".to_string(), serde_json::json!(method_prefix));
        params.insert("depth".to_string(), serde_json::json!(depth));
        params.insert("limit".to_string(), serde_json::json!(50i64));

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            "get_callers_for_impl",
            &params,
            &self.ctx.workspace_label(),
        )?;

        let results = self.execute_query(&cypher, query_params).await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                Some(CallerNode {
                    fqn: r.get("fqn")?.as_str()?.to_string(),
                    name: r.get("name")?.as_str()?.to_string(),
                    file_path: r
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    line: r.get("line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
                    depth: 1,
                })
            })
            .collect())
    }

    /// Finds callees for an impl block by aggregating callees of its child methods.
    ///
    /// Workspace-scoped: only returns callees within the same workspace.
    pub async fn get_callees_for_impl(
        &self,
        method_prefix: &str,
    ) -> Result<Vec<CalleeInfo>, AppError> {
        let mut params = std::collections::HashMap::new();
        params.insert("prefix".to_string(), serde_json::json!(method_prefix));
        params.insert("limit".to_string(), serde_json::json!(100i64));

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            "get_callees_for_impl",
            &params,
            &self.ctx.workspace_label(),
        )?;

        let results = self.execute_query(&cypher, query_params).await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                Some(CalleeInfo {
                    fqn: r.get("fqn")?.as_str()?.to_string(),
                    name: r.get("name")?.as_str()?.to_string(),
                })
            })
            .collect())
    }

    /// Finds functions called by the function identified by `fqn`.
    ///
    /// Workspace-scoped: only returns callees within the same workspace.
    pub async fn get_callees(&self, fqn: &str) -> Result<Vec<CalleeInfo>, AppError> {
        let mut params = std::collections::HashMap::new();
        params.insert("fqn".to_string(), serde_json::json!(fqn));

        let (cypher, query_params) = crate::handlers::graph_templates::resolve_with_workspace(
            "get_callees",
            &params,
            &self.ctx.workspace_label(),
        )?;

        let results = self.execute_query(&cypher, query_params).await?;

        Ok(results
            .into_iter()
            .filter_map(|r| {
                Some(CalleeInfo {
                    fqn: r.get("fqn")?.as_str()?.to_string(),
                    name: r.get("name")?.as_str()?.to_string(),
                })
            })
            .collect())
    }
}

// =============================================================================
// Workspace Label Injection for User Cypher
// =============================================================================

// =============================================================================
// Legacy Functions (for internal/system use only)
// =============================================================================

/// Verifies the Neo4j connection by executing `RETURN 1`.
///
/// Used by health check endpoints. Not workspace-scoped (system-level).
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the query fails or the connection is refused.
pub async fn check_neo4j(graph: &Arc<neo4rs::Graph>) -> Result<(), AppError> {
    let mut result = graph
        .execute(neo4rs::query("RETURN 1 as test"))
        .await
        .map_err(|e| AppError::Neo4j(format!("Neo4j health check failed: {}", e)))?;

    let _row = result
        .next()
        .await
        .map_err(|e| AppError::Neo4j(format!("Neo4j health check failed: {}", e)))?;

    Ok(())
}

/// Executes a Cypher query via Neo4j Bolt protocol and returns results as JSON.
///
/// **For internal/system use only** (health checks, gap analysis, consistency).
/// API handlers must use [`WorkspaceGraphClient`] instead.
///
/// Each row is returned as a `serde_json::Value` object with column names as
/// keys. Parameters in `params` are bound to the query by key name; `Null`
/// values are skipped.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if query execution or row fetching fails.
pub async fn execute_neo4j_query(
    state: &AppState,
    query: &str,
    params: serde_json::Value,
) -> Result<Vec<serde_json::Value>, AppError> {
    execute_on_graph(&state.neo4j_graph, query, params).await
}

/// Internal query execution against a specific graph connection.
async fn execute_on_graph(
    graph: &Arc<neo4rs::Graph>,
    query: &str,
    params: serde_json::Value,
) -> Result<Vec<serde_json::Value>, AppError> {
    let mut q = neo4rs::query(query);

    if let serde_json::Value::Object(map) = &params {
        for (key, value) in map {
            q = match value {
                serde_json::Value::String(s) => q.param(key.as_str(), s.as_str()),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        q.param(key.as_str(), i)
                    } else if let Some(f) = n.as_f64() {
                        q.param(key.as_str(), f)
                    } else {
                        q
                    }
                }
                serde_json::Value::Bool(b) => q.param(key.as_str(), *b),
                serde_json::Value::Null => q,
                other => q.param(key.as_str(), other.to_string()),
            };
        }
    }

    let mut result = graph
        .execute(q)
        .await
        .map_err(|e| AppError::Neo4j(format!("Failed to execute Neo4j query: {}", e)))?;

    let mut rows = Vec::new();
    while let Some(row) = result
        .next()
        .await
        .map_err(|e| AppError::Neo4j(format!("Failed to fetch Neo4j row: {}", e)))?
    {
        let row_json = row_to_json(&row);
        rows.push(row_json);
    }

    Ok(rows)
}

/// Converts a [`neo4rs::Row`] into a JSON object by treating it as a [`neo4rs::BoltMap`].
///
/// Returns `null` if the row cannot be interpreted as a map.
pub fn row_to_json(row: &neo4rs::Row) -> serde_json::Value {
    if let Ok(node) = row.to::<neo4rs::BoltMap>() {
        bolt_map_to_json(&node)
    } else {
        serde_json::Value::Null
    }
}

/// Converts a [`neo4rs::BoltMap`] into a JSON object, recursively converting values.
pub fn bolt_map_to_json(map: &neo4rs::BoltMap) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (key, value) in &map.value {
        obj.insert(key.to_string(), bolt_type_to_json(value));
    }
    serde_json::Value::Object(obj)
}

/// Converts a [`neo4rs::BoltType`] into a JSON value.
///
/// Handles strings, integers, floats, booleans, nulls, lists, maps, and nodes.
/// Node properties are extracted into a flat object with an extra `_labels` array.
/// Unrecognized bolt types are formatted with `Debug`.
pub fn bolt_type_to_json(value: &neo4rs::BoltType) -> serde_json::Value {
    match value {
        neo4rs::BoltType::String(s) => serde_json::Value::String(s.to_string()),
        neo4rs::BoltType::Integer(i) => serde_json::json!(i.value),
        neo4rs::BoltType::Float(f) => serde_json::json!(f.value),
        neo4rs::BoltType::Boolean(b) => serde_json::Value::Bool(b.value),
        neo4rs::BoltType::Null(_) => serde_json::Value::Null,
        neo4rs::BoltType::List(list) => {
            let items: Vec<serde_json::Value> = list.iter().map(bolt_type_to_json).collect();
            serde_json::Value::Array(items)
        }
        neo4rs::BoltType::Map(map) => bolt_map_to_json(map),
        neo4rs::BoltType::Node(node) => {
            let mut obj = serde_json::Map::new();
            for (key, value) in &node.properties.value {
                obj.insert(key.to_string(), bolt_type_to_json(value));
            }
            let labels: Vec<serde_json::Value> =
                node.labels.iter().map(bolt_type_to_json).collect();
            obj.insert("_labels".to_string(), serde_json::Value::Array(labels));
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bolt_type_to_json_string() {
        let bolt = neo4rs::BoltType::String(neo4rs::BoltString::from("hello"));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn test_bolt_type_to_json_integer() {
        let bolt = neo4rs::BoltType::Integer(neo4rs::BoltInteger::new(42));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::json!(42));
    }

    #[test]
    fn test_bolt_type_to_json_float() {
        let val = 2.5_f64;
        let bolt = neo4rs::BoltType::Float(neo4rs::BoltFloat::new(val));
        let json = bolt_type_to_json(&bolt);
        assert!((json.as_f64().unwrap() - val).abs() < f64::EPSILON);
    }

    #[test]
    fn test_bolt_type_to_json_boolean() {
        let bolt = neo4rs::BoltType::Boolean(neo4rs::BoltBoolean::new(true));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::Bool(true));
    }

    #[test]
    fn test_bolt_type_to_json_null() {
        let bolt = neo4rs::BoltType::Null(neo4rs::BoltNull);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_bolt_type_to_json_list() {
        let list = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("a"),
            neo4rs::BoltType::from("b"),
        ]);
        let bolt = neo4rs::BoltType::List(list);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::json!(["a", "b"]));
    }

    #[test]
    fn test_bolt_map_to_json() {
        let mut map = neo4rs::BoltMap::new();
        map.put("name".into(), neo4rs::BoltType::from("test_fn"));
        map.put("line".into(), neo4rs::BoltType::from(42_i64));
        let json = bolt_map_to_json(&map);
        assert_eq!(json["name"], "test_fn");
        assert_eq!(json["line"], 42);
    }

    #[test]
    fn test_bolt_node_to_json() {
        let properties: neo4rs::BoltMap = vec![
            (
                neo4rs::BoltString::from("fqn"),
                neo4rs::BoltType::from("crate::func"),
            ),
            (
                neo4rs::BoltString::from("name"),
                neo4rs::BoltType::from("func"),
            ),
        ]
        .into_iter()
        .collect();
        let labels = neo4rs::BoltList::from(vec![neo4rs::BoltType::from("Function")]);
        let node = neo4rs::BoltNode::new(neo4rs::BoltInteger::new(1), labels, properties);
        let bolt = neo4rs::BoltType::Node(node);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json["fqn"], "crate::func");
        assert_eq!(json["name"], "func");
        assert!(json["_labels"].is_array());
    }

    #[test]
    fn test_row_to_json_with_bolt_map() {
        let fields = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("fqn"),
            neo4rs::BoltType::from("name"),
            neo4rs::BoltType::from("line"),
        ]);
        let data = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("crate::module::func"),
            neo4rs::BoltType::from("func"),
            neo4rs::BoltType::from(10_i64),
        ]);
        let row = neo4rs::Row::new(fields, data);
        let json = row_to_json(&row);
        assert_eq!(json["fqn"], "crate::module::func");
        assert_eq!(json["name"], "func");
        assert_eq!(json["line"], 10);
    }

    // ── WorkspaceContext tests ──────────────────────────────────────────

    #[test]
    fn test_workspace_context_valid() {
        let ctx = WorkspaceContext::new("abc123".to_string()).unwrap();
        assert_eq!(ctx.workspace_id(), "abc123");
        assert_eq!(ctx.workspace_label(), "Workspace_abc123");
    }

    #[test]
    fn test_workspace_context_with_underscores_and_hyphens() {
        let ctx = WorkspaceContext::new("my-workspace_123".to_string()).unwrap();
        assert_eq!(ctx.workspace_label(), "Workspace_myworkspace_");
    }

    #[test]
    fn test_workspace_context_rejects_empty() {
        let err = WorkspaceContext::new("".to_string()).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_workspace_context_rejects_too_long() {
        let long_id = "a".repeat(129);
        let err = WorkspaceContext::new(long_id).unwrap_err();
        assert!(err.to_string().contains("128 characters"));
    }

    #[test]
    fn test_workspace_context_accepts_max_length() {
        let max_id = "a".repeat(128);
        let ctx = WorkspaceContext::new(max_id.clone()).unwrap();
        assert_eq!(ctx.workspace_id(), max_id);
    }

    #[test]
    fn test_workspace_context_rejects_special_chars() {
        for invalid in &["abc!def", "hello world", "a.b", "a/b", "a@b"] {
            let err = WorkspaceContext::new(invalid.to_string()).unwrap_err();
            assert!(
                err.to_string().contains("alphanumeric"),
                "Expected rejection for '{}'",
                invalid
            );
        }
    }

    #[test]
    fn test_workspace_label_from_uuid() {
        let ctx =
            WorkspaceContext::new("550e8400-e29b-41d4-a716-446655440000".to_string()).unwrap();
        assert_eq!(ctx.workspace_label(), "Workspace_550e8400e29b");
        assert_eq!(ctx.short_id(), "550e8400e29b");
    }
}
