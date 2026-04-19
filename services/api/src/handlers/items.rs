//! Code item lookup and call-graph traversal handlers.
//!
//! Provides `GET /tools/get_function` and `GET /tools/get_callers` endpoints
//! for retrieving detailed item metadata from Postgres and call-graph context
//! from Neo4j.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{default_depth, CalleeInfo, CallerInfo, CallerNode};
use crate::errors::AppError;
use crate::extractors::WorkspaceId;
use crate::neo4j::WorkspaceGraphClient;
use crate::state::AppState;
use crate::workspace::acquire_conn;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Query parameters for `GET /tools/get_function`.
#[derive(Debug, Deserialize)]
pub struct GetFunctionQuery {
    /// Fully qualified name of the item to retrieve
    pub fqn: String,
}

/// Detailed information about a code item, including source and call graph.
#[derive(Debug, Serialize)]
pub struct FunctionDetail {
    /// Fully qualified name
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Item kind (`"function"`, `"struct"`, `"impl"`, etc.)
    pub kind: String,
    /// Visibility (`"pub"`, `"pub(crate)"`, etc.)
    pub visibility: Option<String>,
    /// Full type signature
    pub signature: Option<String>,
    /// Doc comment extracted from source
    pub docstring: Option<String>,
    /// Source file path relative to crate root
    pub file_path: String,
    /// Start line in the source file (1-indexed)
    pub start_line: u32,
    /// End line in the source file (1-indexed)
    pub end_line: u32,
    /// Module path (e.g., `"crate::module::submodule"`)
    pub module_path: Option<String>,
    /// Crate name this item belongs to
    pub crate_name: Option<String>,
    /// Full source body (may be truncated for large items)
    pub body_source: Option<String>,
    /// Functions that call this item
    pub callers: Vec<CallerInfo>,
    /// Functions called by this item
    pub callees: Vec<CalleeInfo>,
}

/// Query parameters for `GET /tools/get_callers`.
#[derive(Debug, Deserialize)]
pub struct GetCallersQuery {
    /// Fully qualified name of the target function
    pub fqn: String,
    /// Maximum traversal depth (default: 1, max: 10)
    #[serde(default = "default_depth")]
    pub depth: usize,
}

/// Response for `GET /tools/get_callers`.
#[derive(Debug, Serialize)]
pub struct CallersResponse {
    /// The queried FQN
    pub fqn: String,
    /// Functions that call the target (up to `depth` hops)
    pub callers: Vec<CallerNode>,
    /// The depth that was actually used
    pub depth: usize,
}

// =============================================================================
// Handlers
// =============================================================================

/// Retrieves detailed information about a code item by its fully qualified name.
///
/// Looks up the item in Postgres for metadata and source, then queries Neo4j
/// for callers and callees. For `impl` blocks, callers and callees are
/// aggregated from all child methods.
///
/// # Errors
///
/// Returns [`AppError::Database`] if the Postgres query fails.
/// Returns [`AppError::NotFound`] if no item matches the given FQN.
///
/// # Notes
///
/// As of commit `4d573c9`, this handler aggregates callers/callees from child
/// methods when the queried item is an `impl` block, rather than returning
/// an empty caller list.
pub async fn get_function(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Query(query): Query<GetFunctionQuery>,
) -> Result<Json<FunctionDetail>, AppError> {
    debug!("Get function: {}", query.fqn);

    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws.clone());

    let mut conn = acquire_conn(&state.pg_pool, Some(&ws)).await?;

    let row = sqlx::query_as::<
        sqlx::Postgres,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            i32,
            i32,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
        SELECT e.fqn, e.name, e.item_type, e.visibility, e.signature, e.doc_comment as docstring,
               sf.file_path, e.start_line, e.end_line, sf.module_path, sf.crate_name, e.body_source
        FROM extracted_items e
        LEFT JOIN source_files sf ON e.source_file_id = sf.id
        WHERE e.fqn = $1
        "#,
    )
    .bind(&query.fqn)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query function: {}", e)))?;

    let (
        fqn,
        name,
        item_type,
        visibility,
        signature,
        docstring,
        file_path,
        start_line,
        end_line,
        module_path,
        crate_name,
        body_source,
    ) = row.ok_or_else(|| AppError::NotFound(format!("Item not found: {}", query.fqn)))?;

    // For impl blocks, aggregate callers/callees from child methods.
    // Impl nodes don't have CALLS relationships directly — their methods do.
    let (callers, callees) = if item_type == "impl" {
        let self_type = extract_self_type_from_impl(&name, &signature);

        // Build the method FQN prefix.
        // impl FQN: "module::path::ImplName" where ImplName could be "Type" or "Trait_Type"
        //   or contain "super::" segments like "super::Trait_Type"
        // Method FQNs: "module::path::SelfType::method"
        //
        // Strategy: strip the impl name from the end of the FQN to get the parent module,
        // then append self_type.
        let method_prefix = if fqn.ends_with(&format!("::{}", name)) {
            let module = &fqn[..fqn.len() - name.len() - 2]; // strip "::name"
            format!("{}::{}::", module, self_type)
        } else if let Some(last_sep) = fqn.rfind("::") {
            let module = &fqn[..last_sep];
            format!("{}::{}::", module, self_type)
        } else {
            format!("{}::", self_type)
        };

        debug!(
            "Impl block detected: fqn={}, self_type={}, method_prefix={}",
            fqn, self_type, method_prefix
        );

        let (caller_result, callee_result) = tokio::join!(
            client.get_callers_for_impl(&method_prefix, 1),
            client.get_callees_for_impl(&method_prefix),
        );

        let callers: Vec<CallerInfo> = caller_result
            .unwrap_or_default()
            .into_iter()
            .map(|n| CallerInfo {
                fqn: n.fqn,
                name: n.name,
                file_path: n.file_path,
                line: n.line,
            })
            .collect();
        (callers, callee_result.unwrap_or_default())
    } else {
        // Standard function/method: direct CALLS query
        let (caller_result, callee_result) = tokio::join!(
            client.get_callers(&query.fqn, 1),
            client.get_callees(&query.fqn),
        );

        let callers: Vec<CallerInfo> = caller_result
            .unwrap_or_default()
            .into_iter()
            .map(|n| CallerInfo {
                fqn: n.fqn,
                name: n.name,
                file_path: n.file_path,
                line: n.line,
            })
            .collect();
        (callers, callee_result.unwrap_or_default())
    };

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
        body_source,
        callers,
        callees,
    }))
}

