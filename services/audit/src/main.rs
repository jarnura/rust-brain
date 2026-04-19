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
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{self, Duration};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn, Level};
use uuid::Uuid;

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
    let scan_start = std::time::Instant::now();
    info!("Starting leak detection scan cycle");

    let ws_names = load_workspace_names(&state.pg_pool).await;

    match leak_detector::detect_docker_leaks(&state.pg_pool, state.config.dry_run).await {
        Ok(result) => {
            metrics::ORPHAN_VOLUMES.set(result.orphan_volumes as f64);
            metrics::ORPHAN_CONTAINERS.set(result.orphan_containers as f64);
            metrics::CLEANUP_VOLUMES_REMOVED.set(result.cleaned_volumes as f64);
            metrics::CLEANUP_CONTAINERS_REMOVED.set(result.cleaned_containers as f64);

            set_per_workspace_docker_leaks(&result, &ws_names).await;

            if result.orphan_volumes > 0 || result.orphan_containers > 0 {
                warn!(
                    "Docker leaks: {} orphan volumes, {} orphan containers (dry_run={})",
                    result.orphan_volumes, result.orphan_containers, state.config.dry_run
                );

                for vol in &result.orphan_volume_names {
                    let ws_label = workspace_label_for_volume(vol, &ws_names);
                    metrics::WS_AUDIT_OPERATIONS_TOTAL
                        .with_label_values(&[&ws_label, "leak_detected"])
                        .inc();

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
                    let ws_label = "_unattributed".to_string();
                    metrics::WS_AUDIT_OPERATIONS_TOTAL
                        .with_label_values(&[&ws_label, "leak_detected"])
                        .inc();

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
                        let ws_label = workspace_label_for_volume(vol, &ws_names);
                        metrics::WS_AUDIT_OPERATIONS_TOTAL
                            .with_label_values(&[&ws_label, "leak_cleaned"])
                            .inc();

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
                        metrics::WS_AUDIT_OPERATIONS_TOTAL
                            .with_label_values(&["_unattributed", "leak_cleaned"])
                            .inc();

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
            metrics::CROSS_WORKSPACE_RELATIONSHIPS.set(result.cross_workspace_relationships as f64);
            metrics::LABEL_MISMATCHES.set(result.label_mismatches as f64);

            set_per_workspace_neo4j_metrics(&result, &ws_names);

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
            if result.cross_workspace_relationships > 0 {
                warn!(
                    "Neo4j cross-workspace relationships: {} edges connecting different workspaces",
                    result.cross_workspace_relationships
                );

                for detail in &result.cross_workspace_details {
                    let ws_label = resolve_workspace_label(&detail.source_workspace, &ws_names);
                    metrics::WS_AUDIT_OPERATIONS_TOTAL
                        .with_label_values(&[&ws_label, "cross_workspace_relationship"])
                        .inc();

                    if let Err(e) =
                        audit_writer::record_cross_workspace_relationship(&state.pg_pool, detail)
                            .await
                    {
                        error!(
                            "Failed to write audit record for cross-workspace relationship {}->{}: {}",
                            detail.source_fqn, detail.target_fqn, e
                        );
                    }
                }
            }
            if result.label_mismatches > 0 {
                warn!(
                    "Neo4j label-context mismatches: {} nodes with conflicting workspace labels",
                    result.label_mismatches
                );

                for detail in &result.label_mismatch_details {
                    let ws_label = resolve_workspace_label(&detail.actual_workspace, &ws_names);
                    metrics::WS_AUDIT_OPERATIONS_TOTAL
                        .with_label_values(&[&ws_label, "label_mismatch"])
                        .inc();

                    if let Err(e) =
                        audit_writer::record_label_mismatch(&state.pg_pool, detail).await
                    {
                        error!(
                            "Failed to write audit record for label mismatch {}: {}",
                            detail.fqn, e
                        );
                    }
                }
            }
            if result.multi_label_nodes == 0
                && result.orphan_nodes <= result.baseline_orphan_nodes
                && result.cross_workspace_relationships == 0
                && result.label_mismatches == 0
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

    let elapsed = scan_start.elapsed().as_secs_f64();
    metrics::SCAN_DURATION_SECS.set(elapsed);

    let now = chrono::Utc::now().timestamp();
    metrics::DETECTION_TIMESTAMP.set(now as f64);

    info!("Leak detection scan cycle complete (took {:.2}s)", elapsed);
}

fn set_per_workspace_neo4j_metrics(
    result: &neo4j_scanner::Neo4jScanResult,
    ws_names: &HashMap<String, String>,
) {
    for (ws_label, count) in &result.per_workspace.multi_label_nodes {
        let name = resolve_workspace_label(ws_label, ws_names);
        metrics::WS_AUDIT_MULTI_LABEL_NODES
            .with_label_values(&[&name])
            .set(*count as f64);
    }

    for (ws_label, count) in &result.per_workspace.cross_workspace_rels {
        let name = resolve_workspace_label(ws_label, ws_names);
        metrics::WS_AUDIT_CROSS_WORKSPACE_RELS
            .with_label_values(&[&name])
            .set(*count as f64);
    }

    for (ws_label, count) in &result.per_workspace.label_mismatches {
        let name = resolve_workspace_label(ws_label, ws_names);
        metrics::WS_AUDIT_LABEL_MISMATCHES
            .with_label_values(&[&name])
            .set(*count as f64);
    }

    metrics::WS_AUDIT_ORPHAN_NODES
        .with_label_values(&["_unlabeled"])
        .set(result.per_workspace.orphan_nodes as f64);
}

async fn set_per_workspace_docker_leaks(
    result: &leak_detector::LeakDetectionResult,
    ws_names: &HashMap<String, String>,
) {
    let mut volume_counts: HashMap<String, i64> = HashMap::new();
    for vol in &result.orphan_volume_names {
        let ws_label = workspace_label_for_volume(vol, ws_names);
        *volume_counts.entry(ws_label).or_insert(0) += 1;
    }
    for (ws, count) in &volume_counts {
        metrics::WS_AUDIT_LEAK_VOLUMES
            .with_label_values(&[ws])
            .set(*count as f64);
    }

    metrics::WS_AUDIT_LEAK_CONTAINERS
        .with_label_values(&["_unattributed"])
        .set(result.orphan_containers as f64);
}

async fn load_workspace_names(pool: &sqlx::PgPool) -> HashMap<String, String> {
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM workspaces WHERE status != 'archived'",
    )
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let mut map = HashMap::new();
            for (id, name) in rows {
                let hex = id.simple().to_string();
                map.insert(format!("Workspace_{}", hex), name);
            }
            debug!("Loaded {} workspace name mappings", map.len());
            map
        }
        Err(e) => {
            warn!("Failed to load workspace names: {}", e);
            HashMap::new()
        }
    }
}

