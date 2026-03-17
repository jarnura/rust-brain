//! Neo4j graph query handlers.

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

#[derive(Debug, Deserialize)]
pub struct GetTraitImplsQuery {
    pub trait_name: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
pub struct TraitImplsResponse {
    pub trait_name: String,
    pub implementations: Vec<TraitImpl>,
}

#[derive(Debug, Serialize)]
pub struct TraitImpl {
    pub impl_fqn: String,
    pub type_name: String,
    pub file_path: String,
    pub start_line: u32,
}

#[derive(Debug, Deserialize)]
pub struct FindUsagesOfTypeQuery {
    pub type_name: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
pub struct UsagesResponse {
    pub type_name: String,
    pub usages: Vec<TypeUsage>,
}

#[derive(Debug, Serialize)]
pub struct TypeUsage {
    pub fqn: String,
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line: u32,
}

#[derive(Debug, Deserialize)]
pub struct GetModuleTreeQuery {
    pub crate_name: String,
}

#[derive(Debug, Serialize)]
pub struct ModuleTreeResponse {
    pub crate_name: String,
    pub root: ModuleNode,
}

#[derive(Debug, Serialize)]
pub struct ModuleNode {
    pub name: String,
    pub path: String,
    pub children: Vec<ModuleNode>,
    pub items: Vec<ModuleItem>,
}

#[derive(Debug, Serialize)]
pub struct ModuleItem {
    pub name: String,
    pub kind: String,
    pub visibility: String,
}

#[derive(Debug, Deserialize)]
pub struct QueryGraphRequest {
    pub query: String,
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
pub struct GraphQueryResponse {
    pub results: Vec<serde_json::Value>,
    pub query: String,
    pub row_count: usize,
}

// =============================================================================
// Handlers
// =============================================================================

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