/// Finds all functions that call the target function, up to `depth` hops.
///
/// Traverses `CALLS` relationships in Neo4j. The `depth` parameter is
/// clamped to a maximum of 10 on the API side (and further to 5 inside
/// the Neo4j query layer).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if `depth > 10`.
/// Returns [`AppError::Neo4j`] if the graph query fails.
pub async fn get_callers(
    State(state): State<AppState>,
    WorkspaceId(ws): WorkspaceId,
    Query(query): Query<GetCallersQuery>,
) -> Result<Json<CallersResponse>, AppError> {
    // Validate depth parameter: max 10
    if query.depth > 10 {
        return Err(AppError::BadRequest(
            "depth parameter must be <= 10".to_string(),
        ));
    }

    debug!("Get callers for: {} (depth: {})", query.fqn, query.depth);

    let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);
    let callers = client.get_callers(&query.fqn, query.depth).await?;

    Ok(Json(CallersResponse {
        fqn: query.fqn,
        callers,
        depth: query.depth,
    }))
}

/// Extract the self_type from an impl block's name and signature.
///
/// For inherent impls (name="Type"), returns the name directly.
/// For trait impls (name="Trait_Type"), extracts the type after "for" from
/// the signature, or falls back to the portion after the last underscore.
fn extract_self_type_from_impl(name: &str, signature: &Option<String>) -> String {
    // Try to extract from signature first: "impl Trait for Type" or "impl Type"
    if let Some(sig) = signature {
        let sig = sig.trim();
        // Strip "unsafe " prefix
        let sig = sig.strip_prefix("unsafe ").unwrap_or(sig);
        // Strip "impl" prefix
        if let Some(rest) = sig.strip_prefix("impl") {
            let rest = rest.trim();
            // Strip leading generics <T: ...>
            let rest = if rest.starts_with('<') {
                let mut depth = 0;
                let mut end = 0;
                for (i, c) in rest.char_indices() {
                    match c {
                        '<' => depth += 1,
                        '>' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                rest[end..].trim()
            } else {
                rest
            };

            if let Some(for_pos) = rest.find(" for ") {
                // "Trait for Type<T>" → extract "Type" (strip generics)
                let type_part = rest[for_pos + 5..].trim();
                let type_name = if let Some(angle) = type_part.find('<') {
                    type_part[..angle].trim()
                } else {
                    // Strip trailing whitespace or braces
                    type_part.split_whitespace().next().unwrap_or(type_part)
                };
                if !type_name.is_empty() {
                    return type_name.to_string();
                }
            } else {
                // "impl Type" → inherent impl
                let type_name = if let Some(angle) = rest.find('<') {
                    rest[..angle].trim()
                } else {
                    rest.split_whitespace().next().unwrap_or(rest)
                };
                if !type_name.is_empty() {
                    return type_name.to_string();
                }
            }
        }
    }

    // Fallback: use the name field
    // Inherent: name = "Type" → return as-is
    // Trait impl: name = "Trait_Type" → return portion after last _
    // But be careful with types like "MyStruct" that contain no _
    if name.contains('_') {
        // Could be "Trait_Type" — take after last _
        if let Some(pos) = name.rfind('_') {
            let after = &name[pos + 1..];
            if !after.is_empty() && after.starts_with(|c: char| c.is_uppercase()) {
                return after.to_string();
            }
        }
    }

    name.to_string()
}
