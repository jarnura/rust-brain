//! rust-brain Tool API Server
//!
//! Provides REST endpoints for code intelligence queries.

pub mod opencode;
pub mod config;
pub mod errors;
pub mod state;
pub mod neo4j;
pub mod handlers;
mod gaps;

use axum::{
    response::Redirect,
    routing::{get, post},
    Router,
};
use neo4rs::Graph;
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use rustbrain_common::logging::init_logging_with_directives;
use tracing::{info, Level};

use config::{Config, redact_url};
use state::{AppState, Metrics};

/// Redirect /playground to /playground/ for static file serving
async fn playground_redirect() -> Redirect {
    Redirect::permanent("/playground/")
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
    info!("Connecting to Postgres: {}", redact_url(&config.database_url));
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
    ).await?;
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

    // Create app state
    let state = AppState {
        config: config.clone(),
        pg_pool,
        neo4j_graph: Arc::new(neo4j_graph),
        http_client,
        metrics,
        opencode_client,
    };

    // Build router
    let app = Router::new()
        // Health & metrics
        .route("/health", get(handlers::health::health))
        .route("/metrics", get(handlers::health::metrics_handler))
        // Playground (static file serving)
        .route("/playground", get(playground_redirect))
        .nest_service("/playground/", ServeDir::new("static").append_index_html_on_directories(true))
        // Code intelligence tools
        .route("/tools/search_semantic", post(handlers::search::search_semantic))
        .route("/tools/chat", post(handlers::chat::chat_handler))
        .route("/tools/chat/stream", get(handlers::chat::chat_stream_handler))
        .route("/tools/chat/send", post(handlers::chat::chat_send_handler))
        .route("/tools/get_function", get(handlers::items::get_function))
        .route("/tools/get_callers", get(handlers::items::get_callers))
        .route("/tools/get_trait_impls", get(handlers::graph::get_trait_impls))
        .route("/tools/find_usages_of_type", get(handlers::graph::find_usages_of_type))
        .route("/tools/get_module_tree", get(handlers::graph::get_module_tree))
        .route("/tools/query_graph", post(handlers::graph::query_graph))
        .route("/tools/aggregate_search", post(handlers::search::aggregate_search))
        // Ingestion progress
        .route("/api/ingestion/progress", get(handlers::ingestion::ingestion_progress))
        // OpenCode session management
        .route("/tools/chat/sessions", post(handlers::chat::chat_sessions_create).get(handlers::chat::chat_sessions_list))
        .route("/tools/chat/sessions/:id", get(handlers::chat::chat_sessions_get).delete(handlers::chat::chat_sessions_delete))
        .route("/tools/chat/sessions/:id/fork", post(handlers::chat::chat_sessions_fork))
        .route("/tools/chat/sessions/:id/abort", post(handlers::chat::chat_sessions_abort))
        // Middleware
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
