//! Read-only SQL query handler.
//!
//! Provides `POST /tools/pg_query` for executing parameterized read-only queries
//! against whitelisted tables with security validation.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use sqlx::{Column, Row, TypeInfo};
use tracing::debug;

use crate::errors::AppError;
use crate::state::AppState;

/// Whitelisted tables that may be queried.
const ALLOWED_TABLES: &[&str] = &[
    "extracted_items",
    "source_files",
    "artifacts",
    "tasks",
    "call_sites",
    "trait_implementations",
    "ingestion_runs",
    "audit_events",
    "repositories",
];

/// Mutating SQL keywords that must be rejected (checked as whole tokens).
const MUTATING_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "DROP", "ALTER", "CREATE", "TRUNCATE", "GRANT", "REVOKE", "COPY",
    "EXECUTE", "SET", "LOCK",
    "DISCARD",
    // NOTE: "COMMENT", "SECURITY", "OWNER" removed — they false-positive on
    // column names like "doc_comment", "security_level". DDL commands using
    // these words (COMMENT ON, SECURITY DEFINER, ALTER OWNER) are already
    // blocked by ALTER/CREATE.
];

/// Forbidden schema prefixes (system tables).
const FORBIDDEN_SCHEMAS: &[&str] = &["PG_CATALOG", "INFORMATION_SCHEMA"];

/// Maximum number of rows returned.
const MAX_ROWS: usize = 500;

/// Request body for `POST /tools/pg_query`.
#[derive(Debug, Deserialize)]
pub struct PgQueryRequest {
    pub query: String,
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
}

/// Response body for `POST /tools/pg_query`.
#[derive(Debug, Serialize)]
pub struct PgQueryResponse {
    pub rows: Vec<serde_json::Value>,
    pub row_count: usize,
}

