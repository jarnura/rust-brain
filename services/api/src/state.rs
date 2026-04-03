//! Application state and metrics for the rust-brain API server.
//!
//! [`AppState`] is the Axum shared state passed to every handler via
//! `State(state)`. It holds connection pools, HTTP clients, and the
//! Prometheus [`Metrics`] collector.

use crate::config::Config;
use crate::opencode::OpenCodeClient;
use crate::workspace::WorkspaceManager;
use neo4rs::Graph;
use prometheus::Registry;
use std::sync::Arc;

/// Shared application state available to all Axum handlers.
///
/// Created once during server startup and cloned (cheaply — inner fields
/// are `Arc`-wrapped or already `Clone`) into each request.
#[derive(Clone)]
pub struct AppState {
    /// Server configuration (loaded from environment)
    pub config: Config,
    /// PostgreSQL connection pool
    pub pg_pool: sqlx::postgres::PgPool,
    /// Neo4j Bolt connection (thread-safe via `Arc`)
    pub neo4j_graph: Arc<Graph>,
    /// Shared HTTP client for Qdrant and Ollama requests
    pub http_client: reqwest::Client,
    /// Prometheus metrics collector
    pub metrics: Arc<Metrics>,
    /// Client for the OpenCode session API
    pub opencode_client: OpenCodeClient,
    /// Workspace manager for lifecycle and schema operations
    pub workspace_manager: WorkspaceManager,
}

/// Prometheus metrics for API request tracking.
///
/// Exposes three metric families:
/// - `rustbrain_api_requests_total` — counter by endpoint and method
/// - `rustbrain_api_request_duration_seconds` — histogram by endpoint
/// - `rustbrain_api_errors_total` — counter by endpoint and error code
///
/// Scraped via `GET /metrics` (see [`crate::handlers::health::metrics_handler`]).
pub struct Metrics {
    /// Prometheus registry that owns all metric families
    pub registry: Registry,
    /// Total requests counter (labels: `endpoint`, `method`)
    pub requests_total: prometheus::CounterVec,
    /// Request duration histogram (label: `endpoint`)
    pub request_duration: prometheus::HistogramVec,
    /// Total errors counter (labels: `endpoint`, `error_code`)
    pub errors_total: prometheus::CounterVec,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    /// Creates and registers all metric families with a new Prometheus registry.
    ///
    /// # Panics
    ///
    /// Panics if metric creation or registration fails (e.g., duplicate
    /// metric names). This is called once at startup and is not expected to
    /// fail in normal operation.
    pub fn new() -> Self {
        let registry = Registry::new();

        let requests_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_requests_total", "Total API requests"),
            &["endpoint", "method"],
        )
        .expect("Failed to create requests_total metric");

        let request_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "rustbrain_api_request_duration_seconds",
                "Request duration",
            ),
            &["endpoint"],
        )
        .expect("Failed to create request_duration metric");

        let errors_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_errors_total", "Total API errors"),
            &["endpoint", "error_code"],
        )
        .expect("Failed to create errors_total metric");

        registry
            .register(Box::new(requests_total.clone()))
            .expect("Failed to register requests_total");
        registry
            .register(Box::new(request_duration.clone()))
            .expect("Failed to register request_duration");
        registry
            .register(Box::new(errors_total.clone()))
            .expect("Failed to register errors_total");

        Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
        }
    }

    /// Increments the `requests_total` counter for the given endpoint and HTTP method.
    pub fn record_request(&self, endpoint: &str, method: &str) {
        self.requests_total
            .with_label_values(&[endpoint, method])
            .inc();
    }

    /// Increments the `errors_total` counter for the given endpoint and error code.
    pub fn record_error(&self, endpoint: &str, error_code: &str) {
        self.errors_total
            .with_label_values(&[endpoint, error_code])
            .inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new();
        metrics.record_request("test_endpoint", "GET");
        metrics.record_error("test_endpoint", "500");
        // No panic = success
    }
}
