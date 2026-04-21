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
    extract::{Extension, Path, State},
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
use crate::execution::models::{
    abort_executions_for_workspace, list_running_executions_for_workspace,
};
use crate::github::GithubClient;
use crate::middleware::auth::{require_write_access, ApiKeyContext};
use crate::state::AppState;
use crate::workspace::{
    create_workspace as db_create_workspace, get_workspace as db_get_workspace, lifecycle,
    list_workspaces as db_list_workspaces, schema::create_workspace_schema,
    schema::drop_workspace_schema, CreateWorkspaceParams, Workspace, WorkspaceSourceType,
    WorkspaceStatus,
};
use neo4rs;

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
    /// `true` for directories, `false` for files.
    pub is_dir: bool,
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
    Extension(ctx): Extension<ApiKeyContext>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<impl IntoResponse, AppError> {
    require_write_access(&ctx)?;
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

    let http_client = state.http_client.clone();

    tokio::spawn(async move {
        run_clone(
            pool,
            client,
            docker,
            config,
            http_client,
            ws_id,
            schema_name,
            source_url,
        )
        .await;
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
    let workspace = db_get_workspace(&state.workspace_manager.pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    Ok(Json(workspace))
}

/// `DELETE /workspaces/:id` — archive a workspace and clean up all resources.
///
/// Cleanup steps (in order):
/// 1. Stop active ingestion containers (SIGTERM, 30s timeout before SIGKILL)
/// 2. Stop any running execution containers
/// 3. Abort running executions in the database
/// 4. Drop the per-workspace Postgres schema
///    4a. Delete the per-workspace Neo4j graph data
///    4b. Delete the per-workspace Qdrant collections
/// 5. Remove the Docker volume
/// 6. Clean up the host clone directory
///
/// Ingestion containers are stopped first to prevent them from retrying writes
/// into Qdrant/Neo4j collections that are about to be deleted.
///
/// Returns `204 No Content` on success, `404` if not found.
pub async fn delete_workspace(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    require_write_access(&ctx)?;
    let pool = &state.workspace_manager.pool;

    let workspace = db_get_workspace(pool, id)
        .await
        .map_err(|e| AppError::Database(format!("Failed to fetch workspace: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Workspace not found: {}", id)))?;

    // Check if already archived
    if workspace.status == "archived" {
        return Ok(StatusCode::NO_CONTENT);
    }

    let result = sqlx::query::<sqlx::Postgres>(
        "UPDATE workspaces SET status = 'archived', updated_at = NOW() \
         WHERE id = $1 AND status != 'archived'",
    )
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to archive workspace: {}", e)))?;

    if result.rows_affected() == 0 {
        return Ok(StatusCode::NO_CONTENT);
    }

    info!(
        workspace_id = %id,
        schema_name = ?workspace.schema_name,
        clone_path = ?workspace.clone_path,
        volume_name = ?workspace.volume_name,
        "Workspace archived — starting cleanup"
    );

    // Clone values for the background task
    let schema_name = workspace.schema_name;
    let clone_path = workspace.clone_path;
    let volume_name = workspace.volume_name;
    let docker = state.docker.clone();
    let pool_clone = pool.clone();
    let http_client = state.http_client.clone();
    let qdrant_host = state.config.qdrant_host.clone();
    let neo4j_graph = state.neo4j_graph.clone();
    let ws_hex: String = id.as_simple().to_string();
    let workspace_label = format!("Workspace_{}", &ws_hex[..12]);

    tokio::spawn(async move {
        // 1. Stop active ingestion containers for this workspace before any data wipe.
        //    Containers are labelled with rustbrain.workspace_id at launch so they can
        //    be discovered without tracking a container name or ID separately.
        let ws_id_str = id.to_string();
        match docker
            .find_ingestion_containers_for_workspace(&ws_id_str)
            .await
        {
            Ok(container_ids) if !container_ids.is_empty() => {
                info!(
                    workspace_id = %id,
                    count = container_ids.len(),
                    "Found active ingestion containers — stopping before data wipe"
                );
                for cid in &container_ids {
                    info!(
                        workspace_id = %id,
                        container_id = %cid,
                        "Gracefully stopping ingestion container (SIGTERM, 30s timeout)"
                    );
                    if let Err(e) = docker.stop_container_graceful(cid, 30).await {
                        warn!(
                            workspace_id = %id,
                            container_id = %cid,
                            error = %e,
                            "Failed to gracefully stop ingestion container — forcing removal"
                        );
                        if let Err(e2) = docker.remove_container(cid).await {
                            warn!(
                                workspace_id = %id,
                                container_id = %cid,
                                error = %e2,
                                "Failed to force-remove ingestion container"
                            );
                        }
                    } else {
                        info!(
                            workspace_id = %id,
                            container_id = %cid,
                            "Ingestion container stopped"
                        );
                    }
                }
            }
            Ok(_) => {
                info!(workspace_id = %id, "No active ingestion containers found");
            }
            Err(e) => {
                warn!(
                    workspace_id = %id,
                    error = %e,
                    "Failed to query for active ingestion containers — proceeding with data wipe"
                );
            }
        }

        // 2. Stop running execution containers
        match list_running_executions_for_workspace(&pool_clone, id).await {
            Ok(running) => {
                for (exec_id, container_id_opt) in running {
                    if let Some(cid) = container_id_opt {
                        info!(
                            workspace_id = %id,
                            execution_id = %exec_id,
                            container_id = %cid,
                            "Stopping running execution container"
                        );
                        if let Err(e) = docker.stop_container(&cid).await {
                            warn!(
                                workspace_id = %id,
                                container_id = %cid,
                                error = %e,
                                "Failed to stop container, attempting force remove"
                            );
                        }
                        if let Err(e) = docker.remove_container(&cid).await {
                            warn!(
                                workspace_id = %id,
                                container_id = %cid,
                                error = %e,
                                "Failed to remove container"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    workspace_id = %id,
                    error = %e,
                    "Failed to list running executions"
                );
            }
        }

        // 4. Abort running executions in database
        if let Err(e) = abort_executions_for_workspace(&pool_clone, id).await {
            warn!(
                workspace_id = %id,
                error = %e,
                "Failed to abort running executions"
            );
        }

        // 5. Drop the Postgres schema
        if let Some(schema) = &schema_name {
            info!(workspace_id = %id, schema = %schema, "Dropping workspace schema");
            if let Err(e) = drop_workspace_schema(&pool_clone, schema).await {
                warn!(
                    workspace_id = %id,
                    schema = %schema,
                    error = %e,
                    "Failed to drop workspace schema"
                );
            }
        }

        // 3a. Delete Neo4j graph data for this workspace
        {
            let cypher = format!("MATCH (n:{}) DETACH DELETE n", workspace_label);
            info!(workspace_id = %id, workspace_label = %workspace_label, "Deleting Neo4j graph data");
            match neo4j_graph.run(neo4rs::query(&cypher)).await {
                Ok(_) => info!(workspace_id = %id, "Neo4j graph data deleted"),
                Err(e) => warn!(
                    workspace_id = %id,
                    workspace_label = %workspace_label,
                    error = %e,
                    "Failed to delete Neo4j graph data"
                ),
            }
        }

        // 3b. Delete Qdrant collections
        if let Some(schema) = &schema_name {
            info!(workspace_id = %id, schema = %schema, "Deleting Qdrant collections");
            if let Err(e) =
                lifecycle::delete_qdrant_collections(&http_client, &qdrant_host, schema).await
            {
                warn!(
                    workspace_id = %id,
                    schema = %schema,
                    error = %e,
                    "Failed to delete Qdrant collections"
                );
            }
        }

        // 4. Remove Docker volume
        if let Some(vol) = &volume_name {
            info!(workspace_id = %id, volume = %vol, "Removing Docker volume");
            if let Err(e) = docker.remove_volume(vol).await {
                warn!(
                    workspace_id = %id,
                    volume = %vol,
                    error = %e,
                    "Failed to remove Docker volume"
                );
            }
        }

        // 5. Clean up host clone directory
        if let Some(path) = &clone_path {
            info!(workspace_id = %id, clone_path = %path, "Removing clone directory");
            if let Err(e) = tokio::fs::remove_dir_all(path).await {
                warn!(
                    workspace_id = %id,
                    clone_path = %path,
                    error = %e,
                    "Failed to remove clone directory"
                );
            }
        }

        info!(workspace_id = %id, "Workspace cleanup complete");
    });

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /workspaces/:id/files` — return a directory tree of the cloned repo.
///
/// Returns `400` if not yet cloned, `404` if workspace doesn't exist.
pub async fn list_files(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FileNode>, AppError> {
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

    let root_path = std::path::PathBuf::from(&clone_path);
    if root_path.exists() {
        let tree = tokio::task::spawn_blocking(move || build_tree(&root_path, &root_path))
            .await
            .map_err(|e| AppError::Internal(format!("File tree task panicked: {}", e)))?
            .map_err(|e| AppError::Internal(format!("Failed to build file tree: {}", e)))?;
        return Ok(Json(tree));
    }

    let vol_name = workspace.volume_name.as_deref().ok_or_else(|| {
        AppError::Internal(format!(
            "Clone path does not exist on disk and no Docker volume for workspace {}",
            id
        ))
    })?;

    let entries = state
        .docker
        .list_volume_paths(vol_name)
        .await
        .map_err(|e| {
            AppError::Internal(format!(
                "Failed to list files from Docker volume {}: {}",
                vol_name, e
            ))
        })?;

    let tree = build_tree_from_paths(&entries);
    Ok(Json(tree))
}

// =============================================================================
// Background clone task
// =============================================================================

/// Default Docker volume size in gigabytes for workspace volumes.
const DEFAULT_VOLUME_SIZE_GB: u32 = 10;

/// Clone the repository and then run the ingestion pipeline.
///
/// Status transitions:
/// - Clone success: `cloning` → (sets clone_path, creates volume) → `indexing`
/// - Ingestion success: `indexing` → `ready`
/// - Clone failure: `cloning` → `error`
/// - Volume creation failure: `cloning` → `error`
/// - Ingestion failure: `indexing` → `error`
///
/// `schema_name` is appended to `DATABASE_URL` as `search_path` so the
/// ingestion pipeline writes extracted items into the workspace-scoped
/// Postgres schema rather than the default schema.
#[allow(clippy::too_many_arguments)]
async fn run_clone(
    pool: sqlx::postgres::PgPool,
    client: GithubClient,
    docker: DockerClient,
    config: Config,
    http_client: reqwest::Client,
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
                "Clone complete — creating Docker volume"
            );
            if let Err(e) = lifecycle::clone_workspace(&pool, ws_id, &container_clone_dir).await {
                warn!("Failed to set clone_path for workspace {}: {}", ws_id, e);
            }

            // --- Stage 1b: Create Docker volume ---
            let vol_name = DockerClient::volume_name(&ws_id.to_string());
            if let Err(e) = docker
                .create_volume(&vol_name, DEFAULT_VOLUME_SIZE_GB)
                .await
            {
                warn!(
                    workspace_id = %ws_id,
                    volume = %vol_name,
                    "Failed to create Docker volume: {}", e
                );
                if let Err(e2) =
                    lifecycle::fail(&pool, ws_id, &format!("volume creation failed: {}", e)).await
                {
                    warn!(
                        "Failed to mark workspace {} as error after volume failure: {}",
                        ws_id, e2
                    );
                }
                return;
            }

            // --- Stage 1c: Populate volume with cloned files ---
            if let Err(e) = docker.populate_volume(&vol_name, &host_clone_dir).await {
                warn!(
                    workspace_id = %ws_id,
                    volume = %vol_name,
                    "Failed to populate Docker volume: {}", e
                );
                if let Err(e2) =
                    lifecycle::fail(&pool, ws_id, &format!("volume population failed: {}", e)).await
                {
                    warn!(
                        "Failed to mark workspace {} as error after volume population failure: {}",
                        ws_id, e2
                    );
                }
                return;
            }

            // Persist the volume name so execution handler can use it
            if let Err(e) = lifecycle::set_volume_name(&pool, ws_id, &vol_name).await {
                warn!("Failed to set volume_name for workspace {}: {}", ws_id, e);
            }

            info!(
                workspace_id = %ws_id,
                volume = %vol_name,
                "Volume ready — advancing to indexing"
            );

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

    // Recompute the volume name here — it is deterministic from ws_id and was
    // defined inside the match arm above, so it is no longer in scope.
    let vol_name = DockerClient::volume_name(&ws_id.to_string());

    // --- Stage 2: Create workspace Postgres schema ---
    // The ingestion pipeline writes into a workspace-scoped schema; it must
    // exist (with all tables) before the container starts.
    if let Err(e) = create_workspace_schema(&pool, &schema_name).await {
        error!(
            workspace_id = %ws_id,
            schema = %schema_name,
            "Failed to create workspace schema: {}",
            e
        );
        if let Err(e2) =
            lifecycle::fail(&pool, ws_id, &format!("schema creation failed: {}", e)).await
        {
            error!(
                "Failed to mark workspace {} as error after schema creation failure: {}",
                ws_id, e2
            );
        }
        return;
    }
    info!(
        workspace_id = %ws_id,
        schema = %schema_name,
        "Workspace schema created — starting ingestion pipeline"
    );

    // --- Stage 2b: Create Qdrant collections ---
    if let Err(e) = lifecycle::create_qdrant_collections(
        &http_client,
        &config.qdrant_host,
        &schema_name,
        config.embedding_dimensions,
    )
    .await
    {
        error!(
            workspace_id = %ws_id,
            schema = %schema_name,
            "Failed to create Qdrant collections: {}", e
        );
        if let Err(e2) = lifecycle::fail(
            &pool,
            ws_id,
            &format!("Qdrant collection creation failed: {}", e),
        )
        .await
        {
            error!(
                "Failed to mark workspace {} as error after Qdrant collection creation failure: {}",
                ws_id, e2
            );
        }
        return;
    }
    info!(
        workspace_id = %ws_id,
        schema = %schema_name,
        "Qdrant collections created"
    );

    // --- Stage 3: Ingest ---
    // Append search_path to the DATABASE_URL so the ingestion pipeline
    // writes into the workspace-scoped schema rather than the default schema.
    let db_url_with_schema = append_search_path(&config.database_url, &schema_name);

    let ingestion_cfg = IngestionConfig {
        // Use the named Docker volume (populated in Stage 1b) so the ingestion
        // container can access the repo whether the API itself runs in Docker or
        // bare-metal — host bind-mounts are not accessible to sibling containers.
        volume_name: &vol_name,
        network: &config.docker_network,
        database_url: &db_url_with_schema,
        neo4j_url: &config.neo4j_uri,
        neo4j_user: &config.neo4j_user,
        neo4j_password: &config.neo4j_password,
        ollama_host: &config.ollama_host,
        qdrant_host: &config.qdrant_host,
        embedding_model: &config.embedding_model,
        ingestion_image: &config.ingestion_image,
        workspace_id: &ws_id.to_string(),
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
            is_dir: false,
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
        is_dir: true,
        children,
    })
}

/// Build a [`FileNode`] tree from a flat list of `(relative_path, is_dir)` pairs.
///
/// Used as a fallback when the clone directory is gone but the Docker volume
/// still exists.
fn build_tree_from_paths(entries: &[(String, bool)]) -> FileNode {
    use std::collections::BTreeMap;

    struct DirEntry {
        is_dir: bool,
        children: BTreeMap<String, DirEntry>,
    }

    let mut root = DirEntry {
        is_dir: true,
        children: BTreeMap::new(),
    };

    for (path, is_dir) in entries {
        let parts: Vec<&str> = path.split('/').collect();
        let mut current = &mut root;
        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            let entry = current
                .children
                .entry(part.to_string())
                .or_insert_with(|| DirEntry {
                    is_dir: if is_last { *is_dir } else { true },
                    children: BTreeMap::new(),
                });
            if is_last {
                entry.is_dir = *is_dir;
            }
            current = entry;
        }
    }

    fn to_file_node(name: &str, path: &str, entry: &DirEntry) -> FileNode {
        let mut children: Vec<FileNode> = entry
            .children
            .iter()
            .map(|(child_name, child)| {
                let child_path = if path.is_empty() {
                    child_name.clone()
                } else {
                    format!("{}/{}", path, child_name)
                };
                to_file_node(child_name, &child_path, child)
            })
            .collect();
        children.sort_by_key(|c| (!c.is_dir, c.name.clone()));
        FileNode {
            name: name.to_string(),
            path: path.to_string(),
            is_dir: entry.is_dir,
            children,
        }
    }

    to_file_node(".", "", &root)
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
        assert!(tree.is_dir);
        assert_eq!(tree.children.len(), 1);
        assert_eq!(tree.children[0].name, "main.rs");
        assert!(!tree.children[0].is_dir);
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
            is_dir: false,
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
            is_dir: true,
            children: vec![FileNode {
                name: "main.rs".to_string(),
                path: "src/main.rs".to_string(),
                is_dir: false,
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

    #[test]
    fn test_create_workspace_request_empty_name() {
        let json = r#"{"github_url": "https://github.com/org/repo", "name": ""}"#;
        let req: CreateWorkspaceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name.as_deref(), Some(""));
    }

    #[test]
    fn test_build_tree_from_paths_basic() {
        let entries = vec![
            ("src".to_string(), true),
            ("src/main.rs".to_string(), false),
            ("Cargo.toml".to_string(), false),
        ];
        let tree = build_tree_from_paths(&entries);
        assert!(tree.is_dir);

        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["src", "Cargo.toml"]);

        let src = &tree.children[0];
        assert!(src.is_dir);
        assert_eq!(src.children.len(), 1);
        assert_eq!(src.children[0].name, "main.rs");
        assert_eq!(src.children[0].path, "src/main.rs");
    }

    #[test]
    fn test_build_tree_from_paths_dirs_before_files() {
        let entries = vec![
            ("README.md".to_string(), false),
            ("src".to_string(), true),
            ("src/lib.rs".to_string(), false),
        ];
        let tree = build_tree_from_paths(&entries);
        assert_eq!(tree.children[0].name, "src");
        assert_eq!(tree.children[1].name, "README.md");
    }

    #[test]
    fn test_build_tree_from_paths_empty() {
        let entries: Vec<(String, bool)> = vec![];
        let tree = build_tree_from_paths(&entries);
        assert!(tree.is_dir);
        assert!(tree.children.is_empty());
    }
}
