//! Per-workspace Postgres schema management.
//!
//! Each workspace gets its own schema named `ws_<short_id>` (first 8 chars of
//! the workspace UUID without dashes). This schema contains the same table
//! structure as the main rustbrain schema, scoped to the workspace's codebase.

use anyhow::Context;
use sqlx::PgPool;

/// Derive the schema name for a workspace from its UUID.
///
/// Takes the first 8 hex characters of the UUID (without dashes) to form
/// a short, stable identifier.
///
/// # Example
/// ```
/// use rustbrain_api::workspace::schema::schema_name_for;
/// let name = schema_name_for("550e8400-e29b-41d4-a716-446655440000");
/// assert_eq!(name, "ws_550e8400");
/// ```
pub fn schema_name_for(workspace_id: &str) -> String {
    let hex = workspace_id.replace('-', "");
    let short = &hex[..8.min(hex.len())];
    format!("ws_{short}")
}

/// Create a Postgres schema for the given workspace.
///
/// Executes `CREATE SCHEMA IF NOT EXISTS <schema_name>` and then creates the
/// rustbrain intelligence tables scoped to that schema. Uses raw SQL strings
/// (not compile-time checked) because the schema name is dynamic.
///
/// # Security
///
/// `schema_name` is validated to match `ws_[0-9a-f]{8}` before use. Invalid
/// names return `Err` without executing any SQL.
pub async fn create_workspace_schema(pool: &PgPool, schema_name: &str) -> anyhow::Result<()> {
    validate_schema_name(schema_name)?;

    // CREATE SCHEMA
    let create_schema_sql = format!("CREATE SCHEMA IF NOT EXISTS {schema_name}");
    sqlx::query(&create_schema_sql)
        .execute(pool)
        .await
        .with_context(|| format!("Failed to create schema '{schema_name}'"))?;

    // Create rustbrain intelligence tables scoped to this schema.
    // Each statement must be executed individually since sqlx does not
    // support multiple statements in a single `query().execute()` call.
    for stmt in workspace_ddl_statements(schema_name) {
        sqlx::query(&stmt)
            .execute(pool)
            .await
            .with_context(|| format!("Failed to create tables in schema '{schema_name}'"))?;
    }

    Ok(())
}

/// Validate that a schema name matches `ws_[0-9a-f]{8,12}`.
///
/// Accepts both 8-char (legacy `schema_name_for`) and 12-char
/// (`schema_name_from_id` in the workspace handler) hex suffixes.
fn validate_schema_name(name: &str) -> anyhow::Result<()> {
    if !name.starts_with("ws_") {
        anyhow::bail!("Invalid schema name '{name}': must start with 'ws_'");
    }
    let suffix = &name[3..];
    let valid_len = suffix.len() == 8 || suffix.len() == 12;
    if !valid_len || !suffix.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!(
            "Invalid schema name '{name}': suffix must be 8 or 12 hex characters, got '{suffix}'"
        );
    }
    Ok(())
}

/// Generate individual DDL statements for the rustbrain intelligence tables.
fn workspace_ddl_statements(schema_name: &str) -> Vec<String> {
    vec![
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.source_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    path TEXT NOT NULL UNIQUE,
    content TEXT,
    content_hash TEXT,
    size_bytes BIGINT,
    last_modified TIMESTAMPTZ,
    ingested_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
)"#
        ),
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.extracted_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_file_id UUID REFERENCES {schema_name}.source_files(id) ON DELETE CASCADE,
    fqn TEXT NOT NULL,
    item_type TEXT NOT NULL,
    name TEXT NOT NULL,
    signature TEXT,
    body TEXT,
    doc_comment TEXT,
    visibility TEXT,
    line_start INTEGER,
    line_end INTEGER,
    extracted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (fqn, item_type)
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_extracted_items_fqn ON {schema_name}.extracted_items(fqn)"
        ),
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.call_sites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    caller_fqn TEXT NOT NULL,
    callee_fqn TEXT NOT NULL,
    source_file_id UUID REFERENCES {schema_name}.source_files(id) ON DELETE CASCADE,
    line_number INTEGER,
    extracted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_call_sites_caller ON {schema_name}.call_sites(caller_fqn)"
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_for_derives_correct_prefix() {
        let name = schema_name_for("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(name, "ws_550e8400");
    }

    #[test]
    fn schema_name_for_handles_different_ids() {
        let name = schema_name_for("abcdef12-3456-7890-abcd-ef1234567890");
        assert_eq!(name, "ws_abcdef12");
    }

    #[test]
    fn validate_schema_name_accepts_valid() {
        // 8-char suffix (legacy schema_name_for format)
        assert!(validate_schema_name("ws_abcdef12").is_ok());
        assert!(validate_schema_name("ws_00000000").is_ok());
        assert!(validate_schema_name("ws_ffffffff").is_ok());
        // 12-char suffix (schema_name_from_id format used by the workspace handler)
        assert!(validate_schema_name("ws_abcdef123456").is_ok());
        assert!(validate_schema_name("ws_000000000000").is_ok());
    }

    #[test]
    fn validate_schema_name_rejects_missing_prefix() {
        assert!(validate_schema_name("abcdef12").is_err());
        assert!(validate_schema_name("workspace_abc").is_err());
    }

    #[test]
    fn validate_schema_name_rejects_wrong_suffix_length() {
        assert!(validate_schema_name("ws_abc").is_err());
        // 9, 10, 11, 13 are all invalid — only 8 and 12 are accepted
        assert!(validate_schema_name("ws_abcdef123").is_err());
        assert!(validate_schema_name("ws_abcdef1234567").is_err());
    }

    #[test]
    fn validate_schema_name_rejects_non_hex_suffix() {
        assert!(validate_schema_name("ws_xyz01234").is_err());
        assert!(validate_schema_name("ws_!@#$%^&*").is_err());
    }

    #[test]
    fn workspace_ddl_contains_all_tables() {
        let stmts = workspace_ddl_statements("ws_abcdef12");
        let joined = stmts.join("\n");
        assert!(joined.contains("ws_abcdef12.source_files"));
        assert!(joined.contains("ws_abcdef12.extracted_items"));
        assert!(joined.contains("ws_abcdef12.call_sites"));
        assert!(joined.contains("CREATE TABLE IF NOT EXISTS"));
        assert!(joined.contains("CREATE INDEX IF NOT EXISTS"));
    }

    #[test]
    fn workspace_ddl_does_not_leak_other_schemas() {
        let stmts = workspace_ddl_statements("ws_abcdef12");
        let joined = stmts.join("\n");
        assert!(!joined.contains("public."));
    }
}
