//! `TenantPool` — workspace-scoped Postgres connection pool.
//!
//! Wraps a shared `sqlx::PgPool` and sets `search_path` to the workspace's
//! `ws_<12hex>` schema on every acquired connection. All subsequent SQL on
//! that connection resolves unqualified table names against the workspace
//! schema first, falling back to `public`.
//!
//! The schema name is derived from the workspace UUID by stripping dashes and
//! taking the first 12 hex characters:
//!
//! ```text
//! 550e8400-e29b-41d4-a716-446655440000  →  ws_550e8400e29b
//! ```
//!
//! # Validation
//!
//! [`TenantPool::new`] validates the derived schema name against
//! `ws_[0-9a-f]{12}` and returns an error for invalid UUIDs.

use anyhow::{bail, Context, Result};
use sqlx::pool::PoolConnection;
use sqlx::{PgPool, Postgres};
use tracing::debug;
use uuid::Uuid;

/// A workspace-scoped connection pool.
///
/// Created once per projector run. `acquire()` sets `search_path` on every
/// connection so queries target the correct tenant schema without SQL changes.
#[derive(Clone, Debug)]
pub struct TenantPool {
    pool: PgPool,
    schema_name: String,
    tenant_id: Uuid,
}

impl TenantPool {
    /// Create a new `TenantPool` for the given tenant.
    ///
    /// # Errors
    ///
    /// Returns an error if `tenant_id` cannot produce a valid `ws_[0-9a-f]{12}`
    /// schema name (which should never happen for a valid UUID).
    pub fn new(pool: PgPool, tenant_id: Uuid) -> Result<Self> {
        let schema_name = schema_name_for(&tenant_id.to_string());
        validate_schema_name(&schema_name)
            .with_context(|| format!("Invalid schema name derived from tenant_id {tenant_id}"))?;
        debug!(tenant_id = %tenant_id, schema = %schema_name, "TenantPool::new");
        Ok(Self { pool, schema_name, tenant_id })
    }

    /// Acquire a connection with `search_path` set to this tenant's schema.
    ///
    /// The connection is returned to the pool when the guard is dropped.
    pub async fn acquire(&self) -> Result<PoolConnection<Postgres>> {
        let mut conn = self.pool.acquire().await.with_context(|| {
            format!("Failed to acquire connection for tenant {}", self.tenant_id)
        })?;
        let sql = format!("SET search_path TO {}, public", self.schema_name);
        sqlx::query(&sql)
            .execute(&mut *conn)
            .await
            .with_context(|| {
                format!(
                    "Failed to set search_path={} for tenant {}",
                    self.schema_name, self.tenant_id
                )
            })?;
        Ok(conn)
    }

    /// The schema name for this tenant (e.g., `ws_550e8400e29b`).
    pub fn schema_name(&self) -> &str {
        &self.schema_name
    }

    /// The tenant UUID.
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }
}

/// Derive the schema name from a workspace UUID string.
///
/// Strips dashes and takes the first 12 hex characters, prefixed with `ws_`.
pub fn schema_name_for(workspace_id: &str) -> String {
    let hex = workspace_id.replace('-', "");
    let short = &hex[..12.min(hex.len())];
    format!("ws_{short}")
}

/// Validate that `name` matches `ws_[0-9a-f]{12}`.
fn validate_schema_name(name: &str) -> Result<()> {
    if !name.starts_with("ws_") {
        bail!("Schema name '{name}' must start with 'ws_'");
    }
    let suffix = &name[3..];
    if suffix.len() != 12 || !suffix.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "Schema name '{name}': suffix must be exactly 12 hex characters, got '{suffix}'"
        );
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_for_standard_uuid() {
        let name = schema_name_for("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(name, "ws_550e8400e29b");
    }

    #[test]
    fn schema_name_for_all_zeros() {
        let name = schema_name_for("00000000-0000-0000-0000-000000000000");
        assert_eq!(name, "ws_000000000000");
    }

    #[test]
    fn schema_name_for_all_fs() {
        let name = schema_name_for("ffffffff-ffff-ffff-ffff-ffffffffffff");
        assert_eq!(name, "ws_ffffffffffff");
    }

    #[test]
    fn schema_name_from_uuid_type() {
        let id = Uuid::parse_str("abcdef12-3456-7890-abcd-ef1234567890").unwrap();
        let name = schema_name_for(&id.to_string());
        assert_eq!(name, "ws_abcdef123456");
    }

    #[test]
    fn validate_accepts_valid_schema() {
        assert!(validate_schema_name("ws_abcdef123456").is_ok());
        assert!(validate_schema_name("ws_000000000000").is_ok());
        assert!(validate_schema_name("ws_ffffffffffff").is_ok());
        assert!(validate_schema_name("ws_abcdef012345").is_ok());
    }

    #[test]
    fn validate_rejects_missing_prefix() {
        assert!(validate_schema_name("abcdef123456").is_err());
        assert!(validate_schema_name("workspace_abc123456789").is_err());
    }

    #[test]
    fn validate_rejects_wrong_suffix_length() {
        assert!(validate_schema_name("ws_abc").is_err());
        assert!(validate_schema_name("ws_abcdef12345").is_err()); // 13 chars
        assert!(validate_schema_name("ws_abcdef1234").is_err()); // 11 chars
    }

    #[test]
    fn validate_rejects_non_hex_suffix() {
        assert!(validate_schema_name("ws_xyz012345678").is_err());
        assert!(validate_schema_name("ws_abcdef12345g").is_err()); // 'g' not hex
        assert!(validate_schema_name("ws_abcdef!23456").is_err()); // '!' not hex
    }

    #[tokio::test]
    async fn tenant_pool_new_valid_uuid() {
        let pool = PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let tenant_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let tp = TenantPool::new(pool, tenant_id).unwrap();
        assert_eq!(tp.schema_name(), "ws_550e8400e29b");
        assert_eq!(tp.tenant_id(), tenant_id);
    }

    #[tokio::test]
    async fn tenant_pool_schema_name_accessor() {
        let pool = PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let id = Uuid::new_v4();
        let tp = TenantPool::new(pool, id).unwrap();
        assert!(tp.schema_name().starts_with("ws_"));
        assert_eq!(tp.schema_name().len(), 15); // "ws_" + 12 hex
    }
}
