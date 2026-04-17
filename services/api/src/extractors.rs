//! Axum extractors for the rust-brain API server.
//!
//! Provides [`WorkspaceId`] (required) and [`OptionalWorkspaceId`] (optional)
//! extractors that validate the `X-Workspace-Id` header and return a
//! [`crate::neo4j::WorkspaceContext`].

use axum::{async_trait, extract::FromRequestParts, http::request::Parts};

use crate::errors::AppError;
use crate::neo4j::WorkspaceContext;
use crate::state::AppState;

/// Axum extractor that validates the `X-Workspace-Id` header.
///
/// Returns 400 if the header is missing or contains an invalid workspace ID.
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

/// Axum extractor that optionally validates the `X-Workspace-Id` header.
///
/// Returns `None` when the header is absent (backwards compatible).
/// Returns 400 only if the header is present but contains an invalid value.
#[derive(Debug)]
pub struct OptionalWorkspaceId(pub Option<WorkspaceContext>);

#[async_trait]
impl FromRequestParts<AppState> for OptionalWorkspaceId {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(header_value) = parts
            .headers
            .get("X-Workspace-Id")
            .and_then(|v| v.to_str().ok())
        else {
            return Ok(OptionalWorkspaceId(None));
        };

        if header_value.is_empty() {
            return Ok(OptionalWorkspaceId(None));
        }

        let ctx = WorkspaceContext::new(header_value.to_string())?;
        Ok(OptionalWorkspaceId(Some(ctx)))
    }
}
