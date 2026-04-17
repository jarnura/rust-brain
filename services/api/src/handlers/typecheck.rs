//! Type check query handlers for call_sites and trait_implementations.
//!
//! These handlers query PostgreSQL directly for type resolution data.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use tracing::debug;

use super::default_limit;
use crate::errors::AppError;
use crate::extractors::OptionalWorkspaceId;
use crate::state::AppState;
use crate::workspace::acquire_conn;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Query parameters for `GET /tools/find_calls_with_type`.
#[derive(Debug, Deserialize)]
pub struct FindCallsWithTypeQuery {
    /// Name of the type to search for in `concrete_type_args`
    pub type_name: String,
    /// Optional callee name filter (e.g., `"parse"` to find `parse::<T>()`)
    pub callee_name: Option<String>,
    /// Maximum results (default: 10, capped to 100)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Response for `GET /tools/find_calls_with_type`.
#[derive(Debug, Serialize)]
pub struct CallsWithTypeResponse {
    /// Echo of the queried type name
    pub type_name: String,
    /// Matching call sites
    pub calls: Vec<CallSiteInfo>,
}

/// A call site where a concrete type argument was resolved.
#[derive(Debug, Serialize)]
pub struct CallSiteInfo {
    /// FQN of the calling function
    pub caller_fqn: String,
    /// FQN of the called (generic) function
    pub callee_fqn: String,
    /// Source file containing the call
    pub file_path: String,
    /// Line number of the call
    pub line_number: u32,
    /// Resolved concrete type arguments (e.g., `["String"]` for `parse::<String>()`)
    pub concrete_type_args: Vec<String>,
    /// Whether the call is fully monomorphized
    pub is_monomorphized: bool,
    /// Resolution quality (`"analyzed"` or `"heuristic"`)
    pub quality: String,
}

/// Query parameters for `GET /tools/find_trait_impls_for_type`.
#[derive(Debug, Deserialize)]
pub struct FindTraitImplsForTypeQuery {
    /// Name of the type to search for in `self_type`
    pub type_name: String,
    /// Maximum results (default: 10, capped to 100)
    #[serde(default = "default_limit")]
    pub limit: usize,
}

/// Response for `GET /tools/find_trait_impls_for_type`.
#[derive(Debug, Serialize)]
pub struct TraitImplsForTypeResponse {
    /// Echo of the queried type name
    pub type_name: String,
    /// Trait implementations for this type
    pub implementations: Vec<TraitImplInfo>,
}

/// A discovered trait implementation.
#[derive(Debug, Serialize)]
pub struct TraitImplInfo {
    /// FQN of the trait being implemented
    pub trait_fqn: String,
    /// The implementing type name
    pub self_type: String,
    /// FQN of the impl block
    pub impl_fqn: String,
    /// Source file containing the impl
    pub file_path: String,
    /// Line number of the impl block
    pub line_number: u32,
    /// Generic parameters on the impl
    pub generic_params: Vec<String>,
    /// Resolution quality (`"analyzed"` or `"heuristic"`)
    pub quality: String,
}

// =============================================================================
// Handlers
// =============================================================================

/// Find call sites where concrete_type_args contains a specific type.
///
/// This enables queries like "show me all calls to parse for String".
pub async fn find_calls_with_type(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Query(query): Query<FindCallsWithTypeQuery>,
) -> Result<Json<CallsWithTypeResponse>, AppError> {
    state.metrics.record_request("find_calls_with_type", "GET");
    debug!(
        "Find calls with type: {} (callee: {:?})",
        query.type_name, query.callee_name
    );

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let type_pattern = format!("%{}%", query.type_name);
    let limit = query.limit.min(100) as i32;

    let rows: Vec<sqlx::postgres::PgRow> = if let Some(callee_name) = &query.callee_name {
        let callee_pattern = format!("%::{}", callee_name);
        sqlx::query(
            r#"
            SELECT 
                caller_fqn,
                callee_fqn,
                file_path,
                line_number,
                concrete_type_args,
                is_monomorphized,
                quality
            FROM call_sites
            WHERE callee_fqn LIKE $1
              AND concrete_type_args::text LIKE $2
            ORDER BY file_path, line_number
            LIMIT $3
            "#,
        )
        .bind(&callee_pattern)
        .bind(&type_pattern)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| AppError::Database(format!("Failed to query call_sites: {}", e)))?
    } else {
        sqlx::query(
            r#"
            SELECT 
                caller_fqn,
                callee_fqn,
                file_path,
                line_number,
                concrete_type_args,
                is_monomorphized,
                quality
            FROM call_sites
            WHERE concrete_type_args::text LIKE $1
            ORDER BY file_path, line_number
            LIMIT $2
            "#,
        )
        .bind(&type_pattern)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| AppError::Database(format!("Failed to query call_sites: {}", e)))?
    };

    let calls = rows
        .into_iter()
        .map(|row| {
            let concrete_type_args_json: Option<serde_json::Value> = row.get("concrete_type_args");
            let type_args: Vec<String> = concrete_type_args_json
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();

            CallSiteInfo {
                caller_fqn: row.get("caller_fqn"),
                callee_fqn: row.get("callee_fqn"),
                file_path: row.get("file_path"),
                line_number: row.get::<i32, _>("line_number") as u32,
                concrete_type_args: type_args,
                is_monomorphized: row.get("is_monomorphized"),
                quality: row
                    .get::<Option<String>, _>("quality")
                    .unwrap_or_else(|| "unknown".to_string()),
            }
        })
        .collect();

    Ok(Json(CallsWithTypeResponse {
        type_name: query.type_name,
        calls,
    }))
}

