//! AGE openCypher query functions
//!
//! Ports the 10 core Neo4j Cypher queries to Apache AGE openCypher.
//! All queries use the SQL wrapper pattern:
//! ```sql
//! SELECT * FROM cypher('graph_name', $$ <cypher> $$, $1) AS (result agtype)
//! ```
//!
//! # Key Differences from Neo4j Cypher
//!
//! 1. **ON CREATE SET → COALESCE**: AGE does NOT support `ON CREATE SET` / `ON MATCH SET`
//!    in MERGE. Query #3 uses `SET prop = coalesce(n.prop, default)` instead.
//!
//! 2. **SET n += map → Individual SETs**: AGE does NOT support `SET n += $props` or
//!    `SET n += variable.props` with a map value. Each property must be set individually:
//!    `SET n.key1 = variable.key1, n.key2 = variable.key2, ...`

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::types::{build_agtype_param, build_unwind_param, parse_age_vertex, AgeNode};

// Known node property keys (from Neo4j batch.rs schema)
const NODE_PROPS: &[&str] = &[
    "fqn",
    "name",
    "visibility",
    "is_async",
    "start_line",
    "end_line",
    "file_path",
];

/// Execute an AGE cypher query that requires an agtype parameter.
///
/// AGE's `cypher()` function requires the third argument to be a bare `$1` parameter
/// reference (not a cast like `$1::agtype`). However, sqlx's `.bind()` sends text
/// parameters with the `text` OID, which PostgreSQL can't match to `agtype`.
///
/// This helper uses the PREPARE/EXECUTE pattern within a single transaction so
/// PostgreSQL knows the parameter type is `agtype` before the `cypher()` call is parsed,
/// and both PREPARE and EXECUTE run on the same database connection.
async fn execute_age_query(
    pool: &PgPool,
    sql: &str,
    agtype_param: &str,
    stmt_name: &str,
) -> Result<sqlx::postgres::PgQueryResult> {
    let mut tx = pool.begin().await.context("Failed to begin transaction")?;

    // DEALLOCATE in a PL/pgSQL block that swallows "does not exist" errors
    let deallocate_sql = format!(
        "DO $$ BEGIN DEALLOCATE {name}; EXCEPTION WHEN OTHERS THEN END; $$",
        name = stmt_name
    );
    let _ = sqlx::query(&deallocate_sql).execute(&mut *tx).await;

    let prepare_sql = format!("PREPARE {}(agtype) AS {}", stmt_name, sql);
    sqlx::query(&prepare_sql)
        .execute(&mut *tx)
        .await
        .context("Failed to PREPARE AGE statement")?;

    let exec_sql = format!(
        "EXECUTE {}('{}')",
        stmt_name,
        agtype_param.replace('\'', "''")
    );
    let result = sqlx::query(&exec_sql)
        .execute(&mut *tx)
        .await
        .context("Failed to EXECUTE AGE statement")?;

    let _ = sqlx::query(&format!("DEALLOCATE {}", stmt_name))
        .execute(&mut *tx)
        .await;

    tx.commit().await.context("Failed to commit transaction")?;
    Ok(result)
}