fn resolve_workspace_label(neo4j_label: &str, ws_names: &HashMap<String, String>) -> String {
    ws_names
        .get(neo4j_label)
        .cloned()
        .unwrap_or_else(|| short_id_from_label(neo4j_label))
}

fn short_id_from_label(label: &str) -> String {
    match label.strip_prefix("Workspace_") {
        Some(hex) if hex.len() >= 12 => hex[..12].to_string(),
        Some(hex) => hex.to_string(),
        None => label.to_string(),
    }
}

fn workspace_label_for_volume(vol_name: &str, ws_names: &HashMap<String, String>) -> String {
    let short_id = vol_name.strip_prefix("rustbrain-ws-").unwrap_or(vol_name);

    for (label, name) in ws_names {
        if let Some(hex) = label.strip_prefix("Workspace_") {
            if hex.starts_with(short_id) {
                return name.clone();
            }
        }
    }

    "_unattributed".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_id_from_label_full() {
        let label = "Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6";
        assert_eq!(short_id_from_label(label), "a1b2c3d4e5f6");
    }

    #[test]
    fn test_short_id_from_label_short_hex() {
        let label = "Workspace_abc123";
        assert_eq!(short_id_from_label(label), "abc123");
    }

    #[test]
    fn test_short_id_from_label_no_prefix() {
        assert_eq!(short_id_from_label("some_string"), "some_string");
    }

    #[test]
    fn test_resolve_workspace_label_known() {
        let mut ws_names = HashMap::new();
        ws_names.insert(
            "Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
            "hyperswitch".to_string(),
        );

        let result =
            resolve_workspace_label("Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6", &ws_names);
        assert_eq!(result, "hyperswitch");
    }

    #[test]
    fn test_resolve_workspace_label_unknown() {
        let ws_names = HashMap::new();
        let result =
            resolve_workspace_label("Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6", &ws_names);
        assert_eq!(result, "a1b2c3d4e5f6");
    }

    #[test]
    fn test_workspace_label_for_volume_known() {
        let mut ws_names = HashMap::new();
        ws_names.insert(
            "Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
            "hyperswitch".to_string(),
        );

        let result = workspace_label_for_volume("rustbrain-ws-a1b2c3d", &ws_names);
        assert_eq!(result, "hyperswitch");
    }

    #[test]
    fn test_workspace_label_for_volume_unknown() {
        let ws_names = HashMap::new();
        let result = workspace_label_for_volume("rustbrain-ws-unknown1", &ws_names);
        assert_eq!(result, "_unattributed");
    }

    #[test]
    fn test_workspace_label_for_volume_no_prefix() {
        let ws_names = HashMap::new();
        let result = workspace_label_for_volume("some-random-volume", &ws_names);
        assert_eq!(result, "_unattributed");
    }

    #[test]
    fn test_per_workspace_neo4j_metrics_sets_values() {
        let registry = prometheus::Registry::new();
        registry
            .register(Box::new(metrics::WS_AUDIT_MULTI_LABEL_NODES.clone()))
            .unwrap();
        registry
            .register(Box::new(metrics::WS_AUDIT_CROSS_WORKSPACE_RELS.clone()))
            .unwrap();
        registry
            .register(Box::new(metrics::WS_AUDIT_LABEL_MISMATCHES.clone()))
            .unwrap();
        registry
            .register(Box::new(metrics::WS_AUDIT_ORPHAN_NODES.clone()))
            .unwrap();

        let mut ws_names = HashMap::new();
        ws_names.insert(
            "Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(),
            "test-workspace".to_string(),
        );

        let mut multi_label = HashMap::new();
        multi_label.insert("Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(), 3);

        let mut cross_rels = HashMap::new();
        cross_rels.insert("Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(), 2);

        let mut label_mismatches = HashMap::new();
        label_mismatches.insert("Workspace_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6".to_string(), 1);

        let result = neo4j_scanner::Neo4jScanResult {
            multi_label_nodes: 3,
            orphan_nodes: 5,
            baseline_orphan_nodes: 2,
            cross_workspace_relationships: 2,
            label_mismatches: 1,
            cross_workspace_details: vec![],
            label_mismatch_details: vec![],
            per_workspace: neo4j_scanner::PerWorkspaceBreakdown {
                multi_label_nodes: multi_label,
                cross_workspace_rels: cross_rels,
                label_mismatches,
                orphan_nodes: 5,
            },
        };

        set_per_workspace_neo4j_metrics(&result, &ws_names);

        assert!(metrics::WS_AUDIT_MULTI_LABEL_NODES
            .get_metric_with_label_values(&["test-workspace"])
            .is_ok());
        assert!(metrics::WS_AUDIT_CROSS_WORKSPACE_RELS
            .get_metric_with_label_values(&["test-workspace"])
            .is_ok());
        assert!(metrics::WS_AUDIT_LABEL_MISMATCHES
            .get_metric_with_label_values(&["test-workspace"])
            .is_ok());
        assert!(metrics::WS_AUDIT_ORPHAN_NODES
            .get_metric_with_label_values(&["_unlabeled"])
            .is_ok());
    }
}
