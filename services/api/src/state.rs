//! Application state and metrics for the rust-brain API server.
//!
//! [`AppState`] is the Axum shared state passed to every handler via
//! `State(state)`. It holds connection pools, HTTP clients, and the
//! Prometheus [`Metrics`] collector.

use crate::config::Config;
use crate::docker::DockerClient;
use crate::metrics::WorkspaceGauges;
use crate::middleware::PerKeyRateLimiter;
use crate::opencode::OpenCodeClient;
use crate::workspace::WorkspaceManager;
use neo4rs::Graph;
use prometheus::Registry;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// Per-key rate limiter using fixed-window token buckets.
///
/// Each API key gets its own bucket keyed by key ID, with the rate limit
/// configured in the `api_keys` table. See [`crate::middleware::rate_limit`].
pub type KeyRateLimiter = PerKeyRateLimiter;

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
    /// Docker client for volume and container lifecycle operations
    pub docker: DockerClient,
    /// Process start time for uptime calculation
    pub start_time: Instant,
    /// Per-key rate limiter keyed by API key ID
    pub rate_limiter: Arc<KeyRateLimiter>,
    /// Semaphore limiting concurrent ingestion container spawns.
    ///
    /// Sized from `Config::max_concurrent_ingestions`. Tasks that cannot
    /// acquire a permit are queued in the tokio runtime until a slot frees.
    pub ingestion_semaphore: Arc<Semaphore>,
}

/// Prometheus metrics for API request tracking.
///
/// Exposes three metric families:
/// - `rustbrain_api_requests_total` — counter by endpoint, method, and workspace
/// - `rustbrain_api_request_duration_seconds` — histogram by endpoint and workspace
/// - `rustbrain_api_errors_total` — counter by endpoint, error_code, and workspace
///
/// Scraped via `GET /metrics` (see [`crate::handlers::health::metrics_handler`]).
pub struct Metrics {
    /// Prometheus registry that owns all metric families
    pub registry: Registry,
    /// Total requests counter (labels: `endpoint`, `method`, `workspace`)
    pub requests_total: prometheus::CounterVec,
    /// Request duration histogram (labels: `endpoint`, `workspace`)
    pub request_duration: prometheus::HistogramVec,
    /// Total errors counter (labels: `endpoint`, `error_code`, `workspace`)
    pub errors_total: prometheus::CounterVec,
    /// Per-workspace resource gauges (updated by background collector)
    pub workspace_gauges: Arc<WorkspaceGauges>,
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
            &["endpoint", "method", "workspace"],
        )
        .expect("Failed to create requests_total metric");

        let request_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "rustbrain_api_request_duration_seconds",
                "Request duration",
            ),
            &["endpoint", "workspace"],
        )
        .expect("Failed to create request_duration metric");

        let errors_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_errors_total", "Total API errors"),
            &["endpoint", "error_code", "workspace"],
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

        let workspace_gauges = WorkspaceGauges::new(&registry);

        Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
            workspace_gauges: Arc::new(workspace_gauges),
        }
    }

    /// Increments the `requests_total` counter for the given endpoint, method, and workspace.
    pub fn record_request(&self, endpoint: &str, method: &str, workspace: &str) {
        self.requests_total
            .with_label_values(&[endpoint, method, workspace])
            .inc();
    }

    /// Records a request duration observation for the given endpoint and workspace.
    pub fn record_duration(&self, endpoint: &str, workspace: &str, duration: std::time::Duration) {
        self.request_duration
            .with_label_values(&[endpoint, workspace])
            .observe(duration.as_secs_f64());
    }

    /// Increments the `errors_total` counter for the given endpoint, error code, and workspace.
    pub fn record_error(&self, endpoint: &str, error_code: &str, workspace: &str) {
        self.errors_total
            .with_label_values(&[endpoint, error_code, workspace])
            .inc();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new();
        metrics.record_request("test_endpoint", "GET", "none");
        metrics.record_error("test_endpoint", "500", "none");
        metrics.record_duration(
            "test_endpoint",
            "none",
            std::time::Duration::from_millis(100),
        );
        // No panic = success
    }
}
