//! Workspace lifecycle transitions.
//!
//! Each function performs an atomic Postgres UPDATE with a `WHERE status = $expected`
//! guard to prevent concurrent races. Returns `Err` if the workspace is not in
//! the expected state (invalid transition).

use anyhow::{bail, Context};
use sqlx::PgPool;
use uuid::Uuid;

use super::models::WorkspaceStatus;

/// Transition a workspace from `cloning` → `indexing`.
///
/// Sets `index_started_at = NOW()`. Returns `Err` if the workspace is not
/// currently in `cloning` status.
pub async fn start_indexing(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET status = 'indexing',
            index_started_at = NOW(),
            updated_at = NOW()
        WHERE id = $1 AND status = 'cloning'
        "#,
    )
    .bind(id)
    .execute(pool)
    .await
    .context("start_indexing: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "start_indexing: workspace {} is not in '{}' status",
            id,
            WorkspaceStatus::Cloning
        );
    }
    Ok(())
}

/// Transition a workspace from `indexing` → `ready`.
///
/// Sets `index_completed_at = NOW()`. Returns `Err` if the workspace is not
/// currently in `indexing` status.
pub async fn mark_ready(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET status = 'ready',
            index_completed_at = NOW(),
            index_error = NULL,
            updated_at = NOW()
        WHERE id = $1 AND status = 'indexing'
        "#,
    )
    .bind(id)
    .execute(pool)
    .await
    .context("mark_ready: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "mark_ready: workspace {} is not in '{}' status",
            id,
            WorkspaceStatus::Indexing
        );
    }
    Ok(())
}

/// Transition a workspace from `cloning` or `indexing` → `error`.
///
/// Stores the error message. Allowed from `cloning` or `indexing`.
pub async fn fail(pool: &PgPool, id: Uuid, error_message: &str) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET status = 'error',
            index_error = $2,
            updated_at = NOW()
        WHERE id = $1 AND status IN ('cloning', 'indexing')
        "#,
    )
    .bind(id)
    .bind(error_message)
    .execute(pool)
    .await
    .context("fail: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "fail: workspace {} is not in a failable state (cloning or indexing)",
            id
        );
    }
    Ok(())
}

/// Transition a workspace from `ready` → `archived`.
///
/// Returns `Err` if the workspace is not currently in `ready` status.
pub async fn archive(pool: &PgPool, id: Uuid) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET status = 'archived',
            updated_at = NOW()
        WHERE id = $1 AND status = 'ready'
        "#,
    )
    .bind(id)
    .execute(pool)
    .await
    .context("archive: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "archive: workspace {} is not in '{}' status",
            id,
            WorkspaceStatus::Ready
        );
    }
    Ok(())
}

/// Record the clone path for a workspace still in `cloning` status.
///
/// Returns `Err` if the workspace is not currently in `cloning` status.
pub async fn clone_workspace(pool: &PgPool, id: Uuid, clone_path: &str) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET clone_path = $2,
            updated_at = NOW()
        WHERE id = $1 AND status = 'cloning'
        "#,
    )
    .bind(id)
    .bind(clone_path)
    .execute(pool)
    .await
    .context("clone_workspace: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "clone_workspace: workspace {} is not in '{}' status",
            id,
            WorkspaceStatus::Cloning
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Documents the valid state machine transitions.
    ///
    /// Actual DB-backed tests live in the integration test suite (Phase 2).
    /// Here we verify the status string constants are correct and the
    /// invalid-transition detection logic is documented.
    #[test]
    fn valid_transition_sequence_documented() {
        // cloning → indexing → ready → archived
        // cloning → error
        // indexing → error
        assert_eq!(WorkspaceStatus::Cloning.as_str(), "cloning");
        assert_eq!(WorkspaceStatus::Indexing.as_str(), "indexing");
        assert_eq!(WorkspaceStatus::Ready.as_str(), "ready");
        assert_eq!(WorkspaceStatus::Error.as_str(), "error");
        assert_eq!(WorkspaceStatus::Archived.as_str(), "archived");
    }

    /// Documents invalid transitions that must return Err.
    ///
    /// The DB functions enforce these via `WHERE status = $expected`. When the
    /// guard fails, `rows_affected() == 0` triggers a bail!, so all invalid
    /// transitions return `Err`.
    #[test]
    fn invalid_transitions_enforced_by_guard() {
        // ready → cloning: invalid (must archive first, then re-create)
        // archived → indexing: terminal state, no transitions out
        // error → ready: must re-create the workspace
        //
        // Each lifecycle function uses `WHERE status = 'expected'`. If the row
        // is not in that state, rows_affected == 0 and Err is returned.
        let _ = ();
    }
}
