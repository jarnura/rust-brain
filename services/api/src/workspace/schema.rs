//! Per-workspace Postgres schema management.
//!
//! Each workspace gets its own schema named `ws_<short_id>` (first 12 chars of
//! the workspace UUID without dashes). This schema contains the same table
//! structure as the main rustbrain schema, scoped to the workspace's codebase.

use anyhow::Context;
use sqlx::PgPool;

/// Derive the schema name for a workspace from its UUID.
///
/// Takes the first 12 hex characters of the UUID (without dashes) to form
/// a short, stable identifier.
///
/// # Example
/// ```
/// use rustbrain_api::workspace::schema::schema_name_for;
/// let name = schema_name_for("550e8400-e29b-41d4-a716-446655440000");
/// assert_eq!(name, "ws_550e8400e29b");
/// ```
pub fn schema_name_for(workspace_id: &str) -> String {
    let hex = workspace_id.replace('-', "");
    let short = &hex[..12.min(hex.len())];
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
/// `schema_name` is validated to match `ws_[0-9a-f]{12}` before use. Invalid
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

/// Drop a Postgres schema for the given workspace.
///
/// Executes `DROP SCHEMA IF EXISTS <schema_name> CASCADE`. This removes all
/// tables and data within the schema. Uses raw SQL strings (not compile-time
/// checked) because the schema name is dynamic.
///
/// # Security
///
/// `schema_name` is validated to match `ws_[0-9a-f]{12}` before use. Invalid
/// names return `Err` without executing any SQL.
pub async fn drop_workspace_schema(pool: &PgPool, schema_name: &str) -> anyhow::Result<()> {
    validate_schema_name(schema_name)?;

    let drop_sql = format!("DROP SCHEMA IF EXISTS {schema_name} CASCADE");
    sqlx::query(&drop_sql)
        .execute(pool)
        .await
        .with_context(|| format!("Failed to drop schema '{schema_name}'"))?;

    Ok(())
}

/// Validate that a schema name matches `ws_[0-9a-f]{12}`.
fn validate_schema_name(name: &str) -> anyhow::Result<()> {
    if !name.starts_with("ws_") {
        anyhow::bail!("Invalid schema name '{name}': must start with 'ws_'");
    }
    let suffix = &name[3..];
    if suffix.len() != 12 || !suffix.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!(
            "Invalid schema name '{name}': suffix must be 12 hex characters, got '{suffix}'"
        );
    }
    Ok(())
}

