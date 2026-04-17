//! Apache AGE openCypher POC module
//!
//! Ports the 10 core Neo4j Cypher queries to AGE openCypher running on PostgreSQL.
//! Uses raw sqlx queries — no abstraction layer (per CEO mandate for POC scope).
//!
//! # Key Difference: ON CREATE SET
//!
//! AGE openCypher does NOT support `ON CREATE SET` / `ON MATCH SET` clauses in MERGE.
//! The workaround uses COALESCE to preserve existing properties on match while setting
//! defaults on create:
//!
//! ```cypher
//! -- Neo4j:
//! MERGE (n:Label {id: $id}) ON CREATE SET n.fqn = $fqn
//!
//! -- AGE workaround:
//! MERGE (n:Label {id: $id}) SET n.fqn = coalesce(n.fqn, $fqn)
//! ```
//!
//! # Query Format
//!
//! All AGE openCypher queries use the SQL wrapper pattern:
//! ```sql
//! SELECT * FROM cypher('graph_name', $$ <cypher_query> $$, $1::agtype)
//!   AS (result agtype);
//! ```

pub mod bench;
pub mod queries;
pub mod types;

use anyhow::{Context, Result};
use sqlx::PgPool;
use std::env;
use tracing::{debug, info};

/// Default AGE database URL (separate from main Postgres to avoid conflicts)
pub const DEFAULT_AGE_DATABASE_URL: &str =
    "postgres://rustbrain:password@localhost:5433/rustbrain_age";

/// Default graph name for AGE
pub const DEFAULT_AGE_GRAPH_NAME: &str = "rustbrain";

/// Default batch size for AGE operations (configurable via env)
pub const DEFAULT_AGE_BATCH_SIZE: usize = 1000;

/// Configuration for AGE POC operations
#[derive(Debug, Clone)]
pub struct AgeConfig {
    /// PostgreSQL connection URL for AGE-enabled database
    pub database_url: String,
    /// Name of the AGE graph
    pub graph_name: String,
    /// Batch size for bulk operations
    pub batch_size: usize,
}

impl Default for AgeConfig {
    fn default() -> Self {
        Self {
            database_url: env::var("AGE_DATABASE_URL")
                .unwrap_or_else(|_| DEFAULT_AGE_DATABASE_URL.to_string()),
            graph_name: env::var("AGE_GRAPH_NAME")
                .unwrap_or_else(|_| DEFAULT_AGE_GRAPH_NAME.to_string()),
            batch_size: env::var("AGE_BATCH_SIZE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_AGE_BATCH_SIZE),
        }
    }
}

/// Create a connection pool to the AGE-enabled PostgreSQL database.
///
/// Follows the same pattern as `ExtractStage::connect()` in the existing codebase.
/// Initializes the AGE extension and creates the graph if it doesn't exist.
pub async fn create_age_pool(config: &AgeConfig) -> Result<PgPool> {
    info!(
        "Connecting to AGE PostgreSQL at {}",
        redact_url(&config.database_url)
    );

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .context("Failed to connect to AGE PostgreSQL")?;

    sqlx::query("CREATE EXTENSION IF NOT EXISTS age")
        .execute(&pool)
        .await
        .context("Failed to create AGE extension")?;

    sqlx::query("SET search_path = ag_catalog, \"$user\", public")
        .execute(&pool)
        .await
        .context("Failed to set search_path for AGE")?;
    let graph_name = &config.graph_name;
    let check_graph = sqlx::query_scalar::<_, i64>(
        "SELECT count(*)::bigint FROM ag_catalog.ag_graph WHERE name = $1",
    )
    .bind(graph_name)
    .fetch_one(&pool)
    .await
    .context("Failed to check if AGE graph exists")?;

    if check_graph == 0 {
        let create_query = format!("SELECT create_graph('{}')", graph_name);
        sqlx::query(&create_query)
            .execute(&pool)
            .await
            .context("Failed to create AGE graph")?;
        info!("Created AGE graph: {}", graph_name);
    } else {
        debug!("AGE graph already exists: {}", graph_name);
    }

    info!("Successfully connected to AGE PostgreSQL");
    Ok(pool)
}

/// Redact password from database/connection URLs for safe logging
fn redact_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let scheme_and_user = &url[..colon_pos + 1];
            let rest = &url[at_pos..];
            format!("{}***{}", scheme_and_user, rest)
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_age_config_defaults() {
        let config = AgeConfig::default();
        assert!(!config.database_url.is_empty());
        assert_eq!(config.graph_name, "rustbrain");
        assert_eq!(config.batch_size, 1000);
    }

    #[test]
    fn test_redact_url() {
        let url = "postgres://user:secret@localhost:5432/db";
        let redacted = redact_url(url);
        assert_eq!(redacted, "postgres://user:***@localhost:5432/db");
        assert!(!redacted.contains("secret"));
    }

    #[test]
    fn test_redact_url_no_password() {
        let url = "postgres://localhost:5432/db";
        let redacted = redact_url(url);
        assert_eq!(redacted, url);
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_create_age_pool() {
        let config = AgeConfig::default();
        let pool = create_age_pool(&config).await.unwrap();
        let result: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(result, 1);
    }
}
