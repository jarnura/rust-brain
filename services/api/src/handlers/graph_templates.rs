//! Named query template resolution for `POST /tools/query_graph`.
//!
//! Converts `(query_name, parameters)` pairs (sent by the MCP tool) into
//! `(cypher, params)` tuples suitable for [`crate::neo4j::WorkspaceGraphClient::resolve_and_execute_template`].
//!
//! User-provided property values (names, FQNs, crate names) are always bound
//! as Neo4j parameters (`$param`) — never interpolated into Cypher strings.
//! Only validated labels and clamped depth bounds are interpolated.

use std::collections::HashMap;

use crate::errors::AppError;

/// Node labels that may appear in dynamic-label queries.
const VALID_LABELS: &[&str] = &[
    "Function",
    "Struct",
    "Enum",
    "Trait",
    "Impl",
    "Module",
    "Crate",
    "Type",
    "TypeAlias",
    "Const",
    "Static",
    "Macro",
];

// ── helpers ─────────────────────────────────────────────────────────────────

/// Extracts a required string parameter, trying `keys` in order.
fn require_str(
    params: &HashMap<String, serde_json::Value>,
    keys: &[&str],
) -> Result<String, AppError> {
    for key in keys {
        if let Some(value) = params.get(*key).and_then(|v| v.as_str()) {
            return Ok(value.to_string());
        }
    }
    Err(AppError::BadRequest(format!(
        "Missing required parameter: {}",
        keys.join(" or "),
    )))
}

/// Extracts `label`, validates against [`VALID_LABELS`].
fn require_label(params: &HashMap<String, serde_json::Value>) -> Result<&'static str, AppError> {
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("Missing required parameter: label".into()))?;
    validate_label(label)
}

/// Extracts `label` with a fallback default, validates against [`VALID_LABELS`].
fn label_or_default(
    params: &HashMap<String, serde_json::Value>,
    default: &str,
) -> Result<&'static str, AppError> {
    let label = params
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or(default);
    validate_label(label)
}

fn validate_label(label: &str) -> Result<&'static str, AppError> {
    VALID_LABELS
        .iter()
        .find(|&&valid| valid == label)
        .copied()
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Invalid label: '{}'. Valid labels: {}",
                label,
                VALID_LABELS.join(", "),
            ))
        })
}

/// Extracts `limit`, defaulting to 25 and clamping to 1..=100.
fn extract_limit(params: &HashMap<String, serde_json::Value>) -> i64 {
    params
        .get("limit")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(25)
        .clamp(1, 100)
}

/// Extracts `depth`, defaulting to 1 and clamping to 1..=5.
fn clamp_depth(params: &HashMap<String, serde_json::Value>) -> i64 {
    params
        .get("depth")
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(1)
        .clamp(1, 5)
}

// ── public API ──────────────────────────────────────────────────────────────