/// Generate individual DDL statements for the rustbrain intelligence tables.
///
/// These must match the `public` schema exactly (column names, types, defaults)
/// so the ingestion pipeline can write into workspace-scoped schemas without
/// query mismatches.
fn workspace_ddl_statements(schema_name: &str) -> Vec<String> {
    vec![
        // -- source_files --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.source_files (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    crate_name TEXT NOT NULL,
    module_path TEXT NOT NULL,
    file_path TEXT NOT NULL,
    original_source TEXT NOT NULL,
    expanded_source TEXT,
    git_hash TEXT,
    git_blame JSONB,
    last_indexed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    repository_id UUID,
    content_hash TEXT,
    UNIQUE (crate_name, module_path, file_path)
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_sf_path ON {schema_name}.source_files(file_path)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_sf_crate ON {schema_name}.source_files(crate_name)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_sf_module ON {schema_name}.source_files(crate_name, module_path)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_sf_hash ON {schema_name}.source_files(content_hash)"
        ),
        // -- extracted_items --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.extracted_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_file_id UUID REFERENCES {schema_name}.source_files(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK (item_type IN ('function','struct','enum','trait','impl','type_alias','const','static','macro','module','use','extern_block')),
    fqn TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'private',
    signature TEXT,
    doc_comment TEXT,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    body_source TEXT,
    generic_params JSONB DEFAULT '[]'::jsonb,
    where_clauses JSONB DEFAULT '[]'::jsonb,
    attributes JSONB DEFAULT '[]'::jsonb,
    generated_by TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_fqn ON {schema_name}.extracted_items(fqn)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_name ON {schema_name}.extracted_items(name)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_type ON {schema_name}.extracted_items(item_type)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_source ON {schema_name}.extracted_items(source_file_id)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_crate ON {schema_name}.extracted_items(fqn text_pattern_ops)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_gen ON {schema_name}.extracted_items(generated_by)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_attrs ON {schema_name}.extracted_items USING GIN (attributes)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ei_generics ON {schema_name}.extracted_items USING GIN (generic_params)"
        ),
        // -- call_sites --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.call_sites (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    caller_fqn TEXT NOT NULL,
    callee_fqn TEXT NOT NULL,
    file_path TEXT NOT NULL,
    line_number INTEGER NOT NULL,
    concrete_type_args JSONB DEFAULT '[]'::jsonb,
    is_monomorphized BOOLEAN DEFAULT FALSE,
    quality TEXT NOT NULL DEFAULT 'heuristic' CHECK (quality IN ('analyzed','heuristic')),
    created_at TIMESTAMPTZ DEFAULT NOW()
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_cs_caller ON {schema_name}.call_sites(caller_fqn)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_cs_callee ON {schema_name}.call_sites(callee_fqn)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_cs_file ON {schema_name}.call_sites(file_path)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_cs_types ON {schema_name}.call_sites USING GIN (concrete_type_args)"
        ),
        // -- trait_implementations --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.trait_implementations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    trait_fqn TEXT NOT NULL,
    self_type TEXT NOT NULL,
    impl_fqn TEXT NOT NULL UNIQUE,
    file_path TEXT NOT NULL,
    line_number INTEGER NOT NULL,
    generic_params JSONB DEFAULT '[]'::jsonb,
    quality TEXT NOT NULL DEFAULT 'heuristic' CHECK (quality IN ('analyzed','heuristic')),
    created_at TIMESTAMPTZ DEFAULT NOW()
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ti_trait ON {schema_name}.trait_implementations(trait_fqn)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ti_type ON {schema_name}.trait_implementations(self_type)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_ti_file ON {schema_name}.trait_implementations(file_path)"
        ),
        // -- ingestion_runs --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.ingestion_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','completed','failed','partial')),
    crates_processed INTEGER DEFAULT 0,
    items_extracted INTEGER DEFAULT 0,
    errors JSONB DEFAULT '[]'::jsonb,
    metadata JSONB DEFAULT '{{}}'::jsonb
)"#
        ),
        // -- pipeline_checkpoints --
        format!(
            r#"CREATE TABLE IF NOT EXISTS {schema_name}.pipeline_checkpoints (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id UUID NOT NULL,
    last_stage TEXT NOT NULL,
    files_processed INTEGER NOT NULL DEFAULT 0,
    tier TEXT NOT NULL DEFAULT 'full',
    completed_files JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
)"#
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{schema_name}_pc_run ON {schema_name}.pipeline_checkpoints(run_id, created_at DESC)"
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_for_derives_correct_prefix() {
        let name = schema_name_for("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(name, "ws_550e8400e29b");
    }

    #[test]
    fn schema_name_for_handles_different_ids() {
        let name = schema_name_for("abcdef12-3456-7890-abcd-ef1234567890");
        assert_eq!(name, "ws_abcdef123456");
    }

    #[test]
    fn validate_schema_name_accepts_valid() {
        assert!(validate_schema_name("ws_abcdef123456").is_ok());
        assert!(validate_schema_name("ws_000000000000").is_ok());
        assert!(validate_schema_name("ws_ffffffffffff").is_ok());
    }

    #[test]
    fn validate_schema_name_rejects_missing_prefix() {
        assert!(validate_schema_name("abcdef123456").is_err());
        assert!(validate_schema_name("workspace_abc").is_err());
    }

    #[test]
    fn validate_schema_name_rejects_wrong_suffix_length() {
        assert!(validate_schema_name("ws_abc").is_err());
        assert!(validate_schema_name("ws_abcdef123").is_err());
        assert!(validate_schema_name("ws_abcdef1234567").is_err());
    }

    #[test]
    fn validate_schema_name_rejects_non_hex_suffix() {
        assert!(validate_schema_name("ws_xyz012345678").is_err());
        assert!(validate_schema_name("ws_!@#$%^&*()").is_err());
    }

    #[test]
    fn workspace_ddl_contains_all_tables() {
        let stmts = workspace_ddl_statements("ws_abcdef123456");
        let joined = stmts.join("\n");
        assert!(joined.contains("ws_abcdef123456.source_files"));
        assert!(joined.contains("ws_abcdef123456.extracted_items"));
        assert!(joined.contains("ws_abcdef123456.call_sites"));
        assert!(joined.contains("ws_abcdef123456.trait_implementations"));
        assert!(joined.contains("ws_abcdef123456.ingestion_runs"));
        assert!(joined.contains("ws_abcdef123456.pipeline_checkpoints"));
        assert!(joined.contains("CREATE TABLE IF NOT EXISTS"));
        assert!(joined.contains("CREATE INDEX IF NOT EXISTS"));
        // Verify key columns match the main schema
        assert!(joined.contains("start_line"));
        assert!(joined.contains("end_line"));
        assert!(joined.contains("body_source"));
        assert!(joined.contains("module_path"));
        assert!(joined.contains("crate_name"));
    }

    #[test]
    fn workspace_ddl_does_not_leak_other_schemas() {
        let stmts = workspace_ddl_statements("ws_abcdef123456");
        let joined = stmts.join("\n");
        assert!(!joined.contains("public."));
    }
}
