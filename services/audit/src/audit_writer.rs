//! Audit log writer for the workspace_audit_log table.
//!
//! Writes records for detected leaks and cleaned resources.
//! Also handles retention-based pruning of old audit entries.

use sqlx::PgPool;
use tracing::{debug, info};
use uuid::Uuid;

/// Records a `leak_detected` event in the workspace_audit_log table.
///
/// Since orphaned resources may not be associated with a specific workspace,
/// we use a nil UUID as the workspace_id placeholder and store the resource
/// identifier in the detail JSONB field.
pub async fn record_leak_detected(
    pool: &PgPool,
    resource_type: &str,
    resource_id: &str,
    dry_run: bool,
) -> anyhow::Result<()> {
    let workspace_id = find_workspace_for_resource(pool, resource_type, resource_id).await;

    let detail = serde_json::json!({
        "resource_type": resource_type,
        "resource_id": resource_id,
        "dry_run": dry_run,
    });

    sqlx::query(
        r#"
        INSERT INTO workspace_audit_log (workspace_id, operation, actor, detail)
        VALUES ($1, 'leak_detected', 'audit-service', $2)
        "#,
    )
    .bind(workspace_id)
    .bind(detail)
    .execute(pool)
    .await?;

    debug!(
        "Recorded leak_detected: {} {} (dry_run={})",
        resource_type, resource_id, dry_run
    );

    Ok(())
}

pub async fn record_leak_cleaned(
    pool: &PgPool,
    resource_type: &str,
    resource_id: &str,
) -> anyhow::Result<()> {
    let workspace_id = find_workspace_for_resource(pool, resource_type, resource_id).await;

    let detail = serde_json::json!({
        "resource_type": resource_type,
        "resource_id": resource_id,
    });

    sqlx::query(
        r#"
        INSERT INTO workspace_audit_log (workspace_id, operation, actor, detail)
        VALUES ($1, 'leak_cleaned', 'audit-service', $2)
        "#,
    )
    .bind(workspace_id)
    .bind(detail)
    .execute(pool)
    .await?;

    debug!("Recorded leak_cleaned: {} {}", resource_type, resource_id);

    Ok(())
}

pub async fn prune_audit_log(pool: &PgPool, retention_days: u32) -> anyhow::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM workspace_audit_log WHERE created_at < now() - interval '1 day' * $1",
    )
    .bind(retention_days as i64)
    .execute(pool)
    .await?;

    let deleted = result.rows_affected();
    if deleted > 0 {
        info!(
            "Pruned {} audit log entries (retention: {}d)",
            deleted, retention_days
        );
    } else {
        debug!(
            "No audit log entries to prune (retention: {}d)",
            retention_days
        );
    }

    Ok(deleted)
}

/// Attempts to find the workspace_id associated with a resource.
///
/// For volumes named `rustbrain-ws-<short_id>`, looks up the workspace by
/// matching the volume_name column. For containers named `rustbrain-exec-<short_id>`,
/// looks up the workspace through the executions table.
///
/// Returns a nil UUID if no workspace is found (resource is truly orphaned).
async fn find_workspace_for_resource(
    pool: &PgPool,
    resource_type: &str,
    resource_id: &str,
) -> Uuid {
    match resource_type {
        "volume" => {
            let result = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM workspaces WHERE volume_name = $1 LIMIT 1",
            )
            .bind(resource_id)
            .fetch_optional(pool)
            .await;

            result.ok().flatten().unwrap_or(Uuid::nil())
        }
        "container" => {
            let result = sqlx::query_scalar::<_, Uuid>(
                "SELECT workspace_id FROM executions WHERE container_id = $1 LIMIT 1",
            )
            .bind(resource_id)
            .fetch_optional(pool)
            .await;

            result.ok().flatten().unwrap_or(Uuid::nil())
        }
        _ => Uuid::nil(),
    }
}
