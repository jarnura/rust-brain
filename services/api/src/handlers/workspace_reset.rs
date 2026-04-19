//! Workspace reset handler.
//!
//! `POST /workspaces/:id/reset` — run `git reset --hard HEAD` and
//! `git clean -fd` in the workspace clone directory to discard all
//! uncommitted changes, then mark any running execution as `aborted`.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::state::AppState;
use crate::workspace::get_workspace as db_get_workspace;

// =============================================================================
// Response types
// =============================================================================

/// Response body for `POST /workspaces/:id/reset`.
#[derive(Debug, Serialize)]
pub struct ResetResponse {
    /// Human-readable confirmation.
    pub message: String,
    /// The HEAD commit SHA after reset.
    pub head_sha: String,
}

// =============================================================================
// Handler
// =============================================================================

/// `POST /workspaces/:id/reset` — discard all uncommitted changes.
///
/// Runs `git reset --hard HEAD` followed by `git clean -fd` to remove
/// untracked files.  Also aborts any running execution for the workspace.
pub async fn workspace_reset(
    State(state): State<AppState>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<ResetResponse>, AppError> {
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

    // Reset and clean
    run_git_reset(&clone_path).await?;
    run_git_clean(&clone_path).await?;

    // Abort any running execution for this workspace
    abort_running_executions(&state, workspace_id).await;

    let head_sha = read_head_sha(&clone_path).await?;

    info!(
        workspace_id = %workspace_id,
        head_sha = %head_sha,
        "Workspace reset to HEAD"
    );

    Ok(Json(ResetResponse {
        message: "Workspace reset to HEAD. All uncommitted changes discarded.".to_string(),
        head_sha,
    }))
}

// =============================================================================
// Git helpers
// =============================================================================

/// Run `git reset --hard HEAD`.
async fn run_git_reset(clone_path: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("git")
        .args(["reset", "--hard", "HEAD"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to spawn git reset: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(clone_path = %clone_path, stderr = %stderr, "git reset failed");
        return Err(AppError::Internal(format!(
            "git reset failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Run `git clean -fd` to remove untracked files.
async fn run_git_clean(clone_path: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("git")
        .args(["clean", "-fd"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to spawn git clean: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(clone_path = %clone_path, stderr = %stderr, "git clean failed");
        return Err(AppError::Internal(format!(
            "git clean failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Read the HEAD commit SHA (short).
async fn read_head_sha(clone_path: &str) -> Result<String, AppError> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read HEAD SHA: {}", e)))?;

    if !output.status.success() {
        return Err(AppError::Internal(
            "Failed to read HEAD SHA after reset".to_string(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Mark any `running` executions for this workspace as `aborted`.
async fn abort_running_executions(state: &AppState, workspace_id: Uuid) {
    if let Err(e) = sqlx::query(
        r#"
        UPDATE executions
        SET status = 'aborted', completed_at = NOW()
        WHERE workspace_id = $1 AND status = 'running'
        "#,
    )
    .bind(workspace_id)
    .execute(&state.workspace_manager.pool)
    .await
    {
        warn!(workspace_id = %workspace_id, error = %e, "Failed to abort running executions on reset");
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_response_serializes() {
        let r = ResetResponse {
            message: "Workspace reset to HEAD. All uncommitted changes discarded.".to_string(),
            head_sha: "abc1234".to_string(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert!(json["message"].as_str().unwrap().contains("HEAD"));
        assert_eq!(json["head_sha"], "abc1234");
    }

    #[tokio::test]
    async fn git_reset_invalid_path_returns_error() {
        let result = run_git_reset("/nonexistent/path").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn git_clean_invalid_path_returns_error() {
        let result = run_git_clean("/nonexistent/path").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn read_head_sha_invalid_path_returns_error() {
        let result = read_head_sha("/nonexistent/path").await;
        assert!(result.is_err());
    }
}
