//! Workspace REST handlers.
//!
//! Endpoints:
//!
//! | Method | Path | Handler | Description |
//! |--------|------|---------|-------------|
//! | POST | `/workspaces` | [`create_workspace`] | Create + async clone |
//! | GET | `/workspaces` | [`list_workspaces`] | List all non-archived |
//! | GET | `/workspaces/:id` | [`get_workspace`] | Fetch by UUID |
//! | DELETE | `/workspaces/:id` | [`delete_workspace`] | Archive + cleanup |
//! | GET | `/workspaces/:id/files` | [`list_files`] | Directory tree |

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::docker::{DockerClient, IngestionConfig};
use crate::errors::AppError;
use crate::github::GithubClient;
use crate::state::AppState;
use crate::workspace::{
    create_workspace as db_create_workspace, get_workspace as db_get_workspace, lifecycle,
    list_workspaces as db_list_workspaces, CreateWorkspaceParams, Workspace, WorkspaceSourceType,
    WorkspaceStatus,
};

// =============================================================================
// Request / Response types
// =============================================================================

/// Body for `POST /workspaces`.
#[derive(Debug, Deserialize)]
pub struct CreateWorkspaceRequest {
    /// GitHub HTTPS URL (e.g. `https://github.com/org/repo`).
    pub github_url: String,
    /// Optional human-readable name. Defaults to the repo slug.
    pub name: Option<String>,
}

/// Response body for workspace creation (`202 Accepted`).
#[derive(Debug, Serialize)]
pub struct CreateWorkspaceResponse {
    /// UUID of the newly created workspace.
    pub id: Uuid,
    /// Initial status — always `"cloning"`.
    pub status: String,
    /// Message describing what happens next.
    pub message: String,
}

/// A single entry in a workspace file tree, compatible with react-treeview.
#[derive(Debug, Serialize)]
pub struct FileNode {
    /// Entry name (file or directory name, not the full path).
    pub name: String,
    /// Full path relative to the workspace clone root.
    pub path: String,
    /// `"file"` or `"directory"`.
    #[serde(rename = "type")]
    pub node_type: String,
    /// Child nodes (omitted for files).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<FileNode>,
}

// =============================================================================
// Validation helpers
// =============================================================================

/// Validate that `url` is a GitHub HTTPS URL.
///
/// Accepts `https://github.com/<owner>/<repo>` (with or without `.git` suffix).
fn validate_github_url(url: &str) -> Result<(), AppError> {
    if !url.starts_with("https://github.com/") {
        return Err(AppError::BadRequest(
            "github_url must start with https://github.com/".to_string(),
        ));
    }
    let path = url
        .trim_start_matches("https://github.com/")
        .trim_end_matches(".git");
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return Err(AppError::BadRequest(
            "github_url must be in the form https://github.com/<owner>/<repo>".to_string(),
        ));
    }
    Ok(())
}

/// Derive a repo slug from a GitHub URL (last path segment without `.git`).
fn repo_slug(url: &str) -> String {
    let clean = url
        .trim_start_matches("https://github.com/")
        .trim_end_matches(".git");
    clean
        .split('/')
        .rfind(|s| !s.is_empty())
        .unwrap_or(clean)
        .to_string()
}

/// Generate a short unique schema name from a UUID (first 12 hex chars).
fn schema_name_from_id(id: Uuid) -> String {
    let hex = id.to_string().replace('-', "");
    format!("ws_{}", &hex[..12])
}

// =============================================================================
// Handlers
// =============================================================================

/// `POST /workspaces` — create a workspace and kick off async clone.
///
/// Returns `202 Accepted` immediately. Poll `GET /workspaces/:id` for progress.
pub async fn create_workspace(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, AppError> {
    state.metrics.record_request("create_workspace", "POST");

    validate_github_url(&req.github_url)?;

    let name = req
        .name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| repo_slug(&req.github_url));

    // Detect auth method from env so it's stored on the record
    let client = GithubClient::from_env();
    let auth_method_str = match client.auth_method {
        crate::github::GithubAuthMethod::Pat => Some("pat".to_string()),
        crate::github::GithubAuthMethod::App => Some("app".to_string()),
        crate::github::GithubAuthMethod::None => None,
    };

    // We need a UUID upfront to compute the schema name before inserting
    let id = Uuid::new_v4();
    let schema_name = schema_name_from_id(id);

    let params = CreateWorkspaceParams {
        name,
        source_type: WorkspaceSourceType::Github,
        source_url: req.github_url.clone(),
        schema_name,
        volume_name: None,
        github_auth_method: auth_method_str,
    };

    let workspace = db_create_workspace(&state.workspace_manager.pool, params)
        .await
        .map_err(|e| AppError::Database(format!("Failed to create workspace: {}", e)))?;

    info!(
        workspace_id = %workspace.id,
        source_url = %workspace.source_url,
        "Workspace created — spawning background clone"
    );

    // Spawn background clone task. Handler returns 202 immediately.
    let pool = state.workspace_manager.pool.clone();
    let ws_id = workspace.id;
    let source_url = workspace.source_url.clone();
    let schema_name = workspace
        .schema_name
        .clone()
        .unwrap_or_else(|| schema_name_from_id(ws_id));
    let docker = state.docker.clone();
    let config = state.config.clone();

    tokio::spawn(async move {
        run_clone(pool, client, docker, config, ws_id, schema_name, source_url).await;
    });

    let body = CreateWorkspaceResponse {
        id: workspace.id,
        status: WorkspaceStatus::Cloning.as_str().to_string(),
        message: "Workspace created. Clone started in the background.".to_string(),
    };

    Ok((StatusCode::ACCEPTED, Json(body)))
}