/// Execute an AGE cypher query that returns rows with an agtype parameter.
///
/// Same as `execute_age_query` but returns rows instead of just the execute result.
async fn fetch_age_query(
    pool: &PgPool,
    sql: &str,
    agtype_param: &str,
    stmt_name: &str,
) -> Result<Vec<sqlx::postgres::PgRow>> {
    let mut tx = pool.begin().await.context("Failed to begin transaction")?;

    let deallocate_sql = format!(
        "DO $$ BEGIN DEALLOCATE {name}; EXCEPTION WHEN OTHERS THEN END; $$",
        name = stmt_name
    );
    let _ = sqlx::query(&deallocate_sql).execute(&mut *tx).await;

    let prepare_sql = format!("PREPARE {}(agtype) AS {}", stmt_name, sql);
    sqlx::query(&prepare_sql)
        .execute(&mut *tx)
        .await
        .context("Failed to PREPARE AGE statement")?;

    let exec_sql = format!(
        "EXECUTE {}('{}')",
        stmt_name,
        agtype_param.replace('\'', "''")
    );
    let rows = sqlx::query(&exec_sql)
        .fetch_all(&mut *tx)
        .await
        .context("Failed to EXECUTE AGE statement")?;

    let _ = sqlx::query(&format!("DEALLOCATE {}", stmt_name))
        .execute(&mut *tx)
        .await;

    tx.commit().await.context("Failed to commit transaction")?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Query #1 — Batch node insert (UNWIND)
// Source: batch.rs:238-279
// ---------------------------------------------------------------------------

/// Insert a batch of nodes using UNWIND, grouped by a single label.
///
/// AGE does NOT support `SET n += variable.props`. Properties are set individually
/// from top-level fields in the UNWIND data.
pub async fn batch_insert_nodes(
    pool: &PgPool,
    graph_name: &str,
    label: &str,
    nodes: &[serde_json::Value],
) -> Result<usize> {
    if nodes.is_empty() {
        return Ok(0);
    }

    let set_clauses: Vec<String> = NODE_PROPS
        .iter()
        .map(|p| format!("n.{} = node_data.{}", p, p))
        .collect();
    let set_clause = format!(" SET {}", set_clauses.join(", "));

    let cypher = format!(
        "UNWIND $nodes AS node_data \
         MERGE (n:{label} {{id: node_data.id}}){set_clause}",
        label = label,
        set_clause = set_clause
    );

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let flattened: Vec<serde_json::Value> = nodes.iter().map(|n| flatten_node_data(n)).collect();

    let agtype_param = build_unwind_param(&flattened, "nodes");
    debug!(
        "batch_insert_nodes: {} nodes with label {}",
        nodes.len(),
        label
    );

    execute_age_query(pool, &sql, &agtype_param, "batch_insert_nodes")
        .await
        .context(format!("Failed to batch insert {} nodes", label))?;

    info!("Inserted {} nodes with label {}", nodes.len(), label);
    Ok(nodes.len())
}

/// Flatten a node data object: merge nested `props` into top-level.
fn flatten_node_data(node: &serde_json::Value) -> serde_json::Value {
    let mut flat = serde_json::Map::new();
    if let Some(obj) = node.as_object() {
        for (k, v) in obj {
            if k == "props" {
                if let Some(props_obj) = v.as_object() {
                    for (pk, pv) in props_obj {
                        flat.insert(pk.clone(), pv.clone());
                    }
                }
            } else {
                flat.insert(k.clone(), v.clone());
            }
        }
    }
    serde_json::Value::Object(flat)
}

/// Extract unique property keys (excluding structural keys) from flattened items.
fn extract_prop_keys(items: &[serde_json::Value]) -> Vec<String> {
    let structural_keys = ["id", "from_id", "to_id", "fqn", "name"];
    let mut keys = std::collections::BTreeSet::new();
    for item in items {
        if let Some(obj) = item.as_object() {
            for k in obj.keys() {
                if !structural_keys.contains(&k.as_str()) {
                    keys.insert(k.clone());
                }
            }
        }
    }
    keys.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Query #2 — Batch relationship insert with MATCH both ends (UNWIND)
// Source: batch.rs:329-349
// ---------------------------------------------------------------------------

/// Insert a batch of relationships using UNWIND where both endpoints must exist.
///
/// AGE does NOT support `SET r += variable.props`. Relationship properties from
/// the nested `props` field are set individually after flattening.
pub async fn batch_insert_relationships(
    pool: &PgPool,
    graph_name: &str,
    from_label: &str,
    to_label: &str,
    rel_type: &str,
    rels: &[serde_json::Value],
) -> Result<usize> {
    if rels.is_empty() {
        return Ok(0);
    }

    let flattened: Vec<serde_json::Value> = rels.iter().map(|r| flatten_node_data(r)).collect();

    let rel_prop_keys = extract_prop_keys(&flattened);

    let set_clause = if rel_prop_keys.is_empty() {
        String::new()
    } else {
        let clauses: Vec<String> = rel_prop_keys
            .iter()
            .map(|k| format!("r.{} = rel_data.{}", k, k))
            .collect();
        format!(" SET {}", clauses.join(", "))
    };

    let cypher = format!(
        "UNWIND $rels AS rel_data \
         MATCH (from:{from_label} {{id: rel_data.from_id}}) \
         MATCH (to:{to_label} {{id: rel_data.to_id}}) \
         MERGE (from)-[r:{rel_type}]->(to){set_clause}",
        from_label = from_label,
        to_label = to_label,
        rel_type = rel_type,
        set_clause = set_clause
    );

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let agtype_param = build_unwind_param(&flattened, "rels");
    debug!(
        "batch_insert_relationships: {} rels [{}]-[{}]->[{}]",
        rels.len(),
        from_label,
        rel_type,
        to_label
    );

    execute_age_query(pool, &sql, &agtype_param, "batch_ins_rels")
        .await
        .context(format!("Failed to batch insert {} relationships", rel_type))?;

    info!(
        "Inserted {} relationships [{}]-[{}]->[{}]",
        rels.len(),
        from_label,
        rel_type,
        to_label
    );
    Ok(rels.len())
}

// ---------------------------------------------------------------------------
// Query #3 — Batch relationship insert with MERGE target (COALESCE workaround)
// Source: batch.rs:309-328
//
// CRITICAL: This is the query where the COALESCE workaround replaces
// ON CREATE SET. See CEO-mandated edge case tests below.
// ---------------------------------------------------------------------------

/// Insert a batch of relationships using UNWIND where the target node may not
/// exist and is created as a placeholder stub.
///
/// AGE does NOT support `ON CREATE SET`, so we use COALESCE to preserve
/// existing properties on match while setting defaults on create:
///
/// ```cypher
/// UNWIND $rels AS rel_data
/// MATCH (from:FromLabel {id: rel_data.from_id})
/// MERGE (to:ToLabel {id: rel_data.to_id})
/// SET to.fqn = coalesce(to.fqn, rel_data.to_id),
///     to.name = coalesce(to.name, rel_data.to_id),
///     to.external = coalesce(to.external, true)
/// MERGE (from)-[r:REL_TYPE]->(to)
/// SET r += rel_data.props
/// ```
///
/// Relationship types that use this pattern (from batch.rs):
/// HAS_FIELD, HAS_VARIANT, USES_TYPE, FOR, EXTENDS
pub async fn batch_insert_rels_merge_target(
    pool: &PgPool,
    graph_name: &str,
    from_label: &str,
    to_label: &str,
    rel_type: &str,
    rels: &[serde_json::Value],
) -> Result<usize> {
    if rels.is_empty() {
        return Ok(0);
    }

    let flattened: Vec<serde_json::Value> = rels.iter().map(|r| flatten_node_data(r)).collect();

    let rel_prop_keys = extract_prop_keys(&flattened);

    let rel_set_clause = if rel_prop_keys.is_empty() {
        String::new()
    } else {
        let clauses: Vec<String> = rel_prop_keys
            .iter()
            .map(|k| format!("r.{} = rel_data.{}", k, k))
            .collect();
        format!(" SET {}", clauses.join(", "))
    };

    let cypher = format!(
        "UNWIND $rels AS rel_data \
         MATCH (from:{from_label} {{id: rel_data.from_id}}) \
         MERGE (to:{to_label} {{id: rel_data.to_id}}) \
         SET to.fqn = coalesce(to.fqn, rel_data.to_id), \
             to.name = coalesce(to.name, rel_data.to_id), \
             to.external = coalesce(to.external, true) \
         MERGE (from)-[r:{rel_type}]->(to){rel_set_clause}",
        from_label = from_label,
        to_label = to_label,
        rel_type = rel_type,
        rel_set_clause = rel_set_clause
    );

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let agtype_param = build_unwind_param(&flattened, "rels");
    debug!(
        "batch_insert_rels_merge_target: {} rels [{}]-[{}]->[{}] (COALESCE workaround)",
        rels.len(),
        from_label,
        rel_type,
        to_label
    );

    execute_age_query(pool, &sql, &agtype_param, "batch_ins_rels_mt")
        .await
        .context(format!(
            "Failed to batch insert {} relationships (merge target)",
            rel_type
        ))?;

    info!(
        "Inserted {} relationships [{}]-[{}]->[{}] (merge target)",
        rels.len(),
        from_label,
        rel_type,
        to_label
    );
    Ok(rels.len())
}

// ---------------------------------------------------------------------------
// Query #4 — Single node MERGE
// Source: nodes.rs:110-143
// ---------------------------------------------------------------------------

/// Create or update a single node using MERGE (idempotent).
///
/// AGE does NOT support `SET n += $props` with a parameter map.
/// Instead, each property is set individually: `SET n.key1 = $key1, n.key2 = $key2, ...`
pub async fn merge_node(
    pool: &PgPool,
    graph_name: &str,
    label: &str,
    id: &str,
    props: &HashMap<String, serde_json::Value>,
) -> Result<()> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), serde_json::json!(id));

    let set_clauses: Vec<String> = props
        .keys()
        .filter_map(|key| {
            let safe_key = key.replace('-', "_").replace(' ', "_");
            params.insert(safe_key.clone(), props[key].clone());
            Some(format!("n.{} = ${}", key, key))
        })
        .collect();

    let set_clause = if set_clauses.is_empty() {
        String::new()
    } else {
        format!(" SET {}", set_clauses.join(", "))
    };

    let cypher = format!(
        "MERGE (n:{label} {{id: $id}}){set_clause}",
        label = label,
        set_clause = set_clause
    );

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let agtype_param = build_agtype_param(&params);
    debug!("merge_node: {} [{}]", id, label);

    execute_age_query(pool, &sql, &agtype_param, "merge_node")
        .await
        .context(format!("Failed to merge node {} [{}]", id, label))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Query #5 — Single relationship MERGE
