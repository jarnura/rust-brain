//! rust-brain Tool API Server
//!
//! Provides REST endpoints for code intelligence queries.

pub mod config;
pub mod docker;
pub mod errors;
pub mod execution;
mod gaps;
pub mod github;
pub mod handlers;
pub mod neo4j;
pub mod opencode;
pub mod state;
pub mod workspace;

use axum::{
    extract::DefaultBodyLimit,
    response::Redirect,
    routing::{get, post},
    Router,
};
use neo4rs::Graph;
use rustbrain_common::logging::init_logging_with_directives;
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, Level};

/// Body size limit for query/write endpoints: 1 MiB.
const QUERY_BODY_LIMIT: usize = 1024 * 1024;

/// Rate limit burst size for embedding endpoints (requests per second per IP).
const EMBEDDING_RATE_PER_SEC: u64 = 10;

use config::{redact_url, Config};
use docker::DockerClient;
use state::{AppState, Metrics};

/// Redirect /playground to /playground/ for static file serving
async fn playground_redirect() -> Redirect {
    Redirect::permanent("/playground/")
}

/// Serve the legacy vanilla JS playground
async fn classic_playground() -> axum::response::Html<&'static str> {
    let html = include_str!("../static/classic.html");
    axum::response::Html(html)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing (stdout + optional LOG_FILE; format via LOG_FORMAT env var)
    let _log_guard = init_logging_with_directives(Level::INFO, &["rustbrain_api=debug"]);

    info!("Starting rust-brain API server");

    // Load configuration
    let config = Config::from_env();
    info!("Configuration loaded: port={}", config.port);

    // Connect to Postgres
    info!(
        "Connecting to Postgres: {}",
        redact_url(&config.database_url)
    );
    let pg_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;

    info!("Connected to Postgres");

    // Connect to Neo4j via Bolt protocol
    info!("Connecting to Neo4j: {}", redact_url(&config.neo4j_uri));
    let neo4j_graph = Graph::new(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await?;
    info!("Connected to Neo4j");

    // Create HTTP client (for Qdrant/Ollama)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Create metrics
    let metrics = Arc::new(Metrics::new());

    // Create OpenCode client
    let opencode_client = opencode::OpenCodeClient::new(
        config.opencode_host.clone(),
        config.opencode_auth_user.clone(),
        config.opencode_auth_pass.clone(),
    );

    // Create Docker client
    let docker = DockerClient::new();

    let workspace_manager = workspace::WorkspaceManager::new(pg_pool.clone(), docker.clone());

    let start_time = std::time::Instant::now();

    let state = AppState {
        config: config.clone(),
        pg_pool,
        neo4j_graph: Arc::new(neo4j_graph),
        http_client,
        metrics,
        opencode_client,
        workspace_manager: workspace_manager.clone(),
        docker: docker.clone(),
        start_time,
    };

    // Start timeout sweeper for stale execution containers
    execution::start_sweeper(
        workspace_manager.pool.clone(),
        docker,
        std::time::Duration::from_secs(30),
    );

    // Rate limiter: 10 req/s per IP, burst of 10 for embedding endpoints
    let embedding_governor_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(EMBEDDING_RATE_PER_SEC)
            .burst_size(EMBEDDING_RATE_PER_SEC as u32)
            .finish()
            .expect("valid governor config"),
    );

    // Embedding routes with per-IP rate limiting
    let embedding_routes = Router::new()
        .route(
            "/tools/search_semantic",
            post(handlers::search::search_semantic),
        )
        .route(
            "/tools/aggregate_search",
            post(handlers::search::aggregate_search),
        )
        .route("/tools/search_docs", post(handlers::search::search_docs))
        .layer(GovernorLayer {
            config: embedding_governor_config,
        });

    // Build router
    let app = Router::new()
        // Health & metrics
        .route("/health", get(handlers::health::health))
        .route("/metrics", get(handlers::health::metrics_handler))
        .route("/api/snapshot", get(handlers::health::snapshot_info))
        // Playground (static file serving)
        .route("/playground", get(playground_redirect))
        .route("/playground/classic", get(classic_playground))
        .nest_service(
            "/playground/",
            ServeDir::new("static").append_index_html_on_directories(true),
        )
        // Embedding endpoints (rate-limited)
        .merge(embedding_routes)
        // Code intelligence tools
        .route("/tools/chat", post(handlers::chat::chat_handler))
        .route(
            "/tools/chat/stream",
            get(handlers::chat::chat_stream_handler),
        )
        .route("/tools/chat/send", post(handlers::chat::chat_send_handler))
        .route("/tools/get_function", get(handlers::items::get_function))
        .route("/tools/get_callers", get(handlers::items::get_callers))
        .route(
            "/tools/get_trait_impls",
            get(handlers::graph::get_trait_impls),
        )
        .route(
            "/tools/find_usages_of_type",
            get(handlers::graph::find_usages_of_type),
        )
        .route(
            "/tools/get_module_tree",
            get(handlers::graph::get_module_tree),
        )
        // Type check queries
        .route(
            "/tools/find_calls_with_type",
            get(handlers::typecheck::find_calls_with_type),
        )
        .route(
            "/tools/find_trait_impls_for_type",
            get(handlers::typecheck::find_trait_impls_for_type),
        )
        .route("/tools/query_graph", post(handlers::graph::query_graph))
        // Read-only SQL query
        .route("/tools/pg_query", post(handlers::pg_query::pg_query))
        // Artifacts CRUD
        .route(
            "/api/artifacts",
            post(handlers::artifacts::create_artifact).get(handlers::artifacts::list_artifacts),
        )
        .route(
            "/api/artifacts/:id",
            get(handlers::artifacts::get_artifact).put(handlers::artifacts::update_artifact),
        )
        // Tasks CRUD
        .route(
            "/api/tasks",
            post(handlers::tasks::create_task).get(handlers::tasks::list_tasks),
        )
        .route(
            "/api/tasks/:id",
            get(handlers::tasks::get_task).put(handlers::tasks::update_task),
        )
        // Ingestion progress
        .route(
            "/api/ingestion/progress",
            get(handlers::ingestion::ingestion_progress),
        )
        // Cross-store consistency checker
        .route(
            "/api/consistency",
            get(handlers::consistency::check_consistency),
        )
        .route(
            "/health/consistency",
            get(handlers::consistency::health_consistency),
        )
        // Validator run results
        .route("/validator/runs", get(handlers::validator::list_runs))
        .route("/validator/runs/:id", get(handlers::validator::get_run))
        // Benchmarker
        .route(
            "/benchmarker/suites",
            get(handlers::benchmarker::list_suites),
        )
        .route(
            "/benchmarker/runs",
            get(handlers::benchmarker::list_runs).post(handlers::benchmarker::trigger_run),
        )
        .route("/benchmarker/runs/:id", get(handlers::benchmarker::get_run))
        // OpenCode session management
        .route(
            "/tools/chat/sessions",
            post(handlers::chat::chat_sessions_create).get(handlers::chat::chat_sessions_list),
        )
        .route(
            "/tools/chat/sessions/:id",
            get(handlers::chat::chat_sessions_get).delete(handlers::chat::chat_sessions_delete),
        )
        .route(
            "/tools/chat/sessions/:id/fork",
            post(handlers::chat::chat_sessions_fork),
        )
        .route(
            "/tools/chat/sessions/:id/abort",
            post(handlers::chat::chat_sessions_abort),
        )
        // Workspace management
        .route(
            "/workspaces",
            post(handlers::workspace::create_workspace).get(handlers::workspace::list_workspaces),
        )
        .route(
            "/workspaces/:id",
            get(handlers::workspace::get_workspace).delete(handlers::workspace::delete_workspace),
        )
        .route(
            "/workspaces/:id/files",
            get(handlers::workspace::list_files),
        )
        // Execution management
        .route(
            "/workspaces/:id/execute",
            post(handlers::execution::execute_workspace),
        )
        .route(
            "/workspaces/:id/executions",
            get(handlers::execution::list_executions),
        )
        .route("/executions/:id", get(handlers::execution::get_execution))
        .route(
            "/executions/:id/events",
            get(handlers::execution::stream_events),
        )
        // Workspace git operations
        .route(
            "/workspaces/:id/stream",
            get(handlers::workspace_stream::stream_workspace),
        )
        .route(
            "/workspaces/:id/diff",
            get(handlers::workspace_diff::workspace_diff),
        )
        .route(
            "/workspaces/:id/commit",
            post(handlers::workspace_commit::workspace_commit),
        )
        .route(
            "/workspaces/:id/reset",
            post(handlers::workspace_reset::workspace_reset),
        )
        // Middleware (applied to all routes)
        .layer(DefaultBodyLimit::max(QUERY_BODY_LIMIT))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}
