//! Neo4j helper functions for the rust-brain API server.
//!
//! Provides low-level Cypher execution ([`execute_neo4j_query`]) and
//! higher-level call-graph traversal functions used by the handler layer.
//! All functions return [`AppError::Neo4j`] on failure.

use crate::errors::AppError;
use crate::handlers::{CalleeInfo, CallerNode};
use crate::state::AppState;

/// Verifies the Neo4j connection by executing `RETURN 1`.
///
/// # Errors
///
/// Returns [`AppError::Neo4j`] if the query fails or the connection is refused.
pub async fn check_neo4j(state: &AppState) -> Result<(), AppError> {
    let mut result = state
        .neo4j_graph
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

    let mut result = state
        .neo4j_graph
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
}