// Source: relationships.rs:175-214
// ---------------------------------------------------------------------------

/// Create or update a single relationship using MERGE (idempotent).
///
/// AGE does NOT support `SET r += $props` with a parameter map.
/// Instead, each property is set individually: `SET r.key1 = $key1, ...`
#[allow(clippy::too_many_arguments)]
pub async fn merge_relationship(
    pool: &PgPool,
    graph_name: &str,
    from_label: &str,
    to_label: &str,
    rel_type: &str,
    from_id: &str,
    to_id: &str,
    props: &HashMap<String, serde_json::Value>,
) -> Result<()> {
    let mut params = HashMap::new();
    params.insert("from_id".to_string(), serde_json::json!(from_id));
    params.insert("to_id".to_string(), serde_json::json!(to_id));

    for (key, value) in props {
        params.insert(key.clone(), value.clone());
    }

    let set_clauses: Vec<String> = props
        .keys()
        .map(|key| format!("r.{} = ${}", key, key))
        .collect();

    let set_clause = if set_clauses.is_empty() {
        String::new()
    } else {
        format!(" SET {}", set_clauses.join(", "))
    };

    let cypher = format!(
        "MATCH (from:{from_label} {{id: $from_id}}) \
         MATCH (to:{to_label} {{id: $to_id}}) \
         MERGE (from)-[r:{rel_type}]->(to){set_clause}",
        from_label = from_label,
        to_label = to_label,
        rel_type = rel_type,
        set_clause = set_clause
    );

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let agtype_param = build_agtype_param(&params);
    debug!(
        "merge_relationship: {} -[{}]-> {}",
        from_id, rel_type, to_id
    );

    execute_age_query(pool, &sql, &agtype_param, "merge_rel")
        .await
        .context(format!(
            "Failed to merge relationship {} -[{}]-> {}",
            from_id, rel_type, to_id
        ))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Query #6 — Create indexes
// Source: mod.rs:167-227
//
// AGE does not support Cypher DDL. We create PostgreSQL indexes on the
// AGE vertex label tables instead.
// ---------------------------------------------------------------------------

/// Create PostgreSQL indexes on AGE vertex tables for each node label.
///
/// AGE creates a table named `graph_name."LabelName"` for each label.
/// We create indexes on `properties->>'id'` (unique) and `properties->>'fqn'`.
pub async fn create_indexes(pool: &PgPool, graph_name: &str) -> Result<()> {
    info!("Creating indexes for AGE graph '{}'", graph_name);

    let labels = [
        "Crate",
        "Module",
        "Function",
        "Struct",
        "Enum",
        "Trait",
        "Impl",
        "Type",
        "TypeAlias",
        "Const",
        "Static",
        "Macro",
    ];

    for label in &labels {
        let id_idx = format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_{label}_id ON {graph}.\"{label}\" ((properties->>'id'))",
            label = label,
            graph = graph_name
        );
        match sqlx::query(&id_idx).execute(pool).await {
            Ok(_) => debug!("Created unique index on {}.id", label),
            Err(e) => warn!(
                "Failed to create unique index on {}.id (may already exist): {}",
                label, e
            ),
        }

        let fqn_idx = format!(
            "CREATE INDEX IF NOT EXISTS idx_{label}_fqn ON {graph}.\"{label}\" ((properties->>'fqn'))",
            label = label,
            graph = graph_name
        );
        match sqlx::query(&fqn_idx).execute(pool).await {
            Ok(_) => debug!("Created index on {}.fqn", label),
            Err(e) => warn!(
                "Failed to create index on {}.fqn (may already exist): {}",
                label, e
            ),
        }

        let name_idx = format!(
            "CREATE INDEX IF NOT EXISTS idx_{label}_name ON {graph}.\"{label}\" ((properties->>'name'))",
            label = label,
            graph = graph_name
        );
        match sqlx::query(&name_idx).execute(pool).await {
            Ok(_) => debug!("Created index on {}.name", label),
            Err(e) => warn!(
                "Failed to create index on {}.name (may already exist): {}",
                label, e
            ),
        }
    }

    info!("Index creation complete for AGE graph '{}'", graph_name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Query #7 — Find node by FQN
// Source: mod.rs:361-380
// ---------------------------------------------------------------------------

/// Find a node by its fully qualified name.
///
/// ```cypher
/// MATCH (n {fqn: $fqn}) RETURN n
/// ```
pub async fn find_node_by_fqn(
    pool: &PgPool,
    graph_name: &str,
    fqn: &str,
) -> Result<Option<AgeNode>> {
    let cypher = "MATCH (n {fqn: $fqn}) RETURN n";

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$, $1) AS (result agtype)",
        graph_name, cypher
    );

    let mut params = HashMap::new();
    params.insert("fqn".to_string(), serde_json::json!(fqn));

    let agtype_param = build_agtype_param(&params);

    let rows = fetch_age_query(pool, &sql, &agtype_param, "find_by_fqn")
        .await
        .context(format!("Failed to find node by FQN: {}", fqn))?;

    match rows.into_iter().next() {
        Some(row) => {
            let agtype_str: String = row.get(0);
            let node = parse_age_vertex(&agtype_str);
            Ok(node)
        }
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Query #8 — Find nodes by type
// Source: mod.rs:383-404
// ---------------------------------------------------------------------------

/// Find all nodes of a specific label/type.
///
/// ```cypher
/// MATCH (n:Label) RETURN n
/// ```
pub async fn find_nodes_by_type(
    pool: &PgPool,
    graph_name: &str,
    label: &str,
) -> Result<Vec<AgeNode>> {
    let cypher = format!("MATCH (n:{label}) RETURN n", label = label);

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ {} $$) AS (result agtype)",
        graph_name, cypher
    );

    let rows: Vec<(String,)> = sqlx::query_as(&sql)
        .fetch_all(pool)
        .await
        .context(format!("Failed to find nodes by type: {}", label))?;

    let nodes: Vec<AgeNode> = rows
        .into_iter()
        .filter_map(|(agtype_str,)| parse_age_vertex(&agtype_str))
        .collect();

    debug!("Found {} nodes of type {}", nodes.len(), label);
    Ok(nodes)
}

