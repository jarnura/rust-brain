//! Neo4j graph query handlers.
//!
//! Provides endpoints for graph-based code intelligence:
//! - `GET /tools/get_trait_impls` — find implementations of a trait
//! - `GET /tools/find_usages_of_type` — find usages of a type
//! - `GET /tools/get_module_tree` — hierarchical module structure
//! - `POST /tools/query_graph` — arbitrary read-only Cypher execution

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tracing::debug;

use super::default_limit;
use crate::errors::AppError;
use crate::extractors::WorkspaceId;
use crate::neo4j::WorkspaceGraphClient;
use crate::state::AppState;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Query parameters for `GET /tools/get_trait_impls`.
#[derive(Debug, Deserialize)]
pub struct GetTraitImplsQuery {
    /// Trait name or FQN to search for
    pub trait_name: String,
    /// Maximum results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Response for `GET /tools/get_trait_impls`.
#[derive(Debug, Serialize)]
pub struct TraitImplsResponse {
    /// Echo of the queried trait name
    pub trait_name: String,
    /// Matching implementations
    pub implementations: Vec<TraitImpl>,
}

/// A single trait implementation found in the graph.
#[derive(Debug, Serialize)]
pub struct TraitImpl {
    /// FQN of the impl block
    pub impl_fqn: String,
    /// Name of the implementing type
    pub type_name: String,
    /// Source file path
    pub file_path: String,
    /// Start line of the impl block
    pub start_line: u32,
}

/// Query parameters for `GET /tools/find_usages_of_type`.
#[derive(Debug, Deserialize)]
pub struct FindUsagesOfTypeQuery {
    /// Type name or FQN to search for
    pub type_name: String,
    /// Maximum results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Response for `GET /tools/find_usages_of_type`.
#[derive(Debug, Serialize)]
pub struct UsagesResponse {
    /// Echo of the queried type name
    pub type_name: String,
    /// Items that use this type
    pub usages: Vec<TypeUsage>,
}

/// A code item that uses a specific type.
#[derive(Debug, Serialize)]
pub struct TypeUsage {
    /// FQN of the item using the type
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Item kind (e.g., `"Function"`, `"Struct"`)
    pub kind: String,
    /// Source file path
    pub file_path: String,
    /// Line number
    pub line: u32,
}

/// Query parameters for `GET /tools/get_module_tree`.
#[derive(Debug, Deserialize)]
pub struct GetModuleTreeQuery {
    /// Name of the crate to build the module tree for
    pub crate_name: String,
}

/// Response for `GET /tools/get_module_tree`.
#[derive(Debug, Serialize)]
pub struct ModuleTreeResponse {
    /// Echo of the queried crate name
    pub crate_name: String,
    /// Root node of the module hierarchy
    pub root: ModuleNode,
}

/// A node in the hierarchical module tree.
#[derive(Debug, Serialize)]
pub struct ModuleNode {
    /// Module short name
    pub name: String,
    /// Fully qualified module path
    pub path: String,
    /// Child modules
    pub children: Vec<ModuleNode>,
    /// Non-module items contained in this module
    pub items: Vec<ModuleItem>,
}

/// A non-module item within a module node.
#[derive(Debug, Serialize)]
pub struct ModuleItem {
    /// Item short name
    pub name: String,
    /// Item kind (e.g., `"Function"`, `"Struct"`)
    pub kind: String,
    /// Visibility (e.g., `"pub"`, `"private"`)
    pub visibility: String,
}

/// Request body for `POST /tools/query_graph`.
///
/// Accepts **either** raw Cypher (`query`) or a named template (`query_name`).
/// When both are present, `query` takes precedence.
#[derive(Debug, Deserialize)]
pub struct QueryGraphRequest {
    /// Raw Cypher query string (must be read-only). Mutually exclusive with `query_name`.
    pub query: Option<String>,
    /// Named query template (e.g. `"find_callers"`). Mutually exclusive with `query`.
    pub query_name: Option<String>,
    /// Parameters — bound as Neo4j `$param` for raw Cypher, or used by the template resolver.
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Maximum results hint (default: 10, only used by raw-Cypher callers).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Response for `POST /tools/query_graph`.
#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    /// Raw JSON rows from Neo4j
    pub results: Vec<serde_json::Value>,
    /// Echo of the executed query
    pub query: String,
    /// Number of rows returned
    pub row_count: usize,
}

// =============================================================================
// Handlers
// =============================================================================

/// Finds all implementations of a given trait via Neo4j `IMPLEMENTS` relationships.
///
/// Matches by trait name, FQN substring, or exact FQN.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
pub async fn get_trait_impls(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Query(query): Query<GetTraitImplsQuery>,
) -> Result<Json<TraitImplsResponse>, AppError> {
    debug!("Get trait impls for: {}", query.trait_name);

    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);

