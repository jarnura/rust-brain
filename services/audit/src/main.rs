//! rust-brain Audit Service
//!
//! Workspace leak detection and compliance scanning service.
//! Runs periodic Cypher queries against Neo4j to detect cross-workspace
//! contamination and orphan nodes. Also detects orphaned Docker volumes
//! and containers not tracked in Postgres. Exposes metrics at :8090/metrics.

mod audit_writer;
mod config;
mod health;
mod leak_detector;
mod metrics;
mod neo4j_scanner;

use axum::routing::get;
use axum::Router;
use config::Config;
use neo4rs::Graph;
use rustbrain_common::logging::init_logging_with_directives;
use sqlx::postgres::PgPoolOptions;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{self, Duration};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn, Level};

/// Shared application state passed to Axum handlers.
#[derive(Clone)]
pub struct AppState {
    pub pg_pool: sqlx::PgPool,
    pub neo4j_graph: Arc<Graph>,
    pub config: Config,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _log_guard = init_logging_with_directives(Level::INFO, &["rustbrain_audit=debug"]);

    metrics::init();

    info!("Starting rust-brain audit service");

    let config = Config::from_env();
    info!(
        "Configuration loaded: port={}, interval={}s, dry_run={}, retention={}d",
        config.audit_port, config.audit_interval_secs, config.dry_run, config.log_retention_days
    );

    info!("Connecting to Postgres: {}", config.redacted_database_url());
    let pg_pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await?;
    info!("Connected to Postgres");

    info!("Connecting to Neo4j: {}", config.redacted_neo4j_url());
    let neo4j_graph = Graph::new(
        &config.neo4j_url,
        &config.neo4j_user,
        &config.neo4j_password,
    )
    .await?;
    info!("Connected to Neo4j");

    let state = Arc::new(AppState {
        pg_pool: pg_pool.clone(),
        neo4j_graph: Arc::new(neo4j_graph),
        config: config.clone(),
    });

    let app = Router::new()
        .route("/health", get(health::health_handler))
        .route("/metrics", get(metrics::metrics_handler))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let addr = SocketAddr::from(([0, 0, 0, 0], config.audit_port));
    info!("Listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let scan_state = state.clone();
    let scan_handle = tokio::spawn(async move {
        run_periodic_scan((*scan_state).clone()).await;
    });

    let result = axum::serve(listener, app).await;

    scan_handle.abort();

    result.map_err(Into::into)
}

async fn run_periodic_scan(state: AppState) {
    let interval = Duration::from_secs(state.config.audit_interval_secs as u64);

    run_single_scan(&state).await;

    let mut ticker = time::interval(interval);
    loop {
        ticker.tick().await;
        run_single_scan(&state).await;
    }
}

async fn run_single_scan(state: &AppState) {
    info!("Starting leak detection scan cycle");

    match leak_detector::detect_docker_leaks(&state.pg_pool, state.config.dry_run).await {
        Ok(result) => {
            metrics::ORPHAN_VOLUMES.set(result.orphan_volumes as f64);
            metrics::ORPHAN_CONTAINERS.set(result.orphan_containers as f64);
            metrics::CLEANUP_VOLUMES_REMOVED.set(result.cleaned_volumes as f64);
            metrics::CLEANUP_CONTAINERS_REMOVED.set(result.cleaned_containers as f64);

            if result.orphan_volumes > 0 || result.orphan_containers > 0 {
                warn!(
                    "Docker leaks: {} orphan volumes, {} orphan containers (dry_run={})",
                    result.orphan_volumes, result.orphan_containers, state.config.dry_run
                );

                for vol in &result.orphan_volume_names {
                    if let Err(e) = audit_writer::record_leak_detected(
                        &state.pg_pool,
                        "volume",
                        vol,
                        state.config.dry_run,
                    )
                    .await
                    {
                        error!(
                            "Failed to write audit record for orphan volume {}: {}",
                            vol, e
                        );
                    }
                }
                for cid in &result.orphan_container_ids {
                    if let Err(e) = audit_writer::record_leak_detected(
                        &state.pg_pool,
                        "container",
                        cid,
                        state.config.dry_run,
                    )
                    .await
                    {
                        error!(
                            "Failed to write audit record for orphan container {}: {}",
                            cid, e
                        );
                    }
                }

                if !state.config.dry_run {
                    for vol in &result.cleaned_volume_names {
                        if let Err(e) =
                            audit_writer::record_leak_cleaned(&state.pg_pool, "volume", vol).await
                        {
                            error!(
                                "Failed to write audit record for cleaned volume {}: {}",
                                vol, e
                            );
                        }
                    }
                    for cid in &result.cleaned_container_ids {
                        if let Err(e) =
                            audit_writer::record_leak_cleaned(&state.pg_pool, "container", cid)
                                .await
                        {
                            error!(
                                "Failed to write audit record for cleaned container {}: {}",
                                cid, e
                            );
                        }
                    }
                }
            } else {
                info!("No Docker resource leaks detected");
            }
        }
        Err(e) => {
            error!("Docker leak detection failed: {}", e);
        }
    }

    match neo4j_scanner::scan_neo4j_leaks(&state.neo4j_graph, None).await {
        Ok(result) => {
            metrics::MULTI_LABEL_NODES.set(result.multi_label_nodes as f64);
            metrics::ORPHAN_NODES.set(result.orphan_nodes as f64);
            metrics::BASELINE_ORPHAN_NODES.set(result.baseline_orphan_nodes as f64);

            if result.multi_label_nodes > 0 {
                warn!(
                    "Neo4j cross-workspace contamination: {} nodes with multiple Workspace_ labels",
                    result.multi_label_nodes
                );
            }
            if result.orphan_nodes > result.baseline_orphan_nodes {
                warn!(
                    "Neo4j orphan nodes: {} (baseline: {})",
                    result.orphan_nodes, result.baseline_orphan_nodes
                );
            }
            if result.multi_label_nodes == 0 && result.orphan_nodes <= result.baseline_orphan_nodes
            {
                info!("No Neo4j cross-workspace leaks detected");
            }
        }
        Err(e) => {
            error!("Neo4j leak scan failed: {}", e);
        }
    }

    if let Err(e) =
        audit_writer::prune_audit_log(&state.pg_pool, state.config.log_retention_days).await
    {
        error!("Audit log pruning failed: {}", e);
    }

    let now = chrono::Utc::now().timestamp();
    metrics::DETECTION_TIMESTAMP.set(now as f64);

    info!("Leak detection scan cycle complete");
}