// ---------------------------------------------------------------------------
// Query #9 — Clear all graph data
// Source: mod.rs:262-272
// ---------------------------------------------------------------------------

/// Clear all nodes and relationships from the graph.
///
/// ```cypher
/// MATCH (n) DETACH DELETE n
/// ```
pub async fn clear_all(pool: &PgPool, graph_name: &str) -> Result<()> {
    warn!("Clearing all graph data in AGE graph '{}'", graph_name);

    let sql = format!(
        "SELECT * FROM cypher('{}', $$ MATCH (n) DETACH DELETE n $$) AS (result agtype)",
        graph_name
    );

    sqlx::query(&sql)
        .execute(pool)
        .await
        .context("Failed to clear graph data")?;

    info!("All graph data cleared in AGE graph '{}'", graph_name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Query #10 — Test connection
// Source: mod.rs:145-164
// ---------------------------------------------------------------------------

/// Test the connection to AGE by running a simple RETURN query.
///
/// ```cypher
/// RETURN 1 as test
/// ```
pub async fn test_connection(pool: &PgPool, graph_name: &str) -> Result<bool> {
    let sql = format!(
        "SELECT * FROM cypher('{}', $$ RETURN 1 as test $$) AS (result agtype)",
        graph_name
    );

    let row: Option<(String,)> = sqlx::query_as(&sql)
        .fetch_optional(pool)
        .await
        .context("Failed to test AGE connection")?;

    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_batch_nodes_query() {
        let label = "Function";
        let set_clauses: Vec<String> = NODE_PROPS
            .iter()
            .map(|p| format!("n.{} = node_data.{}", p, p))
            .collect();
        let set_clause = format!(" SET {}", set_clauses.join(", "));

        let cypher = format!(
            "UNWIND $nodes AS node_data \
             MERGE (n:{label} {{id: node_data.id}}){set_clause}",
            label = label,
            set_clause = set_clause
        );
        let sql = format!(
            "SELECT * FROM cypher('rustbrain', $$ {} $$, $1) AS (result agtype)",
            cypher
        );

        assert!(sql.contains("UNWIND $nodes"));
        assert!(sql.contains("MERGE (n:Function {id: node_data.id})"));
        assert!(sql.contains("n.fqn = node_data.fqn"));
        assert!(sql.contains("n.name = node_data.name"));
        assert!(sql.contains("cypher('rustbrain'"));
        assert!(sql.contains("$1)"));
    }

    #[test]
    fn test_build_merge_target_query() {
        let from_label = "Struct";
        let to_label = "Type";
        let rel_type = "HAS_FIELD";

        let cypher = format!(
            "UNWIND $rels AS rel_data \
             MATCH (from:{from_label} {{id: rel_data.from_id}}) \
             MERGE (to:{to_label} {{id: rel_data.to_id}}) \
             SET to.fqn = coalesce(to.fqn, rel_data.to_id), \
                 to.name = coalesce(to.name, rel_data.to_id), \
                 to.external = coalesce(to.external, true) \
             MERGE (from)-[r:{rel_type}]->(to)",
            from_label = from_label,
            to_label = to_label,
            rel_type = rel_type
        );

        assert!(cypher.contains("coalesce(to.fqn, rel_data.to_id)"));
        assert!(cypher.contains("coalesce(to.name, rel_data.to_id)"));
        assert!(cypher.contains("coalesce(to.external, true)"));

        assert!(!cypher.contains("ON CREATE SET"));
        assert!(!cypher.contains("ON MATCH SET"));

        assert!(cypher.contains("MERGE (to:Type {id: rel_data.to_id})"));
    }

    #[test]
    fn test_build_find_by_fqn_query() {
        let cypher = "MATCH (n {fqn: $fqn}) RETURN n";
        let sql = format!(
            "SELECT * FROM cypher('rustbrain', $$ {} $$, $1) AS (result agtype)",
            cypher
        );

        assert!(sql.contains("MATCH (n {fqn: $fqn})"));
        assert!(sql.contains("RETURN n"));
        assert!(sql.contains("cypher('rustbrain'"));
    }

    #[test]
    fn test_build_clear_all_query() {
        let sql = format!(
            "SELECT * FROM cypher('rustbrain', $$ MATCH (n) DETACH DELETE n $$) AS (result agtype)"
        );
        assert!(sql.contains("MATCH (n) DETACH DELETE n"));

        assert!(!sql.contains("$1"));
    }

    #[test]
    fn test_build_test_connection_query() {
        let sql =
            format!("SELECT * FROM cypher('rustbrain', $$ RETURN 1 as test $$) AS (result agtype)");
        assert!(sql.contains("RETURN 1 as test"));
        assert!(!sql.contains("$1"));
    }

    // -----------------------------------------------------------------------
    // CEO-mandated edge case integration tests for COALESCE workaround
    // All marked #[ignore] — require running AGE PostgreSQL
    // -----------------------------------------------------------------------

    async fn setup_pool() -> PgPool {
        let config = super::super::AgeConfig::default();
        super::super::create_age_pool(&config)
            .await
            .expect("Failed to create AGE pool")
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_merge_existing_node_preserves_properties() {
        let pool = setup_pool().await;
        let graph_name = "rustbrain";

        // Start clean
        clear_all(&pool, graph_name).await.unwrap();

        // Create a node with real data
        let mut real_props = HashMap::new();
        real_props.insert("fqn".to_string(), serde_json::json!("real::fqn"));
        real_props.insert("name".to_string(), serde_json::json!("real_name"));
        merge_node(&pool, graph_name, "Type", "type_1", &real_props)
            .await
            .unwrap();

        // Now use batch_insert_rels_merge_target targeting the same node
        let rels = vec![serde_json::json!({
            "from_id": "struct_1",
            "to_id": "type_1",
            "props": {}
        })];

        // First create the from node so MATCH succeeds
        let mut struct_props = HashMap::new();
        struct_props.insert("fqn".to_string(), serde_json::json!("struct_1"));
        struct_props.insert("name".to_string(), serde_json::json!("Struct1"));
        merge_node(&pool, graph_name, "Struct", "struct_1", &struct_props)
            .await
            .unwrap();

        batch_insert_rels_merge_target(&pool, graph_name, "Struct", "Type", "HAS_FIELD", &rels)
            .await
            .unwrap();

        // Verify: existing node's fqn and name were NOT overwritten
        let node = find_node_by_fqn(&pool, graph_name, "real::fqn")
            .await
            .expect("query failed")
            .expect("node not found");

        assert_eq!(
            node.properties.get("fqn").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("real::fqn"),
            "Existing fqn should NOT be overwritten by COALESCE default"
        );
        assert_eq!(
            node.properties.get("name").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("real_name"),
            "Existing name should NOT be overwritten by COALESCE default"
        );
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_merge_new_node_sets_defaults() {
        let pool = setup_pool().await;
        let graph_name = "rustbrain";

        clear_all(&pool, graph_name).await.unwrap();

        // Create from node
        let mut struct_props = HashMap::new();
        struct_props.insert("fqn".to_string(), serde_json::json!("struct_1"));
        struct_props.insert("name".to_string(), serde_json::json!("Struct1"));
        merge_node(&pool, graph_name, "Struct", "struct_1", &struct_props)
            .await
            .unwrap();

        // Use batch_insert_rels_merge_target where target does NOT exist
        let rels = vec![serde_json::json!({
            "from_id": "struct_1",
            "to_id": "new_type_id",
            "props": {}
        })];

        batch_insert_rels_merge_target(&pool, graph_name, "Struct", "Type", "HAS_FIELD", &rels)
            .await
            .unwrap();

        // Verify: new node gets COALESCE defaults: fqn=to_id, name=to_id, external=true
        let node = find_node_by_fqn(&pool, graph_name, "new_type_id")
            .await
            .expect("query failed")
            .expect("newly created node not found");

        assert_eq!(
            node.properties.get("fqn").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("new_type_id"),
            "New node should get fqn=to_id from COALESCE default"
        );
        assert_eq!(
            node.properties.get("name").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("new_type_id"),
            "New node should get name=to_id from COALESCE default"
        );
        assert_eq!(
            node.properties.get("external").and_then(|v| match v {
                super::super::types::AgeValue::Bool(b) => Some(*b),
                _ => None,
            }),
            Some(true),
            "New node should get external=true from COALESCE default"
        );
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_merge_mixed_existing_new_nodes() {
        let pool = setup_pool().await;
        let graph_name = "rustbrain";

        clear_all(&pool, graph_name).await.unwrap();

        // Create from node
        let mut struct_props = HashMap::new();
        struct_props.insert("fqn".to_string(), serde_json::json!("struct_1"));
        struct_props.insert("name".to_string(), serde_json::json!("Struct1"));
        merge_node(&pool, graph_name, "Struct", "struct_1", &struct_props)
            .await
            .unwrap();

        // Create one existing target node with real data
        let mut existing_type_props = HashMap::new();
        existing_type_props.insert("fqn".to_string(), serde_json::json!("existing::type"));
        existing_type_props.insert("name".to_string(), serde_json::json!("ExistingType"));
        merge_node(
            &pool,
            graph_name,
            "Type",
            "existing_type",
            &existing_type_props,
        )
        .await
        .unwrap();

        // Batch with 2 rels: one targeting existing node, one targeting new node
        let rels = vec![
            serde_json::json!({
                "from_id": "struct_1",
                "to_id": "existing_type",
                "props": {}
            }),
            serde_json::json!({
                "from_id": "struct_1",
                "to_id": "brand_new_type",
                "props": {}
            }),
        ];

        batch_insert_rels_merge_target(&pool, graph_name, "Struct", "Type", "HAS_FIELD", &rels)
            .await
            .unwrap();

        // Verify: existing node properties preserved
        let existing = find_node_by_fqn(&pool, graph_name, "existing::type")
            .await
            .expect("query failed")
            .expect("existing node not found");
        assert_eq!(
            existing.properties.get("name").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("ExistingType"),
            "Existing node name should be preserved"
        );

        // Verify: new node gets defaults
        let new_node = find_node_by_fqn(&pool, graph_name, "brand_new_type")
            .await
            .expect("query failed")
            .expect("new node not found");
        assert_eq!(
            new_node.properties.get("external").and_then(|v| match v {
                super::super::types::AgeValue::Bool(b) => Some(*b),
                _ => None,
            }),
            Some(true),
            "New node should get external=true"
        );
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_merge_stub_node_preserves_on_remerge() {
        let pool = setup_pool().await;
        let graph_name = "rustbrain";

        clear_all(&pool, graph_name).await.unwrap();

        // Create from node
        let mut struct_props = HashMap::new();
        struct_props.insert("fqn".to_string(), serde_json::json!("struct_1"));
        struct_props.insert("name".to_string(), serde_json::json!("Struct1"));
        merge_node(&pool, graph_name, "Struct", "struct_1", &struct_props)
            .await
            .unwrap();

        // Step 1: Create stub node via batch_insert_rels_merge_target
        let rels_step1 = vec![serde_json::json!({
            "from_id": "struct_1",
            "to_id": "stub_type_id",
            "props": {}
        })];

        batch_insert_rels_merge_target(
            &pool,
            graph_name,
            "Struct",
            "Type",
            "HAS_FIELD",
            &rels_step1,
        )
        .await
        .unwrap();

        // Verify stub was created with defaults
        let stub = find_node_by_fqn(&pool, graph_name, "stub_type_id")
            .await
            .expect("query failed")
            .expect("stub node not found");
        assert_eq!(
            stub.properties.get("external").and_then(|v| match v {
                super::super::types::AgeValue::Bool(b) => Some(*b),
                _ => None,
            }),
            Some(true),
            "Stub should have external=true"
        );

        // Step 2: Re-merge with same target — COALESCE should preserve existing properties
        let rels_step2 = vec![serde_json::json!({
            "from_id": "struct_1",
            "to_id": "stub_type_id",
            "props": {"field_name": "my_field"}
        })];

        batch_insert_rels_merge_target(
            &pool,
            graph_name,
            "Struct",
            "Type",
            "HAS_FIELD",
            &rels_step2,
        )
        .await
        .unwrap();

        // Verify: stub node's fqn and name remain the stub values (not overwritten)
        let remerged = find_node_by_fqn(&pool, graph_name, "stub_type_id")
            .await
            .expect("query failed")
            .expect("re-merged node not found");

        assert_eq!(
            remerged.properties.get("fqn").and_then(|v| match v {
                super::super::types::AgeValue::String(s) => Some(s.as_str()),
                _ => None,
            }),
            Some("stub_type_id"),
            "Re-merged node fqn should be preserved from stub creation"
        );
        assert_eq!(
            remerged.properties.get("external").and_then(|v| match v {
                super::super::types::AgeValue::Bool(b) => Some(*b),
                _ => None,
            }),
            Some(true),
            "Re-merged node external should remain true"
        );
    }
}
