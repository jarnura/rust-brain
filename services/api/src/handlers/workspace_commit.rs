//! Workspace commit handler.
//!
//! `POST /workspaces/:id/commit` — run `git add -A && git commit -m <message>`
//! in the workspace clone directory and return the resulting commit SHA.

use axum::{
    extract::{Extension, Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::middleware::auth::{require_write_access, ApiKeyContext};
use crate::state::AppState;
use crate::workspace::get_workspace as db_get_workspace;

// =============================================================================
// Request / Response types
// =============================================================================

/// Body for `POST /workspaces/:id/commit`.
#[derive(Debug, Deserialize)]
pub struct CommitRequest {
    /// Commit message.
    pub message: String,
}

/// Response body for `POST /workspaces/:id/commit`.
#[derive(Debug, Serialize)]
pub struct CommitResponse {
    /// Short commit SHA (7 chars).
    pub sha: String,
    /// Full commit message used.
    pub message: String,
}

// =============================================================================
// Handler
// =============================================================================

/// `POST /workspaces/:id/commit` — stage all changes and commit.
///
/// Returns the new commit SHA on success.  Returns `400` if the workspace is
/// not yet cloned or there is nothing to commit.
pub async fn workspace_commit(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CommitRequest>,
) -> Result<Json<CommitResponse>, AppError> {
    require_write_access(&ctx)?;
    if req.message.trim().is_empty() {
        return Err(AppError::BadRequest(
            "commit message must not be empty".to_string(),
        ));
    }

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

    // Stage everything
    run_git_add_all(&clone_path).await?;

    // Commit
    let sha = run_git_commit(&clone_path, &req.message).await?;

    info!(
        workspace_id = %workspace_id,
        sha = %sha,
        "Committed workspace changes"
    );

    Ok(Json(CommitResponse {
        sha,
        message: req.message,
    }))
}

// =============================================================================
// Git helpers
// =============================================================================

/// Stage all changes with `git add -A`.
async fn run_git_add_all(clone_path: &str) -> Result<(), AppError> {
    let output = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to spawn git add: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(clone_path = %clone_path, stderr = %stderr, "git add failed");
        return Err(AppError::Internal(format!(
            "git add failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

/// Commit staged changes and return the short SHA.
async fn run_git_commit(clone_path: &str, message: &str) -> Result<String, AppError> {
    // Configure a default identity in case the container has none
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.email", "rustbrain@localhost"])
        .current_dir(clone_path)
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.name", "rustbrain"])
        .current_dir(clone_path)
        .output()
        .await;

    let output = tokio::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to spawn git commit: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // "nothing to commit" is a user-facing 400, not a 500
        if stdout.contains("nothing to commit") || stderr.contains("nothing to commit") {
            return Err(AppError::BadRequest("nothing to commit".to_string()));
        }
        warn!(clone_path = %clone_path, stderr = %stderr, "git commit failed");
        return Err(AppError::Internal(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    // Extract short SHA from `git rev-parse --short HEAD`
    let sha_output = tokio::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(clone_path)
        .output()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to read commit SHA: {}", e)))?;

    let sha = String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string();

    Ok(sha)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_request_deserializes() {
        let json = r#"{"message": "feat: add login endpoint"}"#;
        let req: CommitRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "feat: add login endpoint");
    }

    #[test]
    fn commit_response_serializes() {
        let r = CommitResponse {
            sha: "abc1234".to_string(),
            message: "feat: add login".to_string(),
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["sha"], "abc1234");
        assert_eq!(json["message"], "feat: add login");
    }

    #[tokio::test]
    async fn git_add_all_invalid_path_returns_error() {
        let result = run_git_add_all("/nonexistent/path").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn git_commit_non_git_dir_returns_error() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No git repo initialised — commit should fail
        let result = run_git_commit(tmp.path().to_str().unwrap(), "test commit").await;
        assert!(result.is_err());
    }
}