/// Tokenize a SQL string by splitting on whitespace and punctuation.
/// This ensures that column names like "updated_at" produce tokens
/// ["UPDATED", "AT"] rather than matching "UPDATE" as a substring.
fn tokenize_sql(upper: &str) -> Vec<&str> {
    upper
        .split(|c: char| c.is_whitespace() || c.is_ascii_punctuation())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Validates the query is read-only and only references whitelisted tables.
fn validate_query(query: &str) -> Result<(), AppError> {
    let upper = query.to_uppercase();
    let tokens = tokenize_sql(&upper);

    // Reject mutating keywords (exact token match — avoids false positives
    // on column names like "updated_at" which tokenizes to ["UPDATED", "AT"])
    for keyword in MUTATING_KEYWORDS {
        if tokens.iter().any(|t| t == keyword) {
            return Err(AppError::BadRequest(
                "Mutating SQL operations are not allowed. Use read-only SELECT queries."
                    .to_string(),
            ));
        }
    }

    // Reject references to system schemas
    for schema in FORBIDDEN_SCHEMAS {
        if upper.contains(schema) {
            return Err(AppError::BadRequest(format!(
                "Access to system schema '{}' is not allowed",
                schema.to_lowercase()
            )));
        }
    }

    // Check that referenced tables (after FROM/JOIN) are whitelisted
    let upper_words: Vec<&str> = upper.split_whitespace().collect();
    for (i, word) in upper_words.iter().enumerate() {
        if (*word == "FROM" || word.ends_with("JOIN")) && i + 1 < upper_words.len() {
            let table_candidate =
                upper_words[i + 1].trim_matches(|c: char| c == '(' || c == ')' || c == ',');
            // Strip optional schema prefix (e.g., "public.extracted_items")
            let table_name = table_candidate
                .rsplit('.')
                .next()
                .unwrap_or(table_candidate);
            if !table_name.is_empty()
                && !table_name.starts_with('(')
                && !table_name.starts_with('$')
                && !ALLOWED_TABLES
                    .iter()
                    .any(|t| t.to_uppercase() == table_name)
            {
                return Err(AppError::BadRequest(format!(
                    "Table '{}' is not in the allowed list",
                    table_name.to_lowercase()
                )));
            }
        }
    }

    Ok(())
}

/// Execute a read-only parameterized SQL query.
///
/// Defense-in-depth: applies two enforcement layers:
/// 1. Keyword-level validation ([`validate_query`]) rejects DML before touching the DB.
/// 2. Database-level `SET LOCAL transaction_read_only = 'on'` inside an explicit
///    transaction ensures Postgres itself refuses any write that slips through.
///    The transaction is always rolled back — even for SELECTs — so nothing persists.
pub async fn pg_query(
    State(state): State<AppState>,
    Json(request): Json<PgQueryRequest>,
) -> Result<Json<PgQueryResponse>, AppError> {
    state.metrics.record_request("pg_query", "POST");
    debug!("pg_query: {}", request.query);

    validate_query(&request.query)?;

    // Append LIMIT if not present in query
    let trimmed = request.query.trim().trim_end_matches(';');
    let final_query = if !request.query.to_uppercase().contains("LIMIT") {
        format!("{} LIMIT {}", trimmed, MAX_ROWS)
    } else {
        trimmed.to_string()
    };

    // Open an explicit transaction and mark it read-only at the DB level.
    // This is defense-in-depth: even if keyword validation is bypassed,
    // Postgres will refuse any write operation.
    let mut tx = state
        .pg_pool
        .begin()
        .await
        .map_err(|e| AppError::Database(format!("Failed to begin transaction: {}", e)))?;

    sqlx::query("SET LOCAL transaction_read_only = 'on'")
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::Database(format!("Failed to set read-only mode: {}", e)))?;

    // Build the query with dynamic binds
    let mut query = sqlx::query(&final_query);
    for param in &request.params {
        query = match param {
            serde_json::Value::String(s) => query.bind(s.as_str()),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    query.bind(i)
                } else if let Some(f) = n.as_f64() {
                    query.bind(f)
                } else {
                    query.bind(n.to_string())
                }
            }
            serde_json::Value::Bool(b) => query.bind(*b),
            serde_json::Value::Null => query.bind(Option::<String>::None),
            other => query.bind(other.to_string()),
        };
    }

    // Execute with 10-second timeout
    let rows = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        query.fetch_all(&mut *tx),
    )
    .await
    .map_err(|_| AppError::BadRequest("Query timed out after 10 seconds".to_string()))?
    .map_err(|e| AppError::Database(format!("Query failed: {}", e)))?;

    // Always rollback — we never want to persist anything from a "read-only" query path.
    let _ = tx.rollback().await;

    // Convert rows to JSON values (cap at MAX_ROWS as safety net)
    let json_rows: Vec<serde_json::Value> = rows
        .iter()
        .take(MAX_ROWS)
        .map(|row| {
            let columns = row.columns();
            let mut map = serde_json::Map::new();
            for col in columns {
                let name = col.name().to_string();
                let type_name = col.type_info().name();
                let value: serde_json::Value = match type_name {
                    "TEXT" | "VARCHAR" | "CHAR" | "NAME" | "BPCHAR" | "UNKNOWN" => row
                        .try_get::<Option<String>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                    "INT4" | "INT2" => row
                        .try_get::<Option<i32>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(|v| serde_json::Value::Number(v.into()))
                        .unwrap_or(serde_json::Value::Null),
                    "INT8" => row
                        .try_get::<Option<i64>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(|v| serde_json::Value::Number(v.into()))
                        .unwrap_or(serde_json::Value::Null),
                    "FLOAT4" | "FLOAT8" => row
                        .try_get::<Option<f64>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .and_then(serde_json::Number::from_f64)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null),
                    "BOOL" => row
                        .try_get::<Option<bool>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(serde_json::Value::Bool)
                        .unwrap_or(serde_json::Value::Null),
                    "JSON" | "JSONB" => row
                        .try_get::<Option<serde_json::Value>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .unwrap_or(serde_json::Value::Null),
                    "UUID" => row
                        .try_get::<Option<uuid::Uuid>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(|v| serde_json::Value::String(v.to_string()))
                        .unwrap_or(serde_json::Value::Null),
                    "TIMESTAMPTZ" | "TIMESTAMP" => row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(|v| serde_json::Value::String(v.to_rfc3339()))
                        .unwrap_or(serde_json::Value::Null),
                    _ => row
                        .try_get::<Option<String>, _>(col.ordinal())
                        .ok()
                        .flatten()
                        .map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null),
                };
                map.insert(name, value);
            }
            serde_json::Value::Object(map)
        })
        .collect();

    let row_count = json_rows.len();
    Ok(Json(PgQueryResponse {
        rows: json_rows,
        row_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_insert_query() {
        let result = validate_query("INSERT INTO extracted_items (name) VALUES ($1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_delete_query() {
        let result = validate_query("DELETE FROM extracted_items WHERE id = $1");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_drop_query() {
        let result = validate_query("DROP TABLE extracted_items");
        assert!(result.is_err());
    }

    #[test]
    fn test_allows_select_query() {
        assert!(validate_query("SELECT * FROM extracted_items WHERE name = $1").is_ok());
    }

    #[test]
    fn test_allows_select_with_join() {
        let query = "SELECT e.*, sf.file_path FROM extracted_items e JOIN source_files sf ON e.source_file_id = sf.id";
        assert!(validate_query(query).is_ok());
    }

    #[test]
    fn test_does_not_false_positive_on_updated_at() {
        // "updated_at" contains "UPDATE" as a substring but tokenizes to
        // ["UPDATED", "AT"] — neither matches "UPDATE" exactly.
        let query = "SELECT id, updated_at FROM tasks WHERE status = $1";
        assert!(validate_query(query).is_ok());
    }

    #[test]
    fn test_rejects_case_insensitive() {
        let result = validate_query("insert into extracted_items (name) values ($1)");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_non_whitelisted_table() {
        let result = validate_query("SELECT * FROM users");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("users"));
    }

    #[test]
    fn test_allows_all_whitelisted_tables() {
        for table in ALLOWED_TABLES {
            let query = format!("SELECT * FROM {}", table);
            assert!(
                validate_query(&query).is_ok(),
                "Should allow table: {}",
                table
            );
        }
    }

    #[test]
    fn test_rejects_system_tables() {
        let result = validate_query("SELECT * FROM pg_catalog.pg_tables");
        assert!(result.is_err());
    }

    #[test]
    fn test_rejects_information_schema() {
        let result = validate_query("SELECT * FROM information_schema.tables");
        assert!(result.is_err());
    }

    #[test]
    fn test_pg_query_request_deserialization() {
        let json = r#"{"query": "SELECT * FROM tasks WHERE status = $1", "params": ["pending"]}"#;
        let request: PgQueryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.query, "SELECT * FROM tasks WHERE status = $1");
        assert_eq!(request.params.len(), 1);
    }

    #[test]
    fn test_pg_query_request_no_params() {
        let json = r#"{"query": "SELECT count(*) FROM tasks"}"#;
        let request: PgQueryRequest = serde_json::from_str(json).unwrap();
        assert!(request.params.is_empty());
    }

    #[test]
    fn test_tokenize_sql_splits_on_underscores() {
        let tokens = tokenize_sql("SELECT UPDATED_AT FROM TASKS");
        assert!(tokens.contains(&"UPDATED"));
        assert!(tokens.contains(&"AT"));
        assert!(!tokens.contains(&"UPDATE"));
    }
}