/// Resolves a named query template with workspace labels injected.
///
/// Same as the legacy `resolve` but every node pattern in the generated Cypher
/// receives the `workspace_label` composite label (e.g., `Workspace_abc123`).
/// Called by [`crate::neo4j::WorkspaceGraphClient::resolve_and_execute_template`].
pub fn resolve_with_workspace(
    query_name: &str,
    parameters: &HashMap<String, serde_json::Value>,
    workspace_label: &str,
) -> Result<(String, serde_json::Value), AppError> {
    let limit = extract_limit(parameters);
    let ws = workspace_label;

    match query_name {
        "find_functions_by_name" => {
            let name = require_str(parameters, &["name"])?;
            Ok((
                format!(
                    "MATCH (f:Function:{ws}) WHERE f.name = $name \
                     AND f.file_path IS NOT NULL \
                     RETURN f.fqn AS fqn, f.name AS name, f.file_path AS file_path, \
                     f.start_line AS start_line, f.visibility AS visibility, \
                     f.signature AS signature \
                     LIMIT $limit"
                ),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_callers" => {
            let name = require_str(parameters, &["name", "fqn"])?;
            let depth = clamp_depth(parameters);
            Ok((
                format!(
                    "MATCH (caller:{ws})-[:CALLS*1..{}]->(target:{ws}) \
                     WHERE (target.name = $name OR target.fqn = $name) \
                     AND caller.file_path IS NOT NULL \
                     RETURN DISTINCT caller.fqn AS fqn, caller.name AS name, \
                     caller.file_path AS file_path, caller.start_line AS start_line \
                     LIMIT $limit",
                    depth
                ),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_callees" => {
            let name = require_str(parameters, &["name", "fqn"])?;
            Ok((
                format!(
                    "MATCH (source:{ws})-[:CALLS]->(callee:{ws}) \
                     WHERE (source.name = $name OR source.fqn = $name) \
                     AND callee.file_path IS NOT NULL \
                     RETURN DISTINCT callee.fqn AS fqn, callee.name AS name, \
                     callee.file_path AS file_path \
                     LIMIT $limit"
                ),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_trait_implementations" => {
            let name = require_str(parameters, &["name"])?;
            Ok((
                format!(
                    "MATCH (impl:{ws})-[:IMPLEMENTS]->(trait:{ws}) \
                     WHERE trait.name = $name OR trait.fqn = $name \
                     AND impl.file_path IS NOT NULL \
                     RETURN impl.fqn AS fqn, impl.name AS name, \
                     impl.file_path AS file_path, impl.for_type AS for_type \
                     LIMIT $limit"
                ),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_by_fqn" => {
            let fqn = require_str(parameters, &["fqn", "name"])?;
            let label = label_or_default(parameters, "Function")?;
            Ok((
                format!(
                    "MATCH (n:{label}:{ws}) WHERE n.fqn = $fqn OR n.name = $fqn \
                     RETURN n \
                     LIMIT $limit"
                ),
                serde_json::json!({"fqn": fqn, "limit": limit}),
            ))
        }

        "find_neighbors" => {
            let fqn = require_str(parameters, &["fqn", "name"])?;
            let depth = clamp_depth(parameters);
            Ok((
                format!(
                    "MATCH (n:{ws})-[r*1..{depth}]-(neighbor:{ws}) \
                     WHERE n.fqn = $fqn OR n.name = $fqn \
                     RETURN DISTINCT type(last(r)) AS relationship, \
                     neighbor.fqn AS fqn, neighbor.name AS name, \
                     labels(neighbor) AS labels \
                     LIMIT $limit"
                ),
                serde_json::json!({"fqn": fqn, "limit": limit}),
            ))
        }

        "find_nodes_by_label" => {
            let label = require_label(parameters)?;
            Ok((
                format!(
                    "MATCH (n:{label}:{ws}) RETURN n.fqn AS fqn, n.name AS name \
                     LIMIT $limit"
                ),
                serde_json::json!({"limit": limit}),
            ))
        }

        "find_module_contents" => {
            let path = require_str(parameters, &["path", "name"])?;
            Ok((
                format!(
                    "MATCH (m:Module:{ws})-[:CONTAINS]->(item:{ws}) \
                     WHERE m.fqn = $path OR m.name = $path \
                     RETURN item.fqn AS fqn, item.name AS name, \
                     labels(item) AS labels, item.visibility AS visibility \
                     LIMIT $limit"
                ),
                serde_json::json!({"path": path, "limit": limit}),
            ))
        }

        "count_by_label" => {
            let label = require_label(parameters)?;
            Ok((
                format!("MATCH (n:{label}:{ws}) RETURN count(n) AS count"),
                serde_json::json!({}),
            ))
        }

        "find_crate_overview" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (c:Crate:{ws}) WHERE c.name = $crate_name \
                     OPTIONAL MATCH (c)-[:CONTAINS]->(item:{ws}) \
                     WITH c, labels(item)[0] AS item_type, count(item) AS cnt \
                     RETURN c.name AS name, item_type, cnt \
                     ORDER BY cnt DESC"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "find_crate_dependencies" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (c:Crate:{ws})-[:DEPENDS_ON]->(dep:Crate:{ws}) \
                     WHERE c.name = $crate_name \
                     RETURN dep.name AS name \
                     ORDER BY dep.name"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "find_crate_dependents" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (dependent:Crate:{ws})-[:DEPENDS_ON]->(c:Crate:{ws}) \
                     WHERE c.name = $crate_name \
                     RETURN dependent.name AS name \
                     ORDER BY dependent.name"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "get_trait_impls_api" => {
            let trait_name = require_str(parameters, &["trait_name"])?;
            Ok((
                format!(
                    "MATCH (impl:Impl:{ws})-[:IMPLEMENTS]->(trait:Trait:{ws}) \
                     WHERE trait.name = $trait_name OR trait.fqn CONTAINS $trait_name OR trait.fqn = $trait_name \
                     RETURN impl.fqn AS impl_fqn, impl.name AS impl_name, trait.name AS trait_name, trait.fqn AS trait_fqn, \
                     impl.start_line AS start_line \
                     LIMIT $limit"
                ),
                serde_json::json!({"trait_name": trait_name, "limit": limit}),
            ))
        }

        "find_usages_of_type" => {
            let type_name = require_str(parameters, &["type_name"])?;
            let fqn_suffix = format!("::{}", type_name);
            Ok((
                format!(
                    "MATCH (n:{ws})-[:USES_TYPE]->(t:{ws}) \
                     WHERE (t:Struct OR t:Enum OR t:Trait OR t:TypeAlias OR t:Type) \
                     AND (t.name = $type_name OR t.fqn = $type_name OR t.fqn ENDS WITH $fqn_suffix) \
                     RETURN n.fqn AS fqn, n.name AS name, labels(n)[0] AS kind, n.file_path AS file_path, n.start_line AS line \
                     LIMIT $limit"
                ),
                serde_json::json!({"type_name": type_name, "fqn_suffix": fqn_suffix, "limit": limit}),
            ))
        }

        "get_module_tree" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (m:Module:{ws}) \
                     WHERE split(m.fqn, '::')[0] = $crate_name \
                     WITH collect(m) AS all_modules \
                     OPTIONAL MATCH (parent:Module:{ws})-[:CONTAINS]->(child:Module:{ws}) \
                     WHERE split(parent.fqn, '::')[0] = $crate_name AND split(child.fqn, '::')[0] = $crate_name \
                     WITH all_modules, collect({{parent: parent.fqn, child: child.fqn}}) AS module_hierarchy \
                     OPTIONAL MATCH (m:Module:{ws})-[:CONTAINS]->(item:{ws}) \
                     WHERE split(m.fqn, '::')[0] = $crate_name AND NOT item:Module \
                     WITH all_modules, module_hierarchy, \
                     collect({{module_fqn: m.fqn, name: item.name, kind: labels(item)[0], visibility: item.visibility}}) AS all_items \
                     RETURN all_modules, module_hierarchy, all_items"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "get_callers" => {
            let fqn = require_str(parameters, &["fqn"])?;
            let depth = clamp_depth(parameters);
            Ok((
                format!(
                    "MATCH (caller:Function:{ws})-[:CALLS*1..{depth}]->(callee:Function:{ws} {{fqn: $fqn}}) \
                     WHERE caller.fqn <> $fqn \
                     RETURN DISTINCT caller.fqn AS fqn, caller.name AS name, caller.file_path AS file_path, \
                     caller.start_line AS line \
                     ORDER BY fqn \
                     LIMIT $limit"
                ),
                serde_json::json!({"fqn": fqn, "limit": limit}),
            ))
        }

        "get_callers_for_impl" => {
            let prefix = require_str(parameters, &["prefix"])?;
            let depth = clamp_depth(parameters);
            Ok((
                format!(
                    "MATCH (method:Function:{ws}) \
                     WHERE method.fqn STARTS WITH $prefix \
                     WITH method \
                     MATCH (caller:Function:{ws})-[:CALLS*1..{depth}]->(method) \
                     WHERE NOT caller.fqn STARTS WITH $prefix \
                     RETURN DISTINCT caller.fqn AS fqn, caller.name AS name, \
                     caller.file_path AS file_path, caller.start_line AS line \
                     ORDER BY fqn \
                     LIMIT $limit"
                ),
                serde_json::json!({"prefix": prefix, "limit": limit}),
            ))
        }

        "get_callees_for_impl" => {
            let prefix = require_str(parameters, &["prefix"])?;
            Ok((
                format!(
                    "MATCH (method:Function:{ws}) \
                     WHERE method.fqn STARTS WITH $prefix \
                     WITH method \
                     MATCH (method)-[:CALLS]->(callee:Function:{ws}) \
                     WHERE NOT callee.fqn STARTS WITH $prefix \
                     RETURN DISTINCT callee.fqn AS fqn, callee.name AS name \
                     ORDER BY name \
                     LIMIT $limit"
                ),
                serde_json::json!({"prefix": prefix, "limit": limit}),
            ))
        }

        "get_callees" => {
            let fqn = require_str(parameters, &["fqn"])?;
            Ok((
                format!(
                    "MATCH (caller:Function:{ws} {{fqn: $fqn}})-[:CALLS]->(callee:Function:{ws}) \
                     RETURN callee.fqn AS fqn, callee.name AS name \
                     ORDER BY name"
                ),
                serde_json::json!({"fqn": fqn}),
            ))
        }

        "consistency_fqns" => Ok((
            format!(
                "MATCH (n:Function:{ws}) RETURN n.fqn AS fqn \
                     UNION MATCH (n:Struct:{ws}) RETURN n.fqn AS fqn \
                     UNION MATCH (n:Enum:{ws}) RETURN n.fqn AS fqn \
                     UNION MATCH (n:Trait:{ws}) RETURN n.fqn AS fqn \
                     UNION MATCH (n:Impl:{ws}) RETURN n.fqn AS fqn \
                     UNION MATCH (n:TypeAlias:{ws}) RETURN n.fqn AS fqn"
            ),
            serde_json::json!({}),
        )),

        "consistency_fqns_filtered" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (n:Function:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn \
                     UNION MATCH (n:Struct:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn \
                     UNION MATCH (n:Enum:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn \
                     UNION MATCH (n:Trait:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn \
                     UNION MATCH (n:Impl:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn \
                     UNION MATCH (n:TypeAlias:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN n.fqn AS fqn"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "consistency_count" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                format!(
                    "MATCH (n:Function:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count \
                     UNION MATCH (n:Struct:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count \
                     UNION MATCH (n:Enum:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count \
                     UNION MATCH (n:Trait:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count \
                     UNION MATCH (n:Impl:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count \
                     UNION MATCH (n:TypeAlias:{ws}) WHERE split(n.fqn, '::')[0] = $crate_name RETURN count(n) AS count"
                ),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        _ => Err(AppError::BadRequest(format!(
            "Unknown query template: '{}'. Available: find_functions_by_name, \
             find_callers, find_callees, find_trait_implementations, find_by_fqn, \
             find_neighbors, find_nodes_by_label, find_module_contents, count_by_label, \
             find_crate_overview, find_crate_dependencies, find_crate_dependents, \
             get_trait_impls_api, find_usages_of_type, get_module_tree, \
             get_callers, get_callers_for_impl, get_callees_for_impl, get_callees, \
             consistency_fqns, consistency_fqns_filtered, consistency_count",
            query_name,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, serde_json::Value)]) -> HashMap<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    // ── happy paths ────────────────────────────────────────────────────

    #[test]
    fn resolve_with_workspace_injects_label() {
        let (cypher, _) = resolve_with_workspace(
            "find_functions_by_name",
            &params(&[("name", serde_json::json!("main"))]),
            "Workspace_abc",
        )
        .unwrap();
        assert!(cypher.contains("Function:Workspace_abc"));
    }

    #[test]
    fn resolve_with_workspace_find_callers() {
        let (cypher, _) = resolve_with_workspace(
            "find_callers",
            &params(&[
                ("name", serde_json::json!("f")),
                ("depth", serde_json::json!(2)),
            ]),
            "Workspace_ws1",
        )
        .unwrap();
        assert!(cypher.contains("caller:Workspace_ws1"));
        assert!(cypher.contains("target:Workspace_ws1"));
    }

    #[test]
    fn resolve_with_workspace_find_nodes_by_label() {
        let (cypher, _) = resolve_with_workspace(
            "find_nodes_by_label",
            &params(&[("label", serde_json::json!("Struct"))]),
            "Workspace_test",
        )
        .unwrap();
        assert!(cypher.contains("Struct:Workspace_test"));
    }

    #[test]
    fn resolve_with_workspace_find_crate_overview() {
        let (cypher, _) = resolve_with_workspace(
            "find_crate_overview",
            &params(&[("crate_name", serde_json::json!("api"))]),
            "Workspace_xyz",
        )
        .unwrap();
        assert!(cypher.contains("Crate:Workspace_xyz"));
        assert!(cypher.contains("item:Workspace_xyz"));
    }

    #[test]
    fn resolve_with_workspace_unknown_template() {
        let err =
            resolve_with_workspace("nonexistent", &HashMap::new(), "Workspace_abc").unwrap_err();
        assert!(err.to_string().contains("Unknown query template"));
    }

    #[test]
    fn resolve_with_workspace_find_by_fqn_with_label() {
        let (cypher, _) = resolve_with_workspace(
            "find_by_fqn",
            &params(&[
                ("fqn", serde_json::json!("crate::MyStruct")),
                ("label", serde_json::json!("Struct")),
            ]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("Struct:Workspace_ws"));
    }

    #[test]
    fn resolve_get_trait_impls_api() {
        let (cypher, params) = resolve_with_workspace(
            "get_trait_impls_api",
            &params(&[("trait_name", serde_json::json!("Clone"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("Impl:Workspace_ws"));
        assert!(cypher.contains("Trait:Workspace_ws"));
        assert_eq!(params["trait_name"], "Clone");
    }

    #[test]
    fn resolve_find_usages_of_type() {
        let (cypher, params) = resolve_with_workspace(
            "find_usages_of_type",
            &params(&[("type_name", serde_json::json!("String"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("USES_TYPE"));
        assert!(cypher.contains(":Workspace_ws"));
        assert_eq!(params["type_name"], "String");
        assert_eq!(params["fqn_suffix"], "::String");
    }

    #[test]
    fn resolve_get_module_tree() {
        let (cypher, params) = resolve_with_workspace(
            "get_module_tree",
            &params(&[("crate_name", serde_json::json!("my_crate"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("Module:Workspace_ws"));
        assert!(cypher.contains("CONTAINS"));
        assert_eq!(params["crate_name"], "my_crate");
    }

    #[test]
    fn resolve_get_callers() {
        let (cypher, params) = resolve_with_workspace(
            "get_callers",
            &params(&[
                ("fqn", serde_json::json!("crate::func")),
                ("depth", serde_json::json!(2)),
            ]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("Function:Workspace_ws"));
        assert!(cypher.contains("CALLS*1..2"));
        assert_eq!(params["fqn"], "crate::func");
    }

    #[test]
    fn resolve_get_callers_for_impl() {
        let (cypher, params) = resolve_with_workspace(
            "get_callers_for_impl",
            &params(&[
                ("prefix", serde_json::json!("crate::Type::")),
                ("depth", serde_json::json!(1)),
            ]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("STARTS WITH $prefix"));
        assert!(cypher.contains("CALLS*1..1"));
        assert_eq!(params["prefix"], "crate::Type::");
    }

    #[test]
    fn resolve_get_callees_for_impl() {
        let (cypher, params) = resolve_with_workspace(
            "get_callees_for_impl",
            &params(&[("prefix", serde_json::json!("crate::Type::"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("STARTS WITH $prefix"));
        assert!(cypher.contains("[:CALLS]->"));
        assert_eq!(params["prefix"], "crate::Type::");
    }

    #[test]
    fn resolve_get_callees() {
        let (cypher, params) = resolve_with_workspace(
            "get_callees",
            &params(&[("fqn", serde_json::json!("crate::func"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("{fqn: $fqn}"));
        assert!(cypher.contains("[:CALLS]->"));
        assert_eq!(params["fqn"], "crate::func");
    }

    #[test]
    fn resolve_consistency_fqns() {
        let (cypher, _) =
            resolve_with_workspace("consistency_fqns", &HashMap::new(), "Workspace_ws").unwrap();
        assert!(cypher.contains("Function:Workspace_ws"));
        assert!(cypher.contains("Struct:Workspace_ws"));
        assert!(cypher.contains("UNION"));
    }

    #[test]
    fn resolve_consistency_fqns_filtered() {
        let (cypher, params) = resolve_with_workspace(
            "consistency_fqns_filtered",
            &params(&[("crate_name", serde_json::json!("api"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("split(n.fqn, '::')[0] = $crate_name"));
        assert_eq!(params["crate_name"], "api");
    }

    #[test]
    fn resolve_consistency_count() {
        let (cypher, params) = resolve_with_workspace(
            "consistency_count",
            &params(&[("crate_name", serde_json::json!("api"))]),
            "Workspace_ws",
        )
        .unwrap();
        assert!(cypher.contains("count(n)"));
        assert_eq!(params["crate_name"], "api");
    }

    #[test]
    fn resolve_get_trait_impls_api_missing_param() {
        let err = resolve_with_workspace("get_trait_impls_api", &HashMap::new(), "Workspace_ws")
            .unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_find_usages_of_type_missing_param() {
        let err = resolve_with_workspace("find_usages_of_type", &HashMap::new(), "Workspace_ws")
            .unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_get_module_tree_missing_param() {
        let err =
            resolve_with_workspace("get_module_tree", &HashMap::new(), "Workspace_ws").unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_get_callers_missing_fqn() {
        let err =
            resolve_with_workspace("get_callers", &HashMap::new(), "Workspace_ws").unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_get_callees_missing_fqn() {
        let err =
            resolve_with_workspace("get_callees", &HashMap::new(), "Workspace_ws").unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_consistency_fqns_filtered_missing_crate() {
        let err =
            resolve_with_workspace("consistency_fqns_filtered", &HashMap::new(), "Workspace_ws")
                .unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }

    #[test]
    fn resolve_consistency_count_missing_crate() {
        let err = resolve_with_workspace("consistency_count", &HashMap::new(), "Workspace_ws")
            .unwrap_err();
        assert!(err.to_string().contains("Missing required parameter"));
    }
}