/// `GET /workspaces` — list all non-archived workspaces.
pub async fn list_workspaces(
    State(state): State<AppState>,
) -> Result<Json<Vec<Workspace>>, AppError> {
    state.metrics.record_request("list_workspaces", "GET");

    let workspaces = db_list_workspaces(&state.workspace_manager.pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to list workspaces: {}", e)))?;

    Ok(Json(workspaces))
}

/// `GET /workspaces/:id` — fetch a workspace by UUID.
pub async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Workspace>, AppError> {
    state.metrics.record_request("get_workspace", "GET");

    let workspace = db_get_workspace(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    Ok(Json(workspace))
}

/// `DELETE /workspaces/:id` — archive a workspace and clean up clone directory.
///
/// Returns `204 No Content` on success, `404` if not found.
pub async fn delete_workspace(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    state.metrics.record_request("delete_workspace", "DELETE");

    let pool = &state.workspace_manager.pool;

    let workspace = db_get_workspace(pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    let result = sqlx::query::<sqlx::Postgres>(
        "UPDATE workspaces SET status = 'archived', updated_at = NOW() \
         WHERE id = $1 AND status != 'archived'",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to archive workspace: {}", e)))?;

    if result.rows_affected() > 0 {
        info!(
            workspace_id = %id,
            clone_path = ?workspace.clone_path,
            "Workspace archived — triggering clone dir cleanup"
        );
        if let Some(clone_path) = workspace.clone_path {
            tokio::spawn(async move {
                if let Err(e) = tokio::fs::remove_dir_all(&clone_path).await {
                    warn!("Failed to remove clone dir {}: {}", clone_path, e);
                }
            });
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /workspaces/:id/files` — return a directory tree of the cloned repo.
///
/// Returns `400` if not yet cloned, `404` if workspace doesn't exist.
pub async fn list_files(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FileNode>, AppError> {
    state.metrics.record_request("list_workspace_files", "GET");

    let workspace = db_get_workspace(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    let clone_path = workspace.clone_path.ok_or_else(|| {
        AppError::BadRequest(format!(
            "Workspace {} is not yet cloned (status: {})",
            id, workspace.status
        ))
    })?;

    let root_path = std::path::PathBuf::from(clone_path);
    if !root_path.exists() {
        return Err(AppError::Internal(format!(
            "Clone path does not exist on disk for workspace {}",
            id
        )));
    }

    let tree = tokio::task::spawn_blocking(move || build_tree(&root_path, &root_path))
        .await
        .map_err(|e| AppError::Internal(format!("File tree task panicked: {}", e)))?
        .map_err(|e| AppError::Internal(format!("Failed to build file tree: {}", e)))?;

    Ok(Json(tree))
}

// =============================================================================
// Background clone task
// =============================================================================

/// Clone the repository and then run the ingestion pipeline.
///
/// Status transitions:
/// - Clone success: `cloning` → (sets clone_path) → `indexing`
/// - Ingestion success: `indexing` → `ready`
/// - Clone failure: `cloning` → `error`
/// - Ingestion failure: `indexing` → `error`
///
/// `schema_name` is appended to `DATABASE_URL` as `search_path` so the
/// ingestion pipeline writes extracted items into the workspace-scoped
/// Postgres schema rather than the default schema.
async fn run_clone(
    pool: sqlx::postgres::PgPool,
    client: GithubClient,
    docker: DockerClient,
    config: Config,
    ws_id: Uuid,
    schema_name: String,
    source_url: String,
) {
    // The clone directory is inside the container at this path.
    // The host-side equivalent is config.workspace_host_clone_root/<ws_id>.
    let container_clone_dir = format!("/tmp/rustbrain-clones/{}", ws_id);
    let host_clone_dir = format!("{}/{}", config.workspace_host_clone_root, ws_id);
    let clone_path = std::path::Path::new(&container_clone_dir);

    // --- Stage 1: Clone ---
    match client.clone_repo(&source_url, clone_path).await {
        Ok(branch) => {
            info!(
                workspace_id = %ws_id,
                default_branch = %branch,
                clone_path = %container_clone_dir,
                "Clone complete — advancing to indexing"
            );
            if let Err(e) = lifecycle::clone_workspace(&pool, ws_id, &container_clone_dir).await {
                warn!("Failed to set clone_path for workspace {}: {}", ws_id, e);
            }
            if let Err(e) = lifecycle::start_indexing(&pool, ws_id).await {
                warn!("Failed to advance workspace {} to indexing: {}", ws_id, e);
                return;
            }
        }
        Err(e) => {
            warn!("Clone failed for workspace {}: {}", ws_id, e);
            if let Err(e2) = lifecycle::fail(&pool, ws_id, &e.to_string()).await {
                warn!(
                    "Failed to mark workspace {} as error after clone failure: {}",
                    ws_id, e2
                );
            }
            return;
        }
    }

    // --- Stage 2: Ingest ---
    // Append search_path to the DATABASE_URL so the ingestion pipeline
    // writes into the workspace-scoped schema rather than the default schema.
    let db_url_with_schema = append_search_path(&config.database_url, &schema_name);

    info!(
        workspace_id = %ws_id,
        schema = %schema_name,
        host_clone_dir = %host_clone_dir,
        "Starting ingestion pipeline"
    );

    let ingestion_cfg = IngestionConfig {
        host_clone_path: &host_clone_dir,
        network: &config.docker_network,
        database_url: &db_url_with_schema,
        neo4j_url: &config.neo4j_uri,
        neo4j_user: &config.neo4j_user,
        neo4j_password: &config.neo4j_password,
        ollama_host: &config.ollama_host,
        qdrant_host: &config.qdrant_host,
        embedding_model: &config.embedding_model,
        ingestion_image: &config.ingestion_image,
    };

    match docker.run_ingestion(&ingestion_cfg).await {
        Ok(output) => {
            info!(
                workspace_id = %ws_id,
                "Ingestion complete — marking workspace ready"
            );
            if !output.is_empty() {
                info!(workspace_id = %ws_id, "Ingestion output: {}", output.trim_end());
            }
            if let Err(e) = lifecycle::mark_ready(&pool, ws_id).await {
                error!(
                    "Failed to mark workspace {} as ready after successful ingestion: {}",
                    ws_id, e
                );
            }
        }
        Err(e) => {
            error!("Ingestion failed for workspace {}: {}", ws_id, e);
            if let Err(e2) = lifecycle::fail(&pool, ws_id, &e.to_string()).await {
                error!(
                    "Failed to mark workspace {} as error after ingestion failure: {}",
                    ws_id, e2
                );
            }
        }
    }
}

/// Append `?options=--search_path%3D<schema>,public` to a Postgres URL.
///
/// If the URL already contains a query string, the `options` parameter is
/// appended with `&`. Handles both plain URLs and those with existing params.
fn append_search_path(database_url: &str, schema_name: &str) -> String {
    // Encode the value: --search_path=<schema>,public
    // The `%3D` encodes `=`, which is required inside the options value.
    let options = format!("--search_path%3D{},public", schema_name);
    let param = format!("options={}", options);

    if database_url.contains('?') {
        format!("{}&{}", database_url, param)
    } else {
        format!("{}?{}", database_url, param)
    }
}

// =============================================================================
// File tree builder
// =============================================================================

/// Recursively build a [`FileNode`] tree rooted at `path`.
///
/// Hidden entries (names starting with `.`) are skipped.
/// Directories sort before files at each level.
fn build_tree(path: &std::path::Path, root: &std::path::Path) -> std::io::Result<FileNode> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    let rel_path = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| name.clone());

    if path.is_file() {
        return Ok(FileNode {
            name,
            path: rel_path,
            node_type: "file".to_string(),
            children: vec![],
        });
    }

    let mut entries: Vec<_> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| !n.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();

    // Directories before files, both sorted by name
    entries.sort_by_key(|e| {
        let is_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        (is_file as u8, e.file_name())
    });

    let mut children = vec![];
    for entry in entries {
        children.push(build_tree(&entry.path(), root)?);
    }

    Ok(FileNode {
        name,
        path: rel_path,
        node_type: "directory".to_string(),
        children,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_append_search_path_no_existing_query() {
        let url = "postgresql://user:pass@host:5432/db";
        let result = append_search_path(url, "ws_abc12345");
        assert_eq!(
            result,
            "postgresql://user:pass@host:5432/db?options=--search_path%3Dws_abc12345,public"
        );
    }

    #[test]
    fn test_append_search_path_existing_query() {
        let url = "postgresql://user:pass@host:5432/db?sslmode=require";
        let result = append_search_path(url, "ws_abc12345");
        assert_eq!(
            result,
            "postgresql://user:pass@host:5432/db?sslmode=require&options=--search_path%3Dws_abc12345,public"
        );
    }

    #[test]
    fn test_append_search_path_preserves_base_url() {
        let url = "postgresql://localhost/mydb";
        let result = append_search_path(url, "ws_00000000");
        assert!(result.starts_with("postgresql://localhost/mydb?"));
        assert!(result.contains("ws_00000000"));
        assert!(result.contains("public"));
    }

    #[test]
    fn test_validate_github_url_valid() {
        assert!(validate_github_url("https://github.com/rust-lang/rust").is_ok());
        assert!(validate_github_url("https://github.com/org/repo.git").is_ok());
        assert!(validate_github_url("https://github.com/a/b").is_ok());
    }

    #[test]
    fn test_validate_github_url_invalid() {
        assert!(validate_github_url("http://github.com/org/repo").is_err());
        assert!(validate_github_url("https://gitlab.com/org/repo").is_err());
        assert!(validate_github_url("https://github.com/onlyone").is_err());
        assert!(validate_github_url("https://github.com/").is_err());
        assert!(validate_github_url("not-a-url").is_err());
        assert!(validate_github_url("").is_err());
    }

    #[test]
    fn test_repo_slug() {
        assert_eq!(repo_slug("https://github.com/org/my-repo"), "my-repo");
        assert_eq!(repo_slug("https://github.com/org/my-repo.git"), "my-repo");
        assert_eq!(repo_slug("https://github.com/rust-lang/rust"), "rust");
    }

    #[test]
    fn test_schema_name_format() {
        let id = Uuid::new_v4();
        let name = schema_name_from_id(id);
        assert!(name.starts_with("ws_"));
        assert_eq!(name.len(), 15); // "ws_" + 12 hex chars
    }

    #[test]
    fn test_build_tree_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();

        let tree = build_tree(tmp.path(), tmp.path()).unwrap();
        assert_eq!(tree.node_type, "directory");
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].name, "main.rs");
        assert_eq!(tree.children[0].node_type, "file");
        assert_eq!(tree.children[0].path, "main.rs");
    }

    #[test]
    fn test_build_tree_nested() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src").join("lib.rs"), "").unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();

        let tree = build_tree(tmp.path(), tmp.path()).unwrap();
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();

        // Directories before files
        let src_idx = names.iter().position(|&n| n == "src").unwrap();
        let toml_idx = names.iter().position(|&n| n == "Cargo.toml").unwrap();
        assert!(src_idx < toml_idx, "directories should appear before files");

        let src = tree.children.iter().find(|c| c.name == "src").unwrap();
        assert_eq!(src.children[0].path, "src/lib.rs");
    }

    #[test]
    fn test_build_tree_skips_hidden() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "").unwrap();
        std::fs::write(tmp.path().join("lib.rs"), "").unwrap();

        let tree = build_tree(tmp.path(), tmp.path()).unwrap();
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert!(!names.contains(&".git"));
        assert!(!names.contains(&".gitignore"));
        assert!(names.contains(&"lib.rs"));
    }

    #[test]
    fn test_file_node_serde_omits_empty_children() {
        let node = FileNode {
            name: "main.rs".to_string(),
            path: "main.rs".to_string(),
            node_type: "file".to_string(),
            children: vec![],
        };
        let json = serde_json::to_value(&node).unwrap();
        // children must be absent for files (skip_serializing_if)
        assert!(json.get("children").is_none());
    }

    #[test]
    fn test_file_node_serde_includes_children_for_dirs() {
        let node = FileNode {
            name: "src".to_string(),
            path: "src".to_string(),
            node_type: "directory".to_string(),
            children: vec![FileNode {
                name: "main.rs".to_string(),
                path: "src/main.rs".to_string(),
                node_type: "file".to_string(),
                children: vec![],
            }],
        };
        let json = serde_json::to_value(&node).unwrap();
        assert_eq!(json["children"][0]["name"], "main.rs");
    }

    #[test]
    fn test_create_workspace_request_deserializes() {
        let json = r#"{"github_url": "https://github.com/org/repo", "name": "my-ws"}"#;
        let req: CreateWorkspaceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.github_url, "https://github.com/org/repo");
        assert_eq!(req.name.as_deref(), Some("my-ws"));
    }

    #[test]
    fn test_create_workspace_request_name_optional() {
        let json = r#"{"github_url": "https://github.com/org/repo"}"#;
        let req: CreateWorkspaceRequest = serde_json::from_str(json).unwrap();
        assert!(req.name.is_none());
    }
}
