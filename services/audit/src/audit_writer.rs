//! Audit log writer for the workspace_audit_log table.
//!
//! Writes records for detected leaks, Neo4j workspace discrepancies,
//! and cleaned resources. Also handles retention-based pruning of old
//! audit entries.

use sqlx::PgPool;
use tracing::{debug, info};
use uuid::Uuid;

use crate::neo4j_scanner::{CrossWorkspaceDetail, LabelMismatchDetail};

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

pub async fn record_cross_workspace_relationship(
    pool: &PgPool,
    detail: &CrossWorkspaceDetail,
) -> anyhow::Result<()> {
    let workspace_id = workspace_id_from_label(&detail.source_workspace);

    let detail_json = serde_json::json!({
        "source_fqn": detail.source_fqn,
        "source_workspace": detail.source_workspace,
        "target_fqn": detail.target_fqn,
        "target_workspace": detail.target_workspace,
        "rel_type": detail.rel_type,
    });

    sqlx::query(
        r#"
        INSERT INTO workspace_audit_log (workspace_id, operation, actor, detail)
        VALUES ($1, 'cross_workspace_relationship', 'audit-service', $2)
        "#,
    )
    .bind(workspace_id)
    .bind(detail_json)
    .execute(pool)
    .await?;

    debug!(
        "Recorded cross_workspace_relationship: {} -[:{}]-> {}",
        detail.source_fqn, detail.rel_type, detail.target_fqn
    );

    Ok(())
}

pub async fn record_label_mismatch(
    pool: &PgPool,
    detail: &LabelMismatchDetail,
) -> anyhow::Result<()> {
    let workspace_id = workspace_id_from_label(&detail.actual_workspace);

    let detail_json = serde_json::json!({
        "fqn": detail.fqn,
        "actual_workspace": detail.actual_workspace,
        "expected_workspace": detail.expected_workspace,
        "neighbor_count": detail.neighbor_count,
    });

    sqlx::query(
        r#"
        INSERT INTO workspace_audit_log (workspace_id, operation, actor, detail)
        VALUES ($1, 'label_mismatch', 'audit-service', $2)
        "#,
    )
    .bind(workspace_id)
    .bind(detail_json)
    .execute(pool)
    .await?;

    debug!(
        "Recorded label_mismatch: {} has {} but neighbors suggest {}",
        detail.fqn, detail.actual_workspace, detail.expected_workspace
    );

    Ok(())
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

fn workspace_id_from_label(label: &str) -> Uuid {
    match label.strip_prefix("Workspace_") {
        Some(hex) if hex.len() >= 32 => {
            let padded = format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            );
            Uuid::parse_str(&padded).unwrap_or(Uuid::nil())
        }
        _ => Uuid::nil(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_id_from_label_valid() {
        let uuid_str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6";
        let label = format!("Workspace_{}", uuid_str);
        let result = workspace_id_from_label(&label);

        let expected = Uuid::parse_str("a1b2c3d4-e5f6-a7b8-c9d0-e1f2a3b4c5d6").unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn test_workspace_id_from_label_no_prefix() {
        let result = workspace_id_from_label("some_random_string");
        assert_eq!(result, Uuid::nil());
    }

    #[test]
    fn test_workspace_id_from_label_short_hex() {
        let result = workspace_id_from_label("Workspace_abc123");
        assert_eq!(result, Uuid::nil());
    }

    #[test]
    fn test_workspace_id_from_label_invalid_hex() {
        let result = workspace_id_from_label("Workspace_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz");
        assert_eq!(result, Uuid::nil());
    }

    #[test]
    fn test_workspace_id_from_label_legacy_zero() {
        let result = workspace_id_from_label("Workspace_00000000000000000000000000000000");
        assert_eq!(
            result,
            Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap()
        );
    }
}
