//! Named query template resolution for `POST /tools/query_graph`.
//!
//! Converts `(query_name, parameters)` pairs (sent by the MCP tool) into
//! `(cypher, params)` tuples suitable for [`crate::neo4j::execute_neo4j_query`].
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

/// Resolves a named query template into `(cypher, params)`.
///
/// Returns `Err(AppError::BadRequest)` for unknown templates, missing required
/// parameters, or invalid label values.
pub fn resolve(
    query_name: &str,
    parameters: &HashMap<String, serde_json::Value>,
) -> Result<(String, serde_json::Value), AppError> {
    let limit = extract_limit(parameters);

    match query_name {
        "find_functions_by_name" => {
            let name = require_str(parameters, &["name"])?;
            Ok((
                "MATCH (f:Function) WHERE f.name = $name \
                 AND f.file_path IS NOT NULL \
                 RETURN f.fqn AS fqn, f.name AS name, f.file_path AS file_path, \
                 f.start_line AS start_line, f.visibility AS visibility, \
                 f.signature AS signature \
                 LIMIT $limit"
                    .into(),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_callers" => {
            let name = require_str(parameters, &["name", "fqn"])?;
            let depth = clamp_depth(parameters);
            Ok((
                format!(
                    "MATCH (caller)-[:CALLS*1..{}]->(target) \
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
                "MATCH (source)-[:CALLS]->(callee) \
                 WHERE (source.name = $name OR source.fqn = $name) \
                 AND callee.file_path IS NOT NULL \
                 RETURN DISTINCT callee.fqn AS fqn, callee.name AS name, \
                 callee.file_path AS file_path \
                 LIMIT $limit"
                    .into(),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_trait_implementations" => {
            let name = require_str(parameters, &["name"])?;
            Ok((
                "MATCH (impl)-[:IMPLEMENTS]->(trait) \
                 WHERE trait.name = $name OR trait.fqn = $name \
                 AND impl.file_path IS NOT NULL \
                 RETURN impl.fqn AS fqn, impl.name AS name, \
                 impl.file_path AS file_path, impl.for_type AS for_type \
                 LIMIT $limit"
                    .into(),
                serde_json::json!({"name": name, "limit": limit}),
            ))
        }

        "find_by_fqn" => {
            let fqn = require_str(parameters, &["fqn", "name"])?;
            let label = label_or_default(parameters, "Function")?;
            Ok((
                format!(
                    "MATCH (n:{label}) WHERE n.fqn = $fqn OR n.name = $fqn \
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
                    "MATCH (n)-[r*1..{depth}]-(neighbor) \
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
                    "MATCH (n:{label}) RETURN n.fqn AS fqn, n.name AS name \
                     LIMIT $limit"
                ),
                serde_json::json!({"limit": limit}),
            ))
        }

        "find_module_contents" => {
            let path = require_str(parameters, &["path", "name"])?;
            Ok((
                "MATCH (m:Module)-[:CONTAINS]->(item) \
                 WHERE m.fqn = $path OR m.name = $path \
                 RETURN item.fqn AS fqn, item.name AS name, \
                 labels(item) AS labels, item.visibility AS visibility \
                 LIMIT $limit"
                    .into(),
                serde_json::json!({"path": path, "limit": limit}),
            ))
        }

        "count_by_label" => {
            let label = require_label(parameters)?;
            Ok((
                format!("MATCH (n:{label}) RETURN count(n) AS count"),
                serde_json::json!({}),
            ))
        }

        "find_crate_overview" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                "MATCH (c:Crate) WHERE c.name = $crate_name \
                 OPTIONAL MATCH (c)-[:CONTAINS]->(item) \
                 WITH c, labels(item)[0] AS item_type, count(item) AS cnt \
                 RETURN c.name AS name, item_type, cnt \
                 ORDER BY cnt DESC"
                    .into(),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "find_crate_dependencies" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                "MATCH (c:Crate)-[:DEPENDS_ON]->(dep:Crate) \
                 WHERE c.name = $crate_name \
                 RETURN dep.name AS name \
                 ORDER BY dep.name"
                    .into(),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        "find_crate_dependents" => {
            let crate_name = require_str(parameters, &["crate_name"])?;
            Ok((
                "MATCH (dependent:Crate)-[:DEPENDS_ON]->(c:Crate) \
                 WHERE c.name = $crate_name \
                 RETURN dependent.name AS name \
                 ORDER BY dependent.name"
                    .into(),
                serde_json::json!({"crate_name": crate_name}),
            ))
        }

        _ => Err(AppError::BadRequest(format!(
            "Unknown query template: '{}'. Available: find_functions_by_name, \
             find_callers, find_callees, find_trait_implementations, find_by_fqn, \
             find_neighbors, find_nodes_by_label, find_module_contents, count_by_label, \
             find_crate_overview, find_crate_dependencies, find_crate_dependents",
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
    fn find_functions_by_name_basic() {
        let (cypher, p) = resolve(
            "find_functions_by_name",
            &params(&[("name", serde_json::json!("main"))]),
        )
        .unwrap();
        assert!(cypher.contains("MATCH (f:Function)"));
        assert!(cypher.contains("$name"));
        assert!(cypher.contains("$limit"));
        assert_eq!(p["name"], "main");
        assert_eq!(p["limit"], 25);
    }

    #[test]
    fn find_callers_with_depth_and_limit() {
        let (cypher, p) = resolve(
            "find_callers",
            &params(&[
                ("name", serde_json::json!("handle_request")),
                ("depth", serde_json::json!(3)),
                ("limit", serde_json::json!(10)),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("[:CALLS*1..3]"));
        assert!(cypher.contains("$name"));
        assert_eq!(p["name"], "handle_request");
        assert_eq!(p["limit"], 10);
    }

    #[test]
    fn find_callers_accepts_fqn_key() {
        let (_, p) = resolve(
            "find_callers",
            &params(&[("fqn", serde_json::json!("crate::module::func"))]),
        )
        .unwrap();
        assert_eq!(p["name"], "crate::module::func");
    }

    #[test]
    fn find_callees_basic() {
        let (cypher, p) = resolve(
            "find_callees",
            &params(&[("name", serde_json::json!("process"))]),
        )
        .unwrap();
        assert!(cypher.contains("[:CALLS]->"));
        assert_eq!(p["name"], "process");
    }

    #[test]
    fn find_trait_implementations_basic() {
        let (cypher, p) = resolve(
            "find_trait_implementations",
            &params(&[("name", serde_json::json!("Iterator"))]),
        )
        .unwrap();
        assert!(cypher.contains("[:IMPLEMENTS]"));
        assert_eq!(p["name"], "Iterator");
    }

    #[test]
    fn find_by_fqn_default_label() {
        let (cypher, p) = resolve(
            "find_by_fqn",
            &params(&[("fqn", serde_json::json!("crate::main"))]),
        )
        .unwrap();
        assert!(cypher.contains("(n:Function)"));
        assert_eq!(p["fqn"], "crate::main");
    }

    #[test]
    fn find_by_fqn_custom_label() {
        let (cypher, _) = resolve(
            "find_by_fqn",
            &params(&[
                ("fqn", serde_json::json!("crate::MyStruct")),
                ("label", serde_json::json!("Struct")),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("(n:Struct)"));
    }

    #[test]
    fn find_neighbors_with_depth() {
        let (cypher, p) = resolve(
            "find_neighbors",
            &params(&[
                ("fqn", serde_json::json!("crate::func")),
                ("depth", serde_json::json!(2)),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("[r*1..2]"));
        assert!(cypher.contains("type(last(r))"));
        assert_eq!(p["fqn"], "crate::func");
    }

    #[test]
    fn find_nodes_by_label_basic() {
        let (cypher, p) = resolve(
            "find_nodes_by_label",
            &params(&[("label", serde_json::json!("Trait"))]),
        )
        .unwrap();
        assert!(cypher.contains("(n:Trait)"));
        assert!(p.get("name").is_none());
    }

    #[test]
    fn find_module_contents_basic() {
        let (cypher, p) = resolve(
            "find_module_contents",
            &params(&[("path", serde_json::json!("crate::handlers"))]),
        )
        .unwrap();
        assert!(cypher.contains("[:CONTAINS]"));
        assert_eq!(p["path"], "crate::handlers");
    }

    #[test]
    fn count_by_label_no_params() {
        let (cypher, p) = resolve(
            "count_by_label",
            &params(&[("label", serde_json::json!("Function"))]),
        )
        .unwrap();
        assert!(cypher.contains("(n:Function)"));
        assert!(cypher.contains("count(n)"));
        assert!(p.as_object().unwrap().is_empty());
    }

    #[test]
    fn find_crate_overview_basic() {
        let (cypher, p) = resolve(
            "find_crate_overview",
            &params(&[("crate_name", serde_json::json!("api"))]),
        )
        .unwrap();
        assert!(cypher.contains("$crate_name"));
        assert_eq!(p["crate_name"], "api");
    }

    #[test]
    fn find_crate_dependencies_basic() {
        let (cypher, p) = resolve(
            "find_crate_dependencies",
            &params(&[("crate_name", serde_json::json!("router"))]),
        )
        .unwrap();
        assert!(cypher.contains("[:DEPENDS_ON]"));
        assert!(cypher.contains("dep.name"));
        assert_eq!(p["crate_name"], "router");
    }

    #[test]
    fn find_crate_dependents_basic() {
        let (cypher, p) = resolve(
            "find_crate_dependents",
            &params(&[("crate_name", serde_json::json!("common"))]),
        )
        .unwrap();
        assert!(cypher.contains("[:DEPENDS_ON]"));
        assert!(cypher.contains("dependent.name"));
        assert_eq!(p["crate_name"], "common");
    }

    // ── error cases ────────────────────────────────────────────────────

    #[test]
    fn unknown_template_returns_error() {
        let err = resolve("nonexistent", &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Unknown query template"));
        assert!(msg.contains("nonexistent"));
    }

    #[test]
    fn missing_required_name() {
        let err = resolve("find_functions_by_name", &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Missing required parameter"));
        assert!(msg.contains("name"));
    }

    #[test]
    fn missing_required_label() {
        let err = resolve("find_nodes_by_label", &HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("label"));
    }

    #[test]
    fn invalid_label_rejected() {
        let err = resolve(
            "find_nodes_by_label",
            &params(&[("label", serde_json::json!("DropTable"))]),
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Invalid label"));
        assert!(msg.contains("DropTable"));
    }

    #[test]
    fn invalid_label_in_find_by_fqn_rejected() {
        let result = resolve(
            "find_by_fqn",
            &params(&[
                ("fqn", serde_json::json!("x")),
                ("label", serde_json::json!("BadLabel")),
            ]),
        );
        assert!(result.is_err());
    }

    #[test]
    fn missing_crate_name_for_dependencies() {
        let err = resolve("find_crate_dependencies", &HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("crate_name"));
    }

    // ── defaults & clamping ────────────────────────────────────────────

    #[test]
    fn depth_clamped_to_max_5() {
        let (cypher, _) = resolve(
            "find_callers",
            &params(&[
                ("name", serde_json::json!("f")),
                ("depth", serde_json::json!(99)),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("[:CALLS*1..5]"));
    }

    #[test]
    fn depth_clamped_to_min_1() {
        let (cypher, _) = resolve(
            "find_callers",
            &params(&[
                ("name", serde_json::json!("f")),
                ("depth", serde_json::json!(0)),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("[:CALLS*1..1]"));
    }

    #[test]
    fn limit_clamped_to_100() {
        let (_, p) = resolve(
            "find_functions_by_name",
            &params(&[
                ("name", serde_json::json!("f")),
                ("limit", serde_json::json!(999)),
            ]),
        )
        .unwrap();
        assert_eq!(p["limit"], 100);
    }

    #[test]
    fn limit_defaults_to_25() {
        let (_, p) = resolve(
            "find_functions_by_name",
            &params(&[("name", serde_json::json!("f"))]),
        )
        .unwrap();
        assert_eq!(p["limit"], 25);
    }

    #[test]
    fn depth_parsed_from_string() {
        let (cypher, _) = resolve(
            "find_callers",
            &params(&[
                ("name", serde_json::json!("f")),
                ("depth", serde_json::json!("3")),
            ]),
        )
        .unwrap();
        assert!(cypher.contains("[:CALLS*1..3]"));
    }

    #[test]
    fn limit_parsed_from_string() {
        let (_, p) = resolve(
            "find_functions_by_name",
            &params(&[
                ("name", serde_json::json!("f")),
                ("limit", serde_json::json!("50")),
            ]),
        )
        .unwrap();
        assert_eq!(p["limit"], 50);
    }
}
