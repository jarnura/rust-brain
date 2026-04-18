//! Workspace diff handler.
//!
//! `GET /workspaces/:id/diff` — run `git diff` inside the workspace clone
//! directory and return the unified patch as JSON.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use tracing::warn;
use uuid::Uuid;

use crate::errors::AppError;
use crate::state::AppState;
use crate::workspace::get_workspace as db_get_workspace;

// =============================================================================
// Response types
// =============================================================================

/// Response body for `GET /workspaces/:id/diff`.
#[derive(Debug, Serialize)]
pub struct DiffResponse {
    /// Unified diff output from `git diff HEAD`.
    pub patch: String,
    /// `true` when there are no staged or unstaged changes.
    pub clean: bool,
}

// =============================================================================
// Handler
// =============================================================================

/// `GET /workspaces/:id/diff` — return the unified patch for the workspace.
///
/// Runs `git diff HEAD` in the workspace clone directory.
/// Returns a `400` if the workspace is not yet cloned.
pub async fn workspace_diff(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<DiffResponse>, AppError> {
    state.metrics.record_request("workspace_diff", "GET");

    let workspace = db_get_workspace(&state.workspace_manager.pool, workspace_id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", workspace_id)))?;

    let clone_path = workspace.clone_path.ok_or_else(|| {
        AppError::BadRequest(format!(
            "Workspace {} is not yet cloned (status: {})",
            workspace_id, workspace.status
        ))
    })?;

    let patch = run_git_diff(&clone_path).await?;
    let clean = patch.is_empty();

    Ok(Json(DiffResponse { patch, clean }))
}

// =============================================================================
// Git helpers
// =============================================================================

/// Run `git diff HEAD` in `clone_path` and return the output string.
async fn run_git_diff(clone_path: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to spawn git diff: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(clone_path = %clone_path, stderr = %stderr, "git diff failed");
        return Err(AppError::Internal(format!(
            "git diff failed: {}",
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_response_clean_when_empty_patch() {
        let r = DiffResponse {
            patch: String::new(),
            clean: true,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["clean"], true);
        assert_eq!(json["patch"], "");
    }

    #[test]
    fn diff_response_not_clean_when_patch_present() {
        let r = DiffResponse {
            patch: "diff --git a/src/main.rs b/src/main.rs".to_string(),
            clean: false,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["clean"], false);
        assert!(json["patch"].as_str().unwrap().contains("diff --git"));
    }

    #[tokio::test]
    async fn run_git_diff_invalid_path_returns_error() {
        let result = run_git_diff("/nonexistent/path/that/cannot/be/a/git/repo").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_git_diff_on_non_git_dir_returns_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = run_git_diff(tmp.path().to_str().unwrap()).await;
        assert!(result.is_err());
    }
}