    let mut params = std::collections::HashMap::new();
    params.insert(
        "trait_name".to_string(),
        serde_json::json!(query.trait_name),
    );
    params.insert("limit".to_string(), serde_json::json!(query.limit as i64));

    let (cypher, query_params) = super::graph_templates::resolve_with_workspace(
        "get_trait_impls_api",
        &params,
        &client.workspace().workspace_label(),
    )?;

    let results = client.execute_query(&cypher, query_params).await?;

    let implementations = results
        .into_iter()
        .filter_map(|r| {
            let impl_name = r.get("impl_name").and_then(|v| v.as_str()).unwrap_or("");
            let type_name = if impl_name.contains('_') {
                impl_name.split('_').nth(1).unwrap_or(impl_name).to_string()
            } else {
                impl_name.to_string()
            };

            Some(TraitImpl {
                impl_fqn: r.get("impl_fqn")?.as_str()?.to_string(),
                type_name,
                file_path: r
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                start_line: r.get("start_line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
            })
        })
        .collect();

    Ok(Json(TraitImplsResponse {
        trait_name: query.trait_name,
        implementations,
    }))
}

/// Finds all items that reference a given type via Neo4j `USES_TYPE` relationships.
///
/// Matches by type name, exact FQN, or FQN suffix (`::TypeName`).
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails.
pub async fn find_usages_of_type(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Query(query): Query<FindUsagesOfTypeQuery>,
) -> Result<Json<UsagesResponse>, AppError> {
    debug!("Find usages of type: {}", query.type_name);

    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);

    let mut params = std::collections::HashMap::new();
    params.insert("type_name".to_string(), serde_json::json!(query.type_name));
    params.insert("limit".to_string(), serde_json::json!(query.limit as i64));

    let (cypher, query_params) = super::graph_templates::resolve_with_workspace(
        "find_usages_of_type",
        &params,
        &client.workspace().workspace_label(),
    )?;

    let results = client.execute_query(&cypher, query_params).await?;

    let usages = results
        .into_iter()
        .filter_map(|r| {
            Some(TypeUsage {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
                kind: r.get("kind")?.as_str()?.to_string(),
                file_path: r.get("file_path")?.as_str().unwrap_or("").to_string(),
                line: r.get("line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
            })
        })
        .collect();

    Ok(Json(UsagesResponse {
        type_name: query.type_name,
        usages,
    }))
}

/// Builds a hierarchical module tree for a crate from Neo4j.
///
/// Queries `Module` nodes, `CONTAINS` relationships, and non-module items.
/// Assembles a recursive tree with a synthetic crate-root node.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the Cypher query fails. Returns an
/// empty tree (root with no children) if no modules are found.
pub async fn get_module_tree(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Query(query): Query<GetModuleTreeQuery>,
) -> Result<Json<ModuleTreeResponse>, AppError> {
    debug!("Get module tree for crate: {}", query.crate_name);

    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);

    let mut params = std::collections::HashMap::new();
    params.insert(
        "crate_name".to_string(),
        serde_json::json!(query.crate_name),
    );

    let (cypher, query_params) = super::graph_templates::resolve_with_workspace(
        "get_module_tree",
        &params,
        &client.workspace().workspace_label(),
    )?;

    let results = client.execute_query(&cypher, query_params).await?;

