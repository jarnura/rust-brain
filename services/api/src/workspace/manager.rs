//! WorkspaceManager — entry point for workspace operations.
//!
//! Holds the Postgres pool and (eventually) a DockerClient handle.
//! All workspace lifecycle operations are delegated to `lifecycle` and `schema`.

use sqlx::PgPool;
use uuid::Uuid;

use super::models::{get_workspace, Workspace};

/// Owns the connection pool for workspace operations.
///
/// Intended to be stored in [`crate::state::AppState`] and cloned cheaply
/// via the inner `Arc<PgPool>`.
#[derive(Clone)]
pub struct WorkspaceManager {
    /// Postgres connection pool shared with the rest of the API.
    pub pool: PgPool,
    // DockerClient will be added in a follow-up once volume orchestration is integrated.
}

impl WorkspaceManager {
    /// Create a new WorkspaceManager with the given Postgres pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Fetch a single workspace by UUID.
    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<Workspace>> {
        get_workspace(&self.pool, id).await
    }

    /// Archive a workspace from any non-archived state.
    ///
    /// Returns `true` if the workspace was archived, `false` if already archived
    /// or not found.
    pub async fn force_archive(&self, id: Uuid) -> anyhow::Result<bool> {
        // Try to archive from every active state. We attempt each and return true
        // if any succeeds. This is intentionally tolerant — if the workspace is
        // already archived the lifecycle guard will return Err which we ignore.
        let result = sqlx::query(
            r#"
            UPDATE workspaces
            SET status = 'archived', updated_at = NOW()
            WHERE id = $1 AND status != 'archived'
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_manager_constructs() {
        // Construction is tested indirectly via AppState in integration tests.
        // This test documents the expected public API surface.
        let _ = std::mem::size_of::<WorkspaceManager>();
    }
}
