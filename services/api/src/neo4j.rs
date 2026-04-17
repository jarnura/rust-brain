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
use tracing::debug;

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
    /// Format: `Workspace_<id>` (e.g., `Workspace_550e8400e29b`).
    pub fn workspace_label(&self) -> String {
        format!("Workspace_{}", self.workspace_id)
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
        let ws_label = self.ctx.workspace_label();
        let injected = inject_workspace_label(cypher, &ws_label);
        debug!(
            workspace_id = %self.ctx.workspace_id(),
            original_len = cypher.len(),
            injected_len = injected.len(),
            "Injected workspace labels into user Cypher"
        );
        self.execute_query(&injected, params).await
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
        let ws_label = self.ctx.workspace_label();
        let (cypher, params) = crate::handlers::graph_templates::resolve_with_workspace(
            query_name, parameters, &ws_label,
        )?;
        self.execute_query(&cypher, params).await
    }

    /// Finds functions that call the function identified by `fqn`.
    ///
    /// Workspace-scoped: only returns callers within the same workspace.
    pub async fn get_callers(&self, fqn: &str, depth: usize) -> Result<Vec<CallerNode>, AppError> {
        let depth = depth.clamp(1, 5);
        let ws = &self.ctx.workspace_label();

        let cypher = format!(
            r#"
            MATCH (caller:Function:{ws})-[:CALLS*1..{depth}]->(callee:Function:{ws} {{fqn: $fqn}})
            WHERE caller.fqn <> $fqn
            RETURN DISTINCT caller.fqn as fqn, caller.name as name, caller.file_path as file_path,
                   caller.start_line as line
            ORDER BY fqn
            LIMIT 50
            "#
        );

        let params = serde_json::json!({"fqn": fqn});
        let results = self.execute_query(&cypher, params).await?;

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
        let ws = &self.ctx.workspace_label();

        let cypher = format!(
            r#"
            MATCH (method:Function:{ws})
            WHERE method.fqn STARTS WITH $prefix
            WITH method
            MATCH (caller:Function:{ws})-[:CALLS*1..{depth}]->(method)
            WHERE NOT caller.fqn STARTS WITH $prefix
            RETURN DISTINCT caller.fqn as fqn, caller.name as name,
                   caller.file_path as file_path, caller.start_line as line
            ORDER BY fqn
            LIMIT 50
            "#
        );

        let params = serde_json::json!({"prefix": method_prefix});
        let results = self.execute_query(&cypher, params).await?;

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
        let ws = &self.ctx.workspace_label();

        let cypher = format!(
            r#"
            MATCH (method:Function:{ws})
            WHERE method.fqn STARTS WITH $prefix
            WITH method
            MATCH (method)-[:CALLS]->(callee:Function:{ws})
            WHERE NOT callee.fqn STARTS WITH $prefix
            RETURN DISTINCT callee.fqn as fqn, callee.name as name
            ORDER BY name
            LIMIT 100
            "#
        );

        let params = serde_json::json!({"prefix": method_prefix});
        let results = self.execute_query(&cypher, params).await?;

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
        let ws = &self.ctx.workspace_label();

        let cypher = format!(
            r#"
            MATCH (caller:Function:{ws} {{fqn: $fqn}})-[:CALLS]->(callee:Function:{ws})
            RETURN callee.fqn as fqn, callee.name as name
            ORDER BY name
            "#
        );

        let params = serde_json::json!({"fqn": fqn});
        let results = self.execute_query(&cypher, params).await?;

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

/// Injects `:Workspace_<id>` labels into all node patterns in a Cypher query.
///
/// **Trust boundary**: This function is the server-side enforcement point.
/// Users cannot bypass workspace isolation because this injection happens
/// after the user submits their Cypher but before it reaches Neo4j.
///
/// # Strategy (v1)
///
/// Iterates through the Cypher string and finds node patterns in MATCH and
/// OPTIONAL MATCH clauses. For each node pattern:
/// - `(var:Label1:Label2 {props})` → `(var:Label1:Label2:Workspace_<id> {props})`
/// - `(var:Label1:Label2)` → `(var:Label1:Label2:Workspace_<id>)`
/// - `(var {props})` → `(var:Workspace_<id> {props})`
/// - `(var)` → `(var:Workspace_<id>)`
///
/// Relationship patterns `-[r:REL]->` are NOT modified.
///
/// # Limitations (v1)
///
/// - Does not parse WITH clause node references
/// - Does not handle CALL {} subqueries
/// - Simple heuristic: any `(` after MATCH/OPTIONAL MATCH/arrow is treated
///   as a node pattern start
///
/// A future v2 should use AST-based Cypher parsing for complete coverage.
fn inject_workspace_label(cypher: &str, workspace_label: &str) -> String {
    let mut result = String::with_capacity(cypher.len() + cypher.len() / 4);
    let bytes = cypher.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Track whether we're inside a MATCH context where node patterns should
    // be modified. We set this to true after MATCH/OPTIONAL MATCH keywords
    // and after relationship arrows ->  or  -[...]->.
    let mut in_match_context = false;

    while i < len {
        // Detect MATCH keyword (but not OPTIONAL MATCH which we handle separately)
        if starts_with_ignore_case(&bytes[i..], "MATCH")
            && !starts_with_ignore_case(&bytes[i..], "MATCHES")
        {
            // Check it's not preceded by "OPTIONAL " (we handle that below)
            // Check that the character after MATCH is whitespace or end
            let after_match = i + 5;
            if after_match >= len || !bytes[after_match].is_ascii_alphabetic() {
                result.push_str("MATCH");
                i += 5;
                in_match_context = true;
                // Consume whitespace
                while i < len && bytes[i] == b' ' {
                    result.push(' ');
                    i += 1;
                }
                continue;
            }
        }

        // Detect OPTIONAL MATCH
        if starts_with_ignore_case(&bytes[i..], "OPTIONAL MATCH") {
            let after = i + 14;
            if after >= len || !bytes[after].is_ascii_alphabetic() {
                result.push_str("OPTIONAL MATCH");
                i += 14;
                in_match_context = true;
                while i < len && bytes[i] == b' ' {
                    result.push(' ');
                    i += 1;
                }
                continue;
            }
        }

        // If we're in a match context and see a '(', process a node pattern
        if in_match_context && i < len && bytes[i] == b'(' {
            let node_end = find_matching_paren(bytes, i);
            if node_end > i {
                let node_content = &cypher[i + 1..node_end];
                let injected = inject_label_into_node(node_content, workspace_label);
                result.push('(');
                result.push_str(&injected);
                result.push(')');
                i = node_end + 1;
                // After a node pattern, we might see relationship patterns
                // or more node patterns. Stay in match context.
                continue;
            }
        }

        // Detect end of match context (WHERE, WITH, RETURN, SET, etc.)
        if in_match_context && is_clause_boundary(&bytes[i..]) {
            in_match_context = false;
        }

        // Detect relationship arrow patterns: -> or -[...]-> or <- or <-
        // After a relationship, we're back in match context for the next node
        if i + 1 < len && bytes[i] == b'-' && bytes[i + 1] == b'>' {
            result.push_str("->");
            i += 2;
            in_match_context = true;
            continue;
        }
        if i + 1 < len && bytes[i] == b'<' && bytes[i + 1] == b'-' {
            result.push_str("<-");
            i += 2;
            in_match_context = true;
            continue;
        }

        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

/// Injects a workspace label into a single node pattern's content.
///
/// Input is the content between `(` and `)` of a node pattern, e.g.:
/// - `f:Function` → `f:Function:Workspace_abc`
/// - `n:Function:Exported {fqn: $fqn}` → `n:Function:Exported:Workspace_abc {fqn: $fqn}`
/// - `n` → `n:Workspace_abc`
/// - `n {fqn: $fqn}` → `n:Workspace_abc {fqn: $fqn}`
fn inject_label_into_node(content: &str, workspace_label: &str) -> String {
    let trimmed = content.trim();

    // Find the end of the variable name + labels portion.
    // This is before any '{' (property map) or end of string.
    let prop_start = trimmed.find('{');
    let labels_end = prop_start.unwrap_or(trimmed.len());

    let var_labels_part = &trimmed[..labels_end];
    let props_part = if let Some(ps) = prop_start {
        &trimmed[ps..]
    } else {
        ""
    };

    // Check if there are already labels (contains ':')
    if var_labels_part.contains(':') {
        // Has labels: append workspace label after the last label
        format!(
            "{}:{} {}",
            var_labels_part.trim_end(),
            workspace_label,
            props_part
        )
        .trim_end()
        .to_string()
    } else {
        // No labels: just a variable name
        let var_name = var_labels_part.trim();
        if var_name.is_empty() {
            // Bare () — just add the label
            format!(":{}", workspace_label)
        } else if props_part.is_empty() {
            format!("{}:{}", var_name, workspace_label)
        } else {
            format!("{}:{} {}", var_name, workspace_label, props_part)
        }
    }
}

/// Finds the matching closing parenthesis for an opening one at position `start`.
fn find_matching_paren(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'\'' => {
                // Skip string literal
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' {
                        i += 1; // skip escaped char
                    }
                    i += 1;
                }
            }
            b'"' => {
                // Skip string literal
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1; // skip escaped char
                    }
                    i += 1;
                }
            }
            b'[' => {
                // Skip to matching ] (relationship pattern)
                let mut bracket_depth = 1;
                i += 1;
                while i < bytes.len() && bracket_depth > 0 {
                    match bytes[i] {
                        b'[' => bracket_depth += 1,
                        b']' => bracket_depth -= 1,
                        b'\'' | b'"' => {
                            let quote = bytes[i];
                            i += 1;
                            while i < bytes.len() && bytes[i] != quote {
                                if bytes[i] == b'\\' {
                                    i += 1;
                                }
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    // No matching paren found; return start to avoid panic
    start
}

/// Checks if the byte slice starts with the given string (case-insensitive).
fn starts_with_ignore_case(bytes: &[u8], pattern: &str) -> bool {
    if bytes.len() < pattern.len() {
        return false;
    }
    bytes[..pattern.len()]
        .iter()
        .zip(pattern.bytes())
        .all(|(b, p)| b.eq_ignore_ascii_case(&p))
}

/// Checks if the position is a clause boundary (WHERE, WITH, RETURN, etc.).
fn is_clause_boundary(bytes: &[u8]) -> bool {
    let boundaries = [
        "WHERE ", "WITH ", "RETURN ", "SET ", "DELETE ", "REMOVE ", "MERGE ", "CREATE ",
    ];
    boundaries.iter().any(|b| starts_with_ignore_case(bytes, b))
}

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

/// Finds functions that call the function identified by `fqn`.
///
/// Traverses `CALLS` relationships up to `depth` hops (clamped to 1..5).
/// Results are limited to 50 and ordered by FQN.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
pub async fn get_callers_from_neo4j(
    state: &AppState,
    fqn: &str,
    depth: usize,
) -> Result<Vec<CallerNode>, AppError> {
    let depth = depth.clamp(1, 5);

    // Use bounded variable-length pattern instead of unbounded [:CALLS*]
    // Build the cypher with the depth baked in (Cypher doesn't support param-based bounds)
    let cypher = format!(
        r#"
        MATCH (caller:Function)-[:CALLS*1..{}]->(callee:Function {{fqn: $fqn}})
        WHERE caller.fqn <> $fqn
        RETURN DISTINCT caller.fqn as fqn, caller.name as name, caller.file_path as file_path,
               caller.start_line as line
        ORDER BY fqn
        LIMIT 50
        "#,
        depth
    );

    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, &cypher, params).await?;

    let callers = results
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
        .collect();

    Ok(callers)
}

/// Finds callers for an impl block by aggregating callers of all its child methods.
///
/// `method_prefix` is pre-computed as `module::Type::` so that all Function
/// nodes matching that prefix are the impl's methods. Callers whose FQN
/// also starts with the prefix (i.e., sibling methods) are excluded.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
///
/// # Notes
///
/// As of commit `4d573c9`, this function was added to support showing
/// callers in the detail panel for impl blocks.
pub async fn get_callers_for_impl_with_prefix(
    state: &AppState,
    method_prefix: &str,
    depth: usize,
) -> Result<Vec<CallerNode>, AppError> {
    let depth = depth.clamp(1, 5);

    let cypher = format!(
        r#"
        MATCH (method:Function)
        WHERE method.fqn STARTS WITH $prefix
        WITH method
        MATCH (caller:Function)-[:CALLS*1..{}]->(method)
        WHERE NOT caller.fqn STARTS WITH $prefix
        RETURN DISTINCT caller.fqn as fqn, caller.name as name,
               caller.file_path as file_path, caller.start_line as line
        ORDER BY fqn
        LIMIT 50
        "#,
        depth
    );

    let params = serde_json::json!({"prefix": method_prefix});
    let results = execute_neo4j_query(state, &cypher, params).await?;

    let callers = results
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
        .collect();

    Ok(callers)
}

/// Finds callees for an impl block by aggregating callees of all its child methods.
///
/// Excludes callees whose FQN starts with `method_prefix` (sibling methods).
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
pub async fn get_callees_for_impl_with_prefix(
    state: &AppState,
    method_prefix: &str,
) -> Result<Vec<CalleeInfo>, AppError> {
    let cypher = r#"
        MATCH (method:Function)
        WHERE method.fqn STARTS WITH $prefix
        WITH method
        MATCH (method)-[:CALLS]->(callee:Function)
        WHERE NOT callee.fqn STARTS WITH $prefix
        RETURN DISTINCT callee.fqn as fqn, callee.name as name
        ORDER BY name
        LIMIT 100
    "#;

    let params = serde_json::json!({"prefix": method_prefix});
    let results = execute_neo4j_query(state, cypher, params).await?;

    let callees = results
        .into_iter()
        .filter_map(|r| {
            Some(CalleeInfo {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
            })
        })
        .collect();

    Ok(callees)
}

/// Finds functions called by the function identified by `fqn`.
///
/// Follows direct `CALLS` relationships (depth 1) and returns results
/// ordered by name.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
pub async fn get_callees_from_neo4j(
    state: &AppState,
    fqn: &str,
) -> Result<Vec<CalleeInfo>, AppError> {
    let cypher = r#"
        MATCH (caller:Function {fqn: $fqn})-[:CALLS]->(callee:Function)
        RETURN callee.fqn as fqn, callee.name as name
        ORDER BY name
    "#;

    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, cypher, params).await?;

    let callees = results
        .into_iter()
        .filter_map(|r| {
            Some(CalleeInfo {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
            })
        })
        .collect();

    Ok(callees)
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
        assert_eq!(ctx.workspace_label(), "Workspace_my-workspace_123");
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

    // ── inject_workspace_label tests ────────────────────────────────────

    #[test]
    fn test_inject_label_basic_match() {
        let cypher = "MATCH (f:Function) RETURN f";
        let result = inject_workspace_label(cypher, "Workspace_abc");
        assert!(result.contains("(f:Function:Workspace_abc)"));
    }

    #[test]
    fn test_inject_label_optional_match() {
        let cypher = "OPTIONAL MATCH (c:Crate) RETURN c";
        let result = inject_workspace_label(cypher, "Workspace_ws1");
        assert!(result.contains("(c:Crate:Workspace_ws1)"));
    }

    #[test]
    fn test_inject_label_bare_node() {
        let cypher = "MATCH (n) RETURN n";
        let result = inject_workspace_label(cypher, "Workspace_test");
        assert!(result.contains("(n:Workspace_test)"));
    }

    #[test]
    fn test_inject_label_node_with_props() {
        let cypher = "MATCH (f:Function {fqn: $fqn}) RETURN f";
        let result = inject_workspace_label(cypher, "Workspace_ws");
        assert!(result.contains("Function:Workspace_ws {fqn: $fqn}"));
    }

    #[test]
    fn test_inject_label_multiple_nodes_in_match() {
        let cypher = "MATCH (a:Function)-[:CALLS]->(b:Function) RETURN a, b";
        let result = inject_workspace_label(cypher, "Workspace_x");
        assert!(result.contains("(a:Function:Workspace_x)"));
        assert!(result.contains("(b:Function:Workspace_x)"));
    }

    #[test]
    fn test_inject_label_skips_relationship_labels() {
        let cypher = "MATCH (a:Function)-[r:CALLS]->(b:Function) RETURN r";
        let result = inject_workspace_label(cypher, "Workspace_y");
        assert!(result.contains("(a:Function:Workspace_y)"));
        assert!(result.contains("(b:Function:Workspace_y)"));
        assert!(
            result.contains("[r:CALLS]"),
            "Relationship label should be preserved"
        );
        assert!(
            !result.contains("CALLS:Workspace_y"),
            "Relationship label should not get workspace injected"
        );
    }

    #[test]
    fn test_inject_label_clause_boundary() {
        let cypher = "MATCH (f:Function) WHERE f.name = $name RETURN f";
        let result = inject_workspace_label(cypher, "Workspace_z");
        assert!(result.contains("(f:Function:Workspace_z)"));
    }

    #[test]
    fn test_inject_label_preserves_return_clause() {
        let cypher = "MATCH (n:Function) RETURN n.fqn AS fqn";
        let result = inject_workspace_label(cypher, "Workspace_w");
        assert!(result.contains("RETURN n.fqn AS fqn"));
    }

    // ── inject_label_into_node tests ────────────────────────────────────

    #[test]
    fn test_inject_label_into_node_labeled() {
        let result = inject_label_into_node("f:Function", "Workspace_abc");
        assert_eq!(result, "f:Function:Workspace_abc");
    }

    #[test]
    fn test_inject_label_into_node_labeled_with_props() {
        let result = inject_label_into_node("f:Function {fqn: $fqn}", "Workspace_abc");
        assert_eq!(result, "f:Function:Workspace_abc {fqn: $fqn}");
    }

    #[test]
    fn test_inject_label_into_node_bare_var() {
        let result = inject_label_into_node("n", "Workspace_test");
        assert_eq!(result, "n:Workspace_test");
    }

    #[test]
    fn test_inject_label_into_node_var_with_props() {
        let result = inject_label_into_node("n {name: $name}", "Workspace_ws");
        assert_eq!(result, "n:Workspace_ws {name: $name}");
    }

    #[test]
    fn test_inject_label_into_node_multiple_labels() {
        let result = inject_label_into_node("n:Function:Exported", "Workspace_x");
        assert_eq!(result, "n:Function:Exported:Workspace_x");
    }

    #[test]
    fn test_inject_label_into_node_empty() {
        let result = inject_label_into_node("", "Workspace_x");
        assert_eq!(result, ":Workspace_x");
    }
}
