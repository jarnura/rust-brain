//! Prometheus metrics for the audit service.
//!
//! Metric names must match exactly with configs/prometheus/alert_rules.yml.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use lazy_static::lazy_static;
use prometheus::{Encoder, Gauge, Opts, Registry, TextEncoder};
use std::sync::Arc;

use crate::AppState;

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();

    /// Number of nodes with multiple Workspace_* labels (cross-workspace contamination).
    pub static ref MULTI_LABEL_NODES: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_workspace_leak_multi_label_nodes",
            "Number of Neo4j nodes with multiple Workspace_ labels indicating cross-workspace contamination")
    ).unwrap();

    /// Number of nodes with zero Workspace_* labels (orphan nodes).
    pub static ref ORPHAN_NODES: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_workspace_leak_orphan_nodes",
            "Number of Neo4j nodes with zero Workspace_ labels (orphan nodes)")
    ).unwrap();

    /// Baseline count of orphan nodes (pre-Phase-3 data not yet labeled).
    pub static ref BASELINE_ORPHAN_NODES: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_workspace_leak_baseline_orphan_nodes",
            "Baseline count of orphan nodes from before Workspace_ labels were applied")
    ).unwrap();

    /// Number of Docker volumes not tracked in Postgres.
    pub static ref ORPHAN_VOLUMES: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_leak_orphan_volumes_total",
            "Number of Docker volumes not tracked in Postgres")
    ).unwrap();

    /// Number of Docker containers not tracked in Postgres.
    pub static ref ORPHAN_CONTAINERS: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_leak_orphan_containers_total",
            "Number of Docker containers not tracked in Postgres")
    ).unwrap();

    /// Unix timestamp of last leak detection run.
    pub static ref DETECTION_TIMESTAMP: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_leak_detection_timestamp_seconds",
            "Unix timestamp of last leak detection run")
    ).unwrap();

    /// Number of orphaned volumes removed in the current run.
    pub static ref CLEANUP_VOLUMES_REMOVED: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_leak_cleanup_volumes_removed_total",
            "Number of orphaned volumes removed in the current run")
    ).unwrap();

    /// Number of orphaned containers removed in the current run.
    pub static ref CLEANUP_CONTAINERS_REMOVED: Gauge = Gauge::with_opts(
        Opts::new("rustbrain_leak_cleanup_containers_removed_total",
            "Number of orphaned containers removed in the current run")
    ).unwrap();
}

/// Register all metrics with the global registry.
pub fn init() {
    REGISTRY
        .register(Box::new(MULTI_LABEL_NODES.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(ORPHAN_NODES.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(BASELINE_ORPHAN_NODES.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(ORPHAN_VOLUMES.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(ORPHAN_CONTAINERS.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(DETECTION_TIMESTAMP.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(CLEANUP_VOLUMES_REMOVED.clone()))
        .unwrap();
    REGISTRY
        .register(Box::new(CLEANUP_CONTAINERS_REMOVED.clone()))
        .unwrap();

    // Set initial baseline to 0; operators should adjust after initial scan
    BASELINE_ORPHAN_NODES.set(0.0);
}

/// Axum handler for GET /metrics — returns Prometheus-format metrics.
pub async fn metrics_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = Vec::new();
    if encoder.encode(&metric_families, &mut buffer).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain")],
            "failed to encode metrics".to_string(),
        );
    }
    match String::from_utf8(buffer) {
        Ok(text) => (
            StatusCode::OK,
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            text,
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/plain")],
            "failed to convert metrics to string".to_string(),
        ),
    }
}