/// Find trait implementations for a specific self_type.
///
/// This enables queries like "show me all traits implemented by String".
pub async fn find_trait_impls_for_type(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Query(query): Query<FindTraitImplsForTypeQuery>,
) -> Result<Json<TraitImplsForTypeResponse>, AppError> {
    state
        .metrics
        .record_request("find_trait_impls_for_type", "GET");
    debug!("Find trait impls for type: {}", query.type_name);

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let type_pattern = format!("%{}%", query.type_name);
    let limit = query.limit.min(100) as i32;

    let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
        r#"
        SELECT 
            trait_fqn,
            self_type,
            impl_fqn,
            file_path,
            line_number,
            generic_params,
            quality
        FROM trait_implementations
        WHERE self_type LIKE $1
        ORDER BY trait_fqn
        LIMIT $2
        "#,
    )
    .bind(&type_pattern)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query trait_implementations: {}", e)))?;

    let implementations = rows
        .into_iter()
        .map(|row| {
            let generic_params_json: Option<serde_json::Value> = row.get("generic_params");
            let generic_params: Vec<String> = generic_params_json
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();

            TraitImplInfo {
                trait_fqn: row.get("trait_fqn"),
                self_type: row.get("self_type"),
                impl_fqn: row.get("impl_fqn"),
                file_path: row.get("file_path"),
                line_number: row.get::<i32, _>("line_number") as u32,
                generic_params,
                quality: row
                    .get::<Option<String>, _>("quality")
                    .unwrap_or_else(|| "unknown".to_string()),
            }
        })
        .collect();

    Ok(Json(TraitImplsForTypeResponse {
        type_name: query.type_name,
        implementations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_calls_with_type_query_deserialization() {
        let json = serde_json::json!({
            "type_name": "String",
            "callee_name": "parse",
            "limit": 50
        });

        let query: FindCallsWithTypeQuery = serde_json::from_value(json).unwrap();
        assert_eq!(query.type_name, "String");
        assert_eq!(query.callee_name, Some("parse".to_string()));
        assert_eq!(query.limit, 50);
    }

    #[test]
    fn test_find_calls_with_type_query_defaults() {
        let json = serde_json::json!({
            "type_name": "Vec"
        });

        let query: FindCallsWithTypeQuery = serde_json::from_value(json).unwrap();
        assert_eq!(query.type_name, "Vec");
        assert_eq!(query.callee_name, None);
        assert_eq!(query.limit, 10);
    }

    #[test]
    fn test_find_trait_impls_for_type_query_deserialization() {
        let json = serde_json::json!({
            "type_name": "MyStruct",
            "limit": 30
        });

        let query: FindTraitImplsForTypeQuery = serde_json::from_value(json).unwrap();
        assert_eq!(query.type_name, "MyStruct");
        assert_eq!(query.limit, 30);
    }

    #[test]
    fn test_call_site_info_serialization() {
        let info = CallSiteInfo {
            caller_fqn: "crate::module::func".to_string(),
            callee_fqn: "core::str::parse".to_string(),
            file_path: "src/module.rs".to_string(),
            line_number: 42,
            concrete_type_args: vec!["String".to_string()],
            is_monomorphized: true,
            quality: "analyzed".to_string(),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("caller_fqn"));
        assert!(json.contains("parse"));
    }

    #[test]
    fn test_trait_impl_info_serialization() {
        let info = TraitImplInfo {
            trait_fqn: "std::clone::Clone".to_string(),
            self_type: "MyStruct".to_string(),
            impl_fqn: "crate::MyStruct".to_string(),
            file_path: "src/lib.rs".to_string(),
            line_number: 10,
            generic_params: vec![],
            quality: "analyzed".to_string(),
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("Clone"));
        assert!(json.contains("MyStruct"));
    }
}
