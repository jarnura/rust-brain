//! Workspace struct, status enum, and Postgres CRUD operations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Lifecycle status of a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    /// Workspace record created; clone not yet started.
    Pending,
    /// Repository is being cloned.
    Cloning,
    /// Codebase is being indexed by the ingestion pipeline.
    Indexing,
    /// Workspace is fully indexed and ready for queries.
    Ready,
    /// An unrecoverable error occurred.
    Error,
    /// Workspace has been archived and is no longer active.
    Archived,
}

impl WorkspaceStatus {
    /// String representation stored in Postgres.
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceStatus::Pending => "pending",
            WorkspaceStatus::Cloning => "cloning",
            WorkspaceStatus::Indexing => "indexing",
            WorkspaceStatus::Ready => "ready",
            WorkspaceStatus::Error => "error",
            WorkspaceStatus::Archived => "archived",
        }
    }

    /// Parse from the text representation stored in Postgres.
    pub fn from_str(s: &str) -> Self {
        match s {
            "cloning" => Self::Cloning,
            "indexing" => Self::Indexing,
            "ready" => Self::Ready,
            "error" => Self::Error,
            "archived" => Self::Archived,
            _ => Self::Pending,
        }
    }
}

impl std::fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Source type for a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSourceType {
    Github,
    Local,
}

impl WorkspaceSourceType {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceSourceType::Github => "github",
            WorkspaceSourceType::Local => "local",
        }
    }
}

/// A workspace represents a cloned and indexed Rust codebase.
///
/// Each workspace has its own Postgres schema (`ws_<short_id>`) and an
/// optional Docker volume for the cloned source files.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub source_type: String,
    pub source_url: String,
    pub clone_path: Option<String>,
    pub volume_name: Option<String>,
    /// Postgres schema name, e.g. `ws_abc12345`
    pub schema_name: Option<String>,
    pub status: String,
    pub default_branch: Option<String>,
    pub github_auth_method: Option<String>,
    pub index_started_at: Option<DateTime<Utc>>,
    pub index_completed_at: Option<DateTime<Utc>>,
    pub index_stage: Option<String>,
    pub index_progress: Option<serde_json::Value>,
    pub index_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Parameters for creating a new workspace.
pub struct CreateWorkspaceParams {
    pub name: String,
    pub source_type: WorkspaceSourceType,
    pub source_url: String,
    pub schema_name: String,
    pub volume_name: Option<String>,
    pub github_auth_method: Option<String>,
}

/// Fetch a single workspace by ID.
pub async fn get_workspace(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Workspace>> {
    let row = sqlx::query_as::<_, Workspace>(
        r#"
        SELECT id, name, source_type, source_url, clone_path, volume_name, schema_name,
               status, default_branch, github_auth_method,
               index_started_at, index_completed_at, index_stage, index_progress, index_error,
               created_at, updated_at
        FROM workspaces
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// List all workspaces ordered by creation time (newest first).
pub async fn list_workspaces(pool: &PgPool) -> anyhow::Result<Vec<Workspace>> {
    let rows = sqlx::query_as::<_, Workspace>(
        r#"
        SELECT id, name, source_type, source_url, clone_path, volume_name, schema_name,
               status, default_branch, github_auth_method,
               index_started_at, index_completed_at, index_stage, index_progress, index_error,
               created_at, updated_at
        FROM workspaces
        ORDER BY created_at DESC
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Insert a new workspace row in `cloning` status.
pub async fn create_workspace(
    pool: &PgPool,
    params: CreateWorkspaceParams,
) -> anyhow::Result<Workspace> {
    let row = sqlx::query_as::<_, Workspace>(
        r#"
        INSERT INTO workspaces (
            name, source_type, source_url, schema_name, volume_name, github_auth_method, status
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'cloning')
        RETURNING id, name, source_type, source_url, clone_path, volume_name, schema_name,
                  status, default_branch, github_auth_method,
                  index_started_at, index_completed_at, index_stage, index_progress, index_error,
                  created_at, updated_at
        "#,
    )
    .bind(params.name)
    .bind(params.source_type.as_str())
    .bind(params.source_url)
    .bind(params.schema_name)
    .bind(params.volume_name)
    .bind(params.github_auth_method)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_status_as_str() {
        assert_eq!(WorkspaceStatus::Pending.as_str(), "pending");
        assert_eq!(WorkspaceStatus::Cloning.as_str(), "cloning");
        assert_eq!(WorkspaceStatus::Indexing.as_str(), "indexing");
        assert_eq!(WorkspaceStatus::Ready.as_str(), "ready");
        assert_eq!(WorkspaceStatus::Error.as_str(), "error");
        assert_eq!(WorkspaceStatus::Archived.as_str(), "archived");
    }

    #[test]
    fn workspace_status_display() {
        assert_eq!(WorkspaceStatus::Ready.to_string(), "ready");
        assert_eq!(WorkspaceStatus::Error.to_string(), "error");
    }

    #[test]
    fn workspace_source_type_as_str() {
        assert_eq!(WorkspaceSourceType::Github.as_str(), "github");
        assert_eq!(WorkspaceSourceType::Local.as_str(), "local");
    }

    #[test]
    fn workspace_status_roundtrip() {
        let pairs = [
            ("pending", WorkspaceStatus::Pending),
            ("cloning", WorkspaceStatus::Cloning),
            ("indexing", WorkspaceStatus::Indexing),
            ("ready", WorkspaceStatus::Ready),
            ("error", WorkspaceStatus::Error),
            ("archived", WorkspaceStatus::Archived),
        ];
        for (s, expected) in &pairs {
            let parsed = WorkspaceStatus::from_str(s);
            assert_eq!(&parsed, expected);
            assert_eq!(parsed.as_str(), *s);
        }
    }

    #[test]
    fn workspace_status_all_variants_covered() {
        let statuses = [
            WorkspaceStatus::Pending,
            WorkspaceStatus::Cloning,
            WorkspaceStatus::Indexing,
            WorkspaceStatus::Ready,
            WorkspaceStatus::Error,
            WorkspaceStatus::Archived,
        ];
        for s in &statuses {
            // Must not be empty
            assert!(!s.as_str().is_empty());
        }
    }
}
