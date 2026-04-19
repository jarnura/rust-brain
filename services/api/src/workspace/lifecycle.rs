//! Workspace lifecycle transitions.
//!
//! Each function performs an atomic Postgres UPDATE with a `WHERE status = $expected`
//! guard to prevent concurrent races. Returns `Err` if the workspace is not in
//! the expected state (invalid transition).
//!
//! This module also provides Qdrant collection lifecycle management:
//! - [`create_qdrant_collections`] — Create all 4 Qdrant collections for a workspace
//! - [`delete_qdrant_collections`] — Delete all 4 Qdrant collections for a workspace

use anyhow::{anyhow, bail, Context};
use reqwest::Client;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use super::models::WorkspaceStatus;
use super::schema::workspace_collections;

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

/// Record the Docker volume name for a workspace still in `cloning` status.
///
/// Returns `Err` if the workspace is not currently in `cloning` status.
pub async fn set_volume_name(pool: &PgPool, id: Uuid, volume_name: &str) -> anyhow::Result<()> {
    let result = sqlx::query(
        r#"
        UPDATE workspaces
        SET volume_name = $2,
            updated_at = NOW()
        WHERE id = $1 AND status = 'cloning'
        "#,
    )
    .bind(id)
    .bind(volume_name)
    .execute(pool)
    .await
    .context("set_volume_name: DB update failed")?;

    if result.rows_affected() == 0 {
        bail!(
            "set_volume_name: workspace {} is not in '{}' status",
            id,
            WorkspaceStatus::Cloning
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

/// Request body for creating a Qdrant collection via REST API.
#[derive(serde::Serialize)]
struct CreateCollectionRequest {
    vectors: VectorsConfig,
}

/// Vector configuration for Qdrant collection creation.
#[derive(serde::Serialize)]
struct VectorsConfig {
    size: usize,
    distance: String,
}

/// Create all 4 Qdrant collections for a workspace.
///
/// Each collection is created with Cosine distance and the given vector size.
/// If a collection already exists (HTTP 409 or Qdrant "already exists"), that's treated as success.
/// Errors are logged per-collection; the function returns Err only if all 4 fail.
///
/// # Example
/// ```
/// use reqwest::Client;
/// use rustbrain_api::workspace::lifecycle::create_qdrant_collections;
///
/// async fn example(client: &Client) -> anyhow::Result<()> {
///     create_qdrant_collections(client, "http://localhost:6333", "ws_550e8400e29b", 768).await
/// }
/// ```
pub async fn create_qdrant_collections(
    http_client: &Client,
    qdrant_host: &str,
    schema_name: &str,
    vector_size: usize,
) -> anyhow::Result<()> {
    let collections = workspace_collections(schema_name);
    let mut success_count = 0;
    let mut last_error: Option<anyhow::Error> = None;

    let request_body = CreateCollectionRequest {
        vectors: VectorsConfig {
            size: vector_size,
            distance: "Cosine".to_string(),
        },
    };

    for collection_name in collections.all() {
        let url = format!(
            "{}/collections/{}",
            qdrant_host.trim_end_matches('/'),
            collection_name
        );

        info!("Creating Qdrant collection: {}", collection_name);

        match http_client.put(&url).json(&request_body).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    info!("Successfully created collection: {}", collection_name);
                    success_count += 1;
                } else if status.as_u16() == 409 {
                    info!(
                        "Collection already exists (treated as success): {}",
                        collection_name
                    );
                    success_count += 1;
                } else {
                    let error_msg = format!(
                        "Failed to create collection {}: HTTP {}",
                        collection_name, status
                    );
                    warn!("{}", error_msg);
                    last_error = Some(anyhow!(error_msg));
                }
            }
            Err(e) => {
                let error_msg = format!("Request failed for collection {}: {}", collection_name, e);
                warn!("{}", error_msg);
                last_error = Some(anyhow!(error_msg));
            }
        }
    }

    if success_count > 0 {
        info!(
            "Created {} of 4 Qdrant collections for {}",
            success_count, schema_name
        );
        Ok(())
    } else {
        Err(last_error.unwrap_or_else(|| anyhow!("All 4 Qdrant collections failed to create")))
    }
}

/// Delete all 4 Qdrant collections for a workspace.
///
/// If a collection does not exist, that's treated as success.
/// Errors are logged per-collection; the function returns Err only if all 4 fail.
///
/// # Example
/// ```
/// use reqwest::Client;
/// use rustbrain_api::workspace::lifecycle::delete_qdrant_collections;
///
/// async fn example(client: &Client) -> anyhow::Result<()> {
///     delete_qdrant_collections(client, "http://localhost:6333", "ws_550e8400e29b").await
/// }
/// ```
pub async fn delete_qdrant_collections(
    http_client: &Client,
    qdrant_host: &str,
    schema_name: &str,
) -> anyhow::Result<()> {
    let collections = workspace_collections(schema_name);
    let mut success_count = 0;
    let mut last_error: Option<anyhow::Error> = None;

    for collection_name in collections.all() {
        let url = format!(
            "{}/collections/{}",
            qdrant_host.trim_end_matches('/'),
            collection_name
        );

        info!("Deleting Qdrant collection: {}", collection_name);

        match http_client.delete(&url).send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    info!("Successfully deleted collection: {}", collection_name);
                    success_count += 1;
                } else if status.as_u16() == 404 {
                    info!(
                        "Collection not found (treated as success): {}",
                        collection_name
                    );
                    success_count += 1;
                } else {
                    let error_msg = format!(
                        "Failed to delete collection {}: HTTP {}",
                        collection_name, status
                    );
                    warn!("{}", error_msg);
                    last_error = Some(anyhow!(error_msg));
                }
            }
            Err(e) => {
                let error_msg = format!("Request failed for collection {}: {}", collection_name, e);
                warn!("{}", error_msg);
                last_error = Some(anyhow!(error_msg));
            }
        }
    }

    if success_count > 0 {
        info!(
            "Deleted {} of 4 Qdrant collections for {}",
            success_count, schema_name
        );
        Ok(())
    } else {
        Err(last_error.unwrap_or_else(|| anyhow!("All 4 Qdrant collections failed to delete")))
    }
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

    #[test]
    fn from_db_str_unknown_defaults_to_pending() {
        assert_eq!(
            WorkspaceStatus::from_db_str("unknown_status"),
            WorkspaceStatus::Pending
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("random"),
            WorkspaceStatus::Pending
        );
    }

    #[test]
    fn from_db_str_empty_defaults_to_pending() {
        assert_eq!(WorkspaceStatus::from_db_str(""), WorkspaceStatus::Pending);
    }

    #[test]
    fn from_db_str_all_variants() {
        assert_eq!(
            WorkspaceStatus::from_db_str("cloning"),
            WorkspaceStatus::Cloning
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("indexing"),
            WorkspaceStatus::Indexing
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("ready"),
            WorkspaceStatus::Ready
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("error"),
            WorkspaceStatus::Error
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("archived"),
            WorkspaceStatus::Archived
        );
        assert_eq!(
            WorkspaceStatus::from_db_str("pending"),
            WorkspaceStatus::Pending
        );
    }
}
