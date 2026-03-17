//! Code item CRUD handlers.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::errors::AppError;
use crate::neo4j::{get_callers_from_neo4j, get_callees_from_neo4j};
use crate::state::AppState;
use super::{CallerInfo, CalleeInfo, CallerNode, default_depth};

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct GetFunctionQuery {
    pub fqn: String,
}

#[derive(Debug, Serialize)]
pub struct FunctionDetail {
    pub fqn: String,
    pub name: String,
    pub kind: String,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub module_path: Option<String>,
    pub crate_name: Option<String>,
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
}

#[derive(Debug, Deserialize)]
pub struct GetCallersQuery {
    pub fqn: String,
    #[serde(default = "default_depth")]
    pub depth: usize,
}

#[derive(Debug, Serialize)]
pub struct CallersResponse {
    pub fqn: String,
    pub callers: Vec<CallerNode>,
    pub depth: usize,
}

// =============================================================================
// Handlers
// =============================================================================

pub async fn get_function(
    State(state): State<AppState>,
    Query(query): Query<GetFunctionQuery>,
) -> Result<Json<FunctionDetail>, AppError> {
    state.metrics.record_request("get_function", "GET");
    debug!("Get function: {}", query.fqn);

    let row = sqlx::query_as::<_, (String, String, String, String, Option<String>, Option<String>, Option<String>, i32, i32, Option<String>, Option<String>)>(
        r#"
        SELECT e.fqn, e.name, e.item_type, e.visibility, e.signature, e.doc_comment as docstring,
               sf.file_path, e.start_line, e.end_line, sf.module_path, sf.crate_name
        FROM extracted_items e
        LEFT JOIN source_files sf ON e.source_file_id = sf.id
        WHERE e.fqn = $1
        "#
    )
    .bind(&query.fqn)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query function: {}", e)))?;

    let (fqn, name, item_type, visibility, signature, docstring, file_path, start_line, end_line, module_path, crate_name) =
        row.ok_or_else(|| AppError::NotFound(format!("Item not found: {}", query.fqn)))?;

    // Get callers from Neo4j and convert to CallerInfo
    let caller_nodes = get_callers_from_neo4j(&state, &query.fqn, 1).await?;
    let callers: Vec<CallerInfo> = caller_nodes
        .into_iter()
        .map(|n| CallerInfo {
            fqn: n.fqn,
            name: n.name,
            file_path: n.file_path,
            line: n.line,
        })
        .collect();

    // Get callees from Neo4j
    let callees = get_callees_from_neo4j(&state, &query.fqn).await?;

    Ok(Json(FunctionDetail {
        fqn,
        name,
        kind: item_type,
        visibility: Some(visibility),
        signature,
        docstring,
        file_path: file_path.unwrap_or_default(),
        start_line: start_line as u32,
        end_line: end_line as u32,
        module_path,
        crate_name,
        callers,
        callees,
    }))
}

pub async fn get_callers(
    State(state): State<AppState>,
    Query(query): Query<GetCallersQuery>,
) -> Result<Json<CallersResponse>, AppError> {
    state.metrics.record_request("get_callers", "GET");

    // Validate depth parameter: max 10
    if query.depth > 10 {
        return Err(AppError::BadRequest(
            "depth parameter must be <= 10".to_string(),
        ));
    }

    debug!("Get callers for: {} (depth: {})", query.fqn, query.depth);

    let callers = get_callers_from_neo4j(&state, &query.fqn, query.depth).await?;

    Ok(Json(CallersResponse {
        fqn: query.fqn,
        callers,
        depth: query.depth,
    }))
}
