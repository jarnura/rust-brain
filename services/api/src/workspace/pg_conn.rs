//! Workspace-scoped Postgres connection acquisition.
//!
//! Provides [`acquire_conn`] which returns a `PgConnection` with `search_path`
//! set to the workspace schema when a workspace context is present, or a plain
//! connection (defaulting to `public`) when absent.
//!
//! # Design
//!
//! The `search_path` is set at the **connection level** (not transaction level)
//! per the RUSA-182 acceptance criteria. This ensures all subsequent queries on
//! that connection resolve unqualified table references against the workspace
//! schema first, then `public` as fallback — without modifying any SQL strings.

use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};
use tracing::debug;

use super::schema::schema_name_for;
use crate::neo4j::WorkspaceContext;

/// Acquire a Postgres connection with optional workspace schema scoping.
///
/// When `workspace_ctx` is `Some`, sets `search_path` to `ws_<short_id>, public`
/// so all unqualified table references resolve to the workspace schema first.
/// When `None`, returns a plain connection with the default `search_path`.
pub async fn acquire_conn(
    pool: &PgPool,
    workspace_ctx: Option<&WorkspaceContext>,
) -> Result<PoolConnection<Postgres>, crate::errors::AppError> {
    let mut conn = pool.acquire().await.map_err(|e| {
        crate::errors::AppError::Database(format!("Failed to acquire connection: {}", e))
    })?;

    if let Some(ctx) = workspace_ctx {
        let ws_id = ctx.workspace_id();
        let schema = schema_name_for(ws_id);
        debug!(workspace_id = %ws_id, schema = %schema, "Setting search_path for workspace");

        let set_sql = format!("SET search_path TO {}, public", schema);
        sqlx::query(&set_sql)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                crate::errors::AppError::Database(format!(
                    "Failed to set search_path for workspace {}: {}",
                    ws_id, e
                ))
            })?;
    } else {
        sqlx::query("SET search_path TO public")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                crate::errors::AppError::Database(format!("Failed to reset search_path: {}", e))
            })?;
    }

    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_name_for_workspace() {
        let ws_id = "550e8400-e29b-41d4-a716-446655440000";
        let schema = schema_name_for(ws_id);
        assert_eq!(schema, "ws_550e8400e29b");
    }
}
