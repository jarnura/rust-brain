//! Configuration loaded from environment variables.
//!
//! All secrets come from the environment — never hardcoded.

use std::env;

/// Audit service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres connection URL.
    pub database_url: String,
    /// Neo4j Bolt protocol URL.
    pub neo4j_url: String,
    /// Neo4j username.
    pub neo4j_user: String,
    /// Neo4j password.
    pub neo4j_password: String,
    /// HTTP port for /health and /metrics endpoints.
    pub audit_port: u16,
    /// Interval between leak detection scans in seconds.
    pub audit_interval_secs: u32,
    /// If true, only report orphans without removing them.
    pub dry_run: bool,
    /// Auto-delete workspace_audit_log entries older than N days.
    pub log_retention_days: u32,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Panics if required variables are missing.
    pub fn from_env() -> Self {
        let database_url =
            env::var("DATABASE_URL").expect("DATABASE_URL environment variable is required");
        let neo4j_url = env::var("NEO4J_URL").expect("NEO4J_URL environment variable is required");
        let neo4j_user = env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string());
        let neo4j_password =
            env::var("NEO4J_PASSWORD").expect("NEO4J_PASSWORD environment variable is required");
        let audit_port = env::var("AUDIT_PORT")
            .unwrap_or_else(|_| "8090".to_string())
            .parse::<u16>()
            .expect("AUDIT_PORT must be a valid port number");
        let audit_interval_secs = env::var("AUDIT_INTERVAL_SECS")
            .unwrap_or_else(|_| "600".to_string())
            .parse::<u32>()
            .expect("AUDIT_INTERVAL_SECS must be a valid number");
        let dry_run = env::var("LEAK_DETECTION_DRY_RUN")
            .unwrap_or_else(|_| "true".to_string())
            .parse::<bool>()
            .expect("LEAK_DETECTION_DRY_RUN must be true or false");
        let log_retention_days = env::var("AUDIT_LOG_RETENTION_DAYS")
            .unwrap_or_else(|_| "90".to_string())
            .parse::<u32>()
            .expect("AUDIT_LOG_RETENTION_DAYS must be a valid number");

        Self {
            database_url,
            neo4j_url,
            neo4j_user,
            neo4j_password,
            audit_port,
            audit_interval_secs,
            dry_run,
            log_retention_days,
        }
    }

    /// Returns the database URL with the password redacted.
    pub fn redacted_database_url(&self) -> String {
        redact_url(&self.database_url)
    }

    /// Returns the Neo4j URL with the password redacted.
    pub fn redacted_neo4j_url(&self) -> String {
        redact_url(&self.neo4j_url)
    }
}

/// Replaces the password in a URL with `***` for safe logging.
fn redact_url(url: &str) -> String {
    // Handle URLs like postgresql://user:password@host/db
    // and bolt://user:password@host
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            // Check that the colon is after the scheme's ://
            let scheme_end = url.find("://").map(|p| p + 3).unwrap_or(0);
            if colon_pos >= scheme_end {
                return format!("{}{}@{}", &url[..=colon_pos], "***", &url[at_pos + 1..]);
            }
        }
    }
    url.to_string()
}
