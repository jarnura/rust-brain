//! WorkspaceManager — entry point for workspace operations.
//!
//! Holds the Postgres pool and (eventually) a DockerClient handle.
//! All workspace lifecycle operations are delegated to `lifecycle` and `schema`.

use sqlx::PgPool;

/// Owns the connection pool for workspace operations.
///
/// Intended to be stored in [`crate::state::AppState`] and cloned cheaply
/// via the inner `Arc<PgPool>`.
#[derive(Clone)]
pub struct WorkspaceManager {
    /// Postgres connection pool shared with the rest of the API.
    pub pool: PgPool,
    // DockerClient will be added here once DevOps Lead delivers it (RUSA-48).
}

impl WorkspaceManager {
    /// Create a new WorkspaceManager with the given Postgres pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
