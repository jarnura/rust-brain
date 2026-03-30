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

use crate::errors::AppError;
use crate::neo4j::execute_neo4j_query;
use crate::state::AppState;
use super::default_limit;

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
#[derive(Debug, Deserialize)]
pub struct QueryGraphRequest {
    /// Cypher query string (must be read-only)
    pub query: String,
    /// Named parameters to bind into the query
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    /// Maximum results (default: 10)
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
    Query(query): Query<GetTraitImplsQuery>,
) -> Result<Json<TraitImplsResponse>, AppError> {
    state.metrics.record_request("get_trait_impls", "GET");
    debug!("Get trait impls for: {}", query.trait_name);

    let cypher = r#"
        MATCH (impl:Impl)-[:IMPLEMENTS]->(trait:Trait)
        WHERE trait.name = $trait_name OR trait.fqn CONTAINS $trait_name OR trait.fqn = $trait_name
        RETURN impl.fqn as impl_fqn, impl.name as impl_name, trait.name as trait_name, trait.fqn as trait_fqn,
               impl.start_line as start_line
        LIMIT $limit
        "#;

    let params = serde_json::json!({
        "trait_name": query.trait_name,
        "limit": query.limit as i32,
    });

    let results = execute_neo4j_query(&state, cypher, params).await?;

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
                file_path: r.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
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
    Query(query): Query<FindUsagesOfTypeQuery>,
) -> Result<Json<UsagesResponse>, AppError> {
    state.metrics.record_request("find_usages_of_type", "GET");
    debug!("Find usages of type: {}", query.type_name);

    let cypher = r#"
        MATCH (n)-[:USES_TYPE]->(t)
        WHERE (t:Struct OR t:Enum OR t:Trait OR t:TypeAlias OR t:Type)
        AND (t.name = $type_name OR t.fqn = $type_name OR t.fqn ENDS WITH $fqn_suffix)
        RETURN n.fqn as fqn, n.name as name, labels(n)[0] as kind, n.file_path as file_path, n.start_line as line
        LIMIT $limit
        "#;

    let fqn_suffix = format!("::{}", query.type_name);

    let params = serde_json::json!({
        "type_name": query.type_name,
        "fqn_suffix": fqn_suffix,
        "limit": query.limit as i32,
    });

    let results = execute_neo4j_query(&state, cypher, params).await?;

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
    Query(query): Query<GetModuleTreeQuery>,
) -> Result<Json<ModuleTreeResponse>, AppError> {
    state.metrics.record_request("get_module_tree", "GET");
    debug!("Get module tree for crate: {}", query.crate_name);

    let cypher = r#"
        // Get all modules for this crate (crate name is first part of FQN)
        MATCH (m:Module)
        WHERE split(m.fqn, '::')[0] = $crate_name
        WITH collect(m) as all_modules

        // Get all parent-child module relationships within this crate
        OPTIONAL MATCH (parent:Module)-[:CONTAINS]->(child:Module)
        WHERE split(parent.fqn, '::')[0] = $crate_name AND split(child.fqn, '::')[0] = $crate_name
        WITH all_modules, collect({parent: parent.fqn, child: child.fqn}) as module_hierarchy

        // Get items for each module (using CONTAINS, not DEFINES)
        OPTIONAL MATCH (m:Module)-[:CONTAINS]->(item)
        WHERE split(m.fqn, '::')[0] = $crate_name AND NOT item:Module
        WITH all_modules, module_hierarchy,
             collect({module_fqn: m.fqn, name: item.name, kind: labels(item)[0], visibility: item.visibility}) as all_items

        RETURN all_modules, module_hierarchy, all_items
        "#;

    let params = serde_json::json!({
        "crate_name": query.crate_name,
    });

    let results = execute_neo4j_query(&state, cypher, params).await?;

    let root = if let Some(first) = results.first() {
        // Parse all modules
        let modules_map: HashMap<String, ModuleNode> = first.get("all_modules")
            .and_then(|v| v.as_array())
            .map(|mods| {
                mods.iter().filter_map(|m| {
                    let fqn = m.get("fqn")?.as_str()?.to_string();
                    let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    Some((fqn.clone(), ModuleNode {
                        name,
                        path: fqn,
                        children: vec![],
                        items: vec![],
                    }))
                }).collect()
            })
            .unwrap_or_default();

        // Parse module hierarchy (parent -> children)
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut has_parent: HashSet<String> = HashSet::new();

        if let Some(hierarchy) = first.get("module_hierarchy").and_then(|v| v.as_array()) {
            for rel in hierarchy {
                if let (Some(parent_fqn), Some(child_fqn)) = (
                    rel.get("parent").and_then(|v| v.as_str()),
                    rel.get("child").and_then(|v| v.as_str())
                ) {
                    children_map.entry(parent_fqn.to_string()).or_default().push(child_fqn.to_string());
                    has_parent.insert(child_fqn.to_string());
                }
            }
        }

        // Parse items grouped by module
        let mut items_map: HashMap<String, Vec<ModuleItem>> = HashMap::new();
        if let Some(items) = first.get("all_items").and_then(|v| v.as_array()) {
            for item in items {
                if let Some(module_fqn) = item.get("module_fqn").and_then(|v| v.as_str()) {
                    items_map.entry(module_fqn.to_string()).or_default().push(ModuleItem {
                        name: item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        kind: item.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                        visibility: item.get("visibility").and_then(|v| v.as_str()).unwrap_or("private").to_string(),
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
        let root_modules: Vec<String> = modules_map.keys()
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

/// Executes an arbitrary read-only Cypher query against Neo4j.
///
/// The query is validated to reject write operations (`CREATE`, `DELETE`,
/// `SET`, `REMOVE`, `MERGE`) before execution.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if the query contains write keywords.
/// Returns [`AppError::Neo4j`] if execution fails.
pub async fn query_graph(
    State(state): State<AppState>,
    Json(req): Json<QueryGraphRequest>,
) -> Result<Json<GraphQueryResponse>, AppError> {
    state.metrics.record_request("query_graph", "POST");

    // Validate query is read-only
    let query_lower = req.query.to_lowercase();
    if query_lower.contains("create") || query_lower.contains("delete") ||
       query_lower.contains("set") || query_lower.contains("remove") ||
       query_lower.contains("merge") {
        return Err(AppError::BadRequest("Only read-only queries are allowed".to_string()));
    }

    debug!("Executing Cypher query: {}", req.query);

    let results = execute_neo4j_query(&state, &req.query, serde_json::Value::Object(
        req.parameters.into_iter().map(|(k, v)| (k, v)).collect()
    )).await?;

    let row_count = results.len();

    Ok(Json(GraphQueryResponse {
        query: req.query,
        results,
        row_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_graph_request_deserialization() {
        let json = serde_json::json!({
            "query": "MATCH (n) RETURN n LIMIT 10",
            "parameters": {"name": "test"},
            "limit": 20
        });

        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "MATCH (n) RETURN n LIMIT 10");
        assert_eq!(req.parameters.get("name").unwrap(), "test");
        assert_eq!(req.limit, 20);
    }
}
