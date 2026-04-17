//! Axum extractors for the rust-brain API server.
//!
//! Provides [`WorkspaceId`] — an extractor that validates the `X-Workspace-Id`
//! header and returns a [`crate::neo4j::WorkspaceContext`].

use axum::{async_trait, extract::FromRequestParts, http::request::Parts};

use crate::errors::AppError;
use crate::neo4j::WorkspaceContext;

/// Axum extractor that validates the `X-Workspace-Id` header.
///
/// Returns a [`WorkspaceContext`] if the header is present and valid.
/// Returns 400 if the header is missing or contains an invalid workspace ID.
///
/// # Usage
///
/// ```ignore
/// async fn my_handler(
///     State(state): State<AppState>,
///     WorkspaceId(ws): WorkspaceId,
/// ) -> Result<Json<...>, AppError> {
///     let client = WorkspaceGraphClient::new(state.neo4j_graph.clone(), ws);
///     // ...
/// }
/// ```
#[derive(Debug)]
pub struct WorkspaceId(pub WorkspaceContext);

#[async_trait]
impl FromRequestParts<AppState> for WorkspaceId {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let workspace_id = parts
            .headers
            .get("X-Workspace-Id")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::BadRequest("Missing X-Workspace-Id header".to_string()))?;

        let ctx = WorkspaceContext::new(workspace_id.to_string())?;
        Ok(WorkspaceId(ctx))
    }
}

use crate::state::AppState;