    let root = if let Some(first) = results.first() {
        // Parse all modules
        let modules_map: HashMap<String, ModuleNode> = first
            .get("all_modules")
            .and_then(|v| v.as_array())
            .map(|mods| {
                mods.iter()
                    .filter_map(|m| {
                        let fqn = m.get("fqn")?.as_str()?.to_string();
                        let name = m
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some((
                            fqn.clone(),
                            ModuleNode {
                                name,
                                path: fqn,
                                children: vec![],
                                items: vec![],
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Parse module hierarchy (parent -> children)
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_parent: HashSet<String> = HashSet::new();

        if let Some(hierarchy) = first.get("module_hierarchy").and_then(|v| v.as_array()) {
            for rel in hierarchy {
                if let (Some(parent_fqn), Some(child_fqn)) = (
                    rel.get("parent").and_then(|v| v.as_str()),
                    rel.get("child").and_then(|v| v.as_str()),
                ) {
                    children_map
                        .entry(parent_fqn.to_string())
                        .or_default()
                        .push(child_fqn.to_string());
                    has_parent.insert(child_fqn.to_string());
                }
            }
        }

        // Parse items grouped by module
        let mut items_map: HashMap<String, Vec<ModuleItem>> = HashMap::new();
        if let Some(items) = first.get("all_items").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(module_fqn) = item.get("module_fqn").and_then(|v| v.as_str()) {
                    items_map
                        .entry(module_fqn.to_string())
                        .or_default()
                        .push(ModuleItem {
                            name: item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            kind: item
                                .get("kind")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string(),
                            visibility: item
                                .get("visibility")
                                .and_then(|v| v.as_str())
                                .unwrap_or("private")
                                .to_string(),
                        });
                }
            }
        }

        // Build tree structure
        let mut modules_map = modules_map;

        // Assign items to modules
        for (fqn, items) in items_map {
            if let Some(module) = modules_map.get_mut(&fqn) {
                module.items = items;
            }
        }

        // Find root modules (modules without a parent within the same crate)
        let root_modules: Vec<String> = modules_map
            .keys()
            .filter(|fqn| !has_parent.contains(*fqn))
            .cloned()
            .collect();

        // Build children recursively
        fn build_children(
            fqn: &str,
            modules_map: &mut HashMap<String, ModuleNode>,
            children_map: &HashMap<String, Vec<String>>,
        ) -> ModuleNode {
            let mut node = modules_map.remove(fqn).unwrap_or_else(|| ModuleNode {
                name: fqn.split("::").last().unwrap_or(fqn).to_string(),
                path: fqn.to_string(),
                children: vec![],
                items: vec![],
            });

            if let Some(child_fqns) = children_map.get(fqn) {
                for child_fqn in child_fqns {
                    let child_node = build_children(child_fqn, modules_map, children_map);
                    node.children.push(child_node);
                }
            }

            node
        }

        // Build tree starting from root modules
        let mut root_children: Vec<ModuleNode> = Vec::new();
        for root_fqn in &root_modules {
            let node = build_children(root_fqn, &mut modules_map, &children_map);
            root_children.push(node);
        }

        // Create synthetic crate root since there's no actual crate root Module node
        ModuleNode {
            name: query.crate_name.clone(),
            path: query.crate_name.clone(),
            children: root_children,
            items: vec![],
        }
    } else {
        ModuleNode {
            name: query.crate_name.clone(),
            path: query.crate_name.clone(),
            children: vec![],
            items: vec![],
        }
    };

    Ok(Json(ModuleTreeResponse {
        crate_name: query.crate_name,
        root,
    }))
}

/// APOC procedure namespaces that are permitted in read-only queries.
///
/// Any `CALL apoc.<namespace>.*` not in this list is rejected to prevent
/// write/side-effect APOC procedures from modifying the graph
/// (e.g., `apoc.create.node`, `apoc.do.when`, `apoc.periodic.commit`).
const APOC_READONLY_NAMESPACES: &[&str] = &[
    "apoc.path.",
    "apoc.algo.",
    "apoc.coll.",
    "apoc.text.",
    "apoc.map.",
    "apoc.convert.",
    "apoc.date.",
    "apoc.meta.",
    "apoc.util.",
    "apoc.help",
    "apoc.version",
];

/// APOC procedure prefixes that re-enter the Cypher planner — always rejected.
///
/// These procedures can execute arbitrary Cypher strings, bypassing read-only
/// restrictions and workspace label injection. They must be blocked even if
/// they don't match standard write keywords.
const APOC_PLANNER_REENTRY_PREFIXES: &[&str] = &[
    "apoc.cypher.run",
    "apoc.cypher.runmany",
    "apoc.cypher.runfile",
    "apoc.do.when",
    "apoc.do.case",
    "apoc.when",
    "apoc.case",
    "apoc.periodic.commit",
    "apoc.periodic.iterate",
    "apoc.graph.fromcypher",
];

/// Cypher write keyword tokens that must be rejected in raw user queries.
const CYPHER_WRITE_TOKENS: &[&str] = &["create", "delete", "set", "remove", "merge"];

/// Validates that a raw Cypher string is read-only.
///
/// Rejects:
/// - Queries containing DML write keywords (CREATE, DELETE, SET, REMOVE, MERGE)
/// - CALL of any APOC procedure not in [`APOC_READONLY_NAMESPACES`]
/// - CALL of any APOC procedure in [`APOC_PLANNER_REENTRY_PREFIXES`]
fn validate_cypher(query: &str) -> Result<(), AppError> {
    let stripped = super::workspace_label::strip_comments(query)?;
    let query_lower = super::workspace_label::normalize_whitespace(&stripped.to_lowercase());

    // First check: reject planner-reentry APOC procedures
    if query_lower.contains("call apoc.") {
        let mut remaining = query_lower.as_str();
        while let Some(pos) = remaining.find("call apoc.") {
            let after_call = &remaining[pos + 5..];
            let proc_name: String = after_call
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '(')
                .collect();

            for prefix in APOC_PLANNER_REENTRY_PREFIXES {
                if proc_name.starts_with(prefix) {
                    return Err(AppError::BadRequest(format!(
                        "APOC procedure '{}' is not allowed — it can execute arbitrary Cypher",
                        proc_name
                    )));
                }
            }

            remaining = &remaining[pos + 10..];
        }
    }

    // Reject write keywords
    for keyword in CYPHER_WRITE_TOKENS {
        // Split on whitespace and punctuation to avoid false-positive substring matches
        if query_lower
            .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
            .any(|token| token == *keyword)
        {
            return Err(AppError::BadRequest(
                "Only read-only queries are allowed".to_string(),
            ));
        }
    }

    // Validate APOC procedure calls
    // Pattern: `CALL apoc.xxx.yyy` — detect by looking for `call apoc.` tokens
    if query_lower.contains("call apoc.") {
        // Extract the procedure name following each `call apoc.` occurrence
        let mut remaining = query_lower.as_str();
        while let Some(pos) = remaining.find("call apoc.") {
            // Advance past `call ` to point at `apoc.xxx`
            let after_call = &remaining[pos + 5..];
            // Collect the procedure identifier up to the first whitespace or `(`
            let proc_name: String = after_call
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '(')
                .collect();

            let is_allowed = APOC_READONLY_NAMESPACES
                .iter()
                .any(|ns| proc_name.starts_with(ns));

            if !is_allowed {
                return Err(AppError::BadRequest(format!(
                    "APOC procedure '{}' is not in the read-only allowlist",
                    proc_name
                )));
            }

            // Advance past this occurrence to look for the next
            remaining = &remaining[pos + 10..];
        }
    }

    // Reject LOAD CSV — SSRF vector
    if query_lower
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .any(|token| token == "load")
    {
        return Err(AppError::BadRequest(
            "LOAD CSV is not allowed — potential SSRF vector".to_string(),
        ));
    }

    Ok(())
}

/// Executes a read-only Cypher query against Neo4j.
///
/// Supports two request formats:
/// - **Raw Cypher**: `{"query": "MATCH ..."}` — validated to reject writes,
///   workspace labels injected server-side.
/// - **Named template**: `{"query_name": "find_callers", "parameters": {...}}`
///   — resolved to workspace-scoped Cypher via [`super::graph_templates::resolve_with_workspace`].
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if the raw query contains write keywords,
/// APOC write procedures, if neither `query` nor `query_name` is provided,
/// or if template resolution fails (unknown template, missing params, invalid label).
/// Returns [`AppError::Neo4j`] if execution fails.
pub async fn query_graph(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Json(mut req): Json<QueryGraphRequest>,
) -> Result<Json<GraphQueryResponse>, AppError> {
    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);

    // Defense-in-depth: strip any workspace-related parameters to prevent injection attempts
    super::workspace_label::sanitize_parameters(&mut req.parameters);

    let (cypher, results) = if let Some(ref query) = req.query {
        validate_cypher(query)?;
        let params = serde_json::Value::Object(req.parameters.into_iter().collect());
        let results = client.execute_user_cypher(query, params).await?;
        (query.clone(), results)
    } else if let Some(ref query_name) = req.query_name {
        let (cypher, params) = crate::handlers::graph_templates::resolve_with_workspace(
            query_name,
            &req.parameters,
            &client.workspace().workspace_label(),
        )?;
        let results = client.execute_query(&cypher, params).await?;
        (cypher, results)
    } else {
        return Err(AppError::BadRequest(
            "Either 'query' (raw Cypher) or 'query_name' (template name) is required".into(),
        ));
    };

    debug!("Executing Cypher query: {}", cypher);

    let row_count = results.len();

    Ok(Json(GraphQueryResponse {
        query: cypher,
        results,
        row_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_cypher_request_deserialization() {
        let json = serde_json::json!({
            "query": "MATCH (n) RETURN n LIMIT 10",
            "parameters": {"name": "test"},
            "limit": 20
        });

        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query.as_deref(), Some("MATCH (n) RETURN n LIMIT 10"));
        assert!(req.query_name.is_none());
        assert_eq!(req.parameters.get("name").unwrap(), "test");
        assert_eq!(req.limit, 20);
    }

    #[test]
    fn test_template_request_deserialization() {
        let json = serde_json::json!({
            "query_name": "find_callers",
            "parameters": {"name": "foo", "limit": 5}
        });

        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert!(req.query.is_none());
        assert_eq!(req.query_name.as_deref(), Some("find_callers"));
        assert_eq!(req.parameters.get("name").unwrap(), "foo");
        assert_eq!(req.parameters.get("limit").unwrap(), 5);
    }

    #[test]
    fn test_request_with_neither_query_nor_template() {
        let json = serde_json::json!({
            "parameters": {"name": "foo"}
        });

        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert!(req.query.is_none());
        assert!(req.query_name.is_none());
    }

    #[test]
    fn test_request_defaults() {
        let json = serde_json::json!({"query": "MATCH (n) RETURN n"});
        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert!(req.parameters.is_empty());
        assert_eq!(req.limit, 10);
    }

    // --- validate_cypher tests ---

    #[test]
    fn test_rejects_create_keyword() {
        assert!(validate_cypher("CREATE (n:Node) RETURN n").is_err());
    }

    #[test]
    fn test_rejects_delete_keyword() {
        assert!(validate_cypher("MATCH (n) DELETE n").is_err());
    }

    #[test]
    fn test_rejects_merge_keyword() {
        assert!(validate_cypher("MERGE (n:Node {id: 1}) RETURN n").is_err());
    }

    #[test]
    fn test_rejects_set_keyword() {
        assert!(validate_cypher("MATCH (n) SET n.x = 1 RETURN n").is_err());
    }

    #[test]
    fn test_rejects_remove_keyword() {
        assert!(validate_cypher("MATCH (n) REMOVE n.x RETURN n").is_err());
    }

    #[test]
    fn test_allows_match_return() {
        assert!(validate_cypher("MATCH (n:Function) RETURN n.name LIMIT 10").is_ok());
    }

    #[test]
    fn test_rejects_apoc_create_node() {
        // Rejected: "create" keyword fires (tokenization splits on `.`)
        // The APOC whitelist check is a second line of defence for write-only APOC
        // procedures whose names don't happen to contain a write keyword.
        let q = "CALL apoc.create.node(['Label'], {name: 'test'})";
        assert!(
            validate_cypher(q).is_err(),
            "should reject apoc.create.node"
        );
    }

    #[test]
    fn test_rejects_apoc_do_when() {
        assert!(validate_cypher("CALL apoc.do.when(true, 'CREATE (n) RETURN n', '')").is_err());
    }

    #[test]
    fn test_rejects_apoc_periodic_commit() {
        assert!(validate_cypher("CALL apoc.periodic.commit('MATCH ...')").is_err());
    }

    #[test]
    fn test_allows_apoc_path_expand() {
        assert!(validate_cypher(
            "CALL apoc.path.expand(startNode, 'CALLS>', null, 1, 3) YIELD path RETURN path"
        )
        .is_ok());
    }

    #[test]
    fn test_allows_apoc_algo_dijkstra() {
        assert!(validate_cypher(
            "CALL apoc.algo.dijkstra(a, b, 'EDGE', 'cost') YIELD path RETURN path"
        )
        .is_ok());
    }

    #[test]
    fn test_rejects_apoc_refactor() {
        assert!(validate_cypher("CALL apoc.refactor.mergeNodes([n1, n2])").is_err());
    }

    #[test]
    fn test_rejects_apoc_cypher_runfile() {
        assert!(validate_cypher("CALL apoc.cypher.runFile('file.cyp')").is_err());
    }

    #[test]
    fn test_allows_apoc_meta_stats() {
        assert!(validate_cypher("CALL apoc.meta.stats() YIELD labels RETURN labels").is_ok());
    }

    #[test]
    fn test_rejects_apoc_export() {
        assert!(validate_cypher("CALL apoc.export.csv.all('out.csv', {})").is_err());
    }

    #[test]
    fn test_rejects_apoc_cypher_run() {
        let err = validate_cypher("CALL apoc.cypher.run('MATCH (n) RETURN n', {})").unwrap_err();
        assert!(err.to_string().contains("apoc.cypher.run"));
        assert!(err.to_string().contains("not allowed"));
    }

    #[test]
    fn test_rejects_apoc_cypher_run_many() {
        let err =
            validate_cypher("CALL apoc.cypher.runMany('MATCH (n) RETURN n', [{}])").unwrap_err();
        assert!(err.to_string().contains("apoc.cypher.runmany"));
    }

    #[test]
    fn test_rejects_apoc_do_case() {
        let err =
            validate_cypher("CALL apoc.do.case([true, 'RETURN 1'], 'RETURN 0', {})").unwrap_err();
        assert!(err.to_string().contains("apoc.do.case"));
    }

    #[test]
    fn test_rejects_apoc_when() {
        let err = validate_cypher("CALL apoc.when(true, 'RETURN 1', 'RETURN 0', {})").unwrap_err();
        assert!(err.to_string().contains("apoc.when"));
    }

    #[test]
    fn test_rejects_apoc_case() {
        let err =
            validate_cypher("CALL apoc.case([true, 'RETURN 1'], 'RETURN 0', {})").unwrap_err();
        assert!(err.to_string().contains("apoc.case"));
    }

    #[test]
    fn test_rejects_apoc_periodic_iterate() {
        let err = validate_cypher(
            "CALL apoc.periodic.iterate('MATCH (n) RETURN n', 'SET n.x = 1', {batchSize: 100})",
        )
        .unwrap_err();
        assert!(err.to_string().contains("apoc.periodic.iterate"));
    }

    #[test]
    fn test_rejects_apoc_graph_fromcypher() {
        let err = validate_cypher(
            "CALL apoc.graph.fromCypher('MATCH (n) RETURN n', {}) YIELD graph RETURN graph",
        )
        .unwrap_err();
        assert!(err.to_string().contains("apoc.graph.fromcypher"));
    }

    #[test]
    fn test_rejects_apoc_graph_namespace() {
        assert!(
            validate_cypher("CALL apoc.graph.fromDB('neo4j', {}) YIELD graph RETURN graph")
                .is_err()
        );
    }

    #[test]
    fn test_rejects_load_csv() {
        assert!(
            validate_cypher("LOAD CSV FROM 'http://example.com/data.csv' AS line RETURN line")
                .is_err()
        );
    }

    #[test]
    fn test_rejects_load_csv_with_headers() {
        assert!(validate_cypher(
            "LOAD CSV WITH HEADERS FROM 'http://example.com/data.csv' AS row RETURN row"
        )
        .is_err());
    }

    #[test]
    fn test_rejects_load_csv_whitespace_variant() {
        assert!(validate_cypher(
            "LOAD\tCSV FROM 'http://169.254.169.254/latest/meta-data/' AS line RETURN line"
        )
        .is_err());
    }

    #[test]
    fn test_rejects_apoc_call_double_space() {
        assert!(validate_cypher("CALL  apoc.cypher.run('MATCH (n) RETURN n', {})").is_err());
    }

    #[test]
    fn test_rejects_apoc_call_tab() {
        assert!(validate_cypher("CALL\tapoc.cypher.run('MATCH (n) RETURN n', {})").is_err());
    }

    #[test]
    fn test_rejects_apoc_call_newline() {
        assert!(validate_cypher("CALL\napoc.cypher.run('MATCH (n) RETURN n', {})").is_err());
    }

    #[test]
    fn test_rejects_apoc_hidden_in_comment() {
        let err = validate_cypher("CALL /* sneaky */ apoc.cypher.run('MATCH (n) RETURN n', {})")
            .unwrap_err();
        assert!(err.to_string().contains("apoc.cypher.run"));
    }
}
