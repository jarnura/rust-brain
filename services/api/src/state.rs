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
    /// In-process LRU cache for query embeddings.
    ///
    /// Key: `"{model}:{sha256_hex(query_text)}"`. Value: the raw `Vec<f32>`
    /// returned by Ollama. Eliminates repeated Ollama calls for identical
    /// queries, cutting semantic search latency from ~22s to <1ms on cache hits.
    pub embedding_cache: moka::future::Cache<String, Vec<f32>>,
}

/// Prometheus metrics for API request tracking.
///
/// Exposes four metric families:
/// - `rustbrain_api_requests_total` — counter by endpoint, method, and workspace
/// - `rustbrain_api_request_duration_seconds` — histogram by endpoint and workspace
/// - `rustbrain_api_errors_total` — counter by endpoint, error_code, and workspace
/// - `rustbrain_embedding_duration_seconds` — histogram by model and cache_hit
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
    /// Embedding latency histogram (labels: `model`, `cache_hit`)
    ///
    /// `cache_hit="true"` observations are sub-millisecond (in-process cache).
    /// `cache_hit="false"` observations cover the full Ollama round-trip (can be 10–30s).
    pub embedding_duration: prometheus::HistogramVec,
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

        // Buckets spanning cache hits (~sub-ms) through full Ollama round-trips (~30s)
        let embedding_buckets = vec![
            0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 15.0, 30.0,
        ];
        let embedding_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "rustbrain_embedding_duration_seconds",
                "Embedding generation duration (cache hit or Ollama round-trip)",
            )
            .buckets(embedding_buckets),
            &["model", "cache_hit"],
        )
        .expect("Failed to create embedding_duration metric");

        registry
            .register(Box::new(embedding_duration.clone()))
            .expect("Failed to register embedding_duration");

        let workspace_gauges = WorkspaceGauges::new(&registry);

        Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
            workspace_gauges: Arc::new(workspace_gauges),
            embedding_duration,
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

    /// Records an embedding duration observation.
    ///
    /// `cache_hit` should be `"true"` for in-process cache hits and `"false"` for
    /// Ollama round-trips. `model` is the embedding model name from config.
    pub fn record_embedding_duration(
        &self,
        model: &str,
        cache_hit: bool,
        duration: std::time::Duration,
    ) {
        self.embedding_duration
            .with_label_values(&[model, if cache_hit { "true" } else { "false" }])
            .observe(duration.as_secs_f64());
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

    #[test]
    fn test_embedding_duration_histogram_registered() {
        let metrics = Metrics::new();
        // Prometheus only reports a metric family once at least one observation exists
        metrics.record_embedding_duration("test-model", false, std::time::Duration::from_millis(1));
        let mfs = metrics.registry.gather();
        let names: Vec<&str> = mfs.iter().map(|mf| mf.get_name()).collect();
        assert!(
            names.contains(&"rustbrain_embedding_duration_seconds"),
            "embedding_duration histogram not found in registry; got: {:?}",
            names
        );
    }

    #[test]
    fn test_embedding_duration_cache_hit_observation() {
        let metrics = Metrics::new();
        metrics.record_embedding_duration(
            "qwen3-embedding:4b",
            true,
            std::time::Duration::from_micros(500),
        );

        // The histogram for cache_hit="true" must have exactly one sample
        let mfs = metrics.registry.gather();
        let mf = mfs
            .iter()
            .find(|mf| mf.get_name() == "rustbrain_embedding_duration_seconds")
            .expect("metric family not found");

        let hit_metric = mf
            .get_metric()
            .iter()
            .find(|m| {
                m.get_label()
                    .iter()
                    .any(|lp| lp.get_name() == "cache_hit" && lp.get_value() == "true")
            })
            .expect("cache_hit=true metric not found");

        assert_eq!(hit_metric.get_histogram().get_sample_count(), 1);
    }

    #[test]
    fn test_embedding_duration_cache_miss_observation() {
        let metrics = Metrics::new();
        metrics.record_embedding_duration(
            "qwen3-embedding:4b",
            false,
            std::time::Duration::from_secs(12),
        );

        let mfs = metrics.registry.gather();
        let mf = mfs
            .iter()
            .find(|mf| mf.get_name() == "rustbrain_embedding_duration_seconds")
            .expect("metric family not found");

        let miss_metric = mf
            .get_metric()
            .iter()
            .find(|m| {
                m.get_label()
                    .iter()
                    .any(|lp| lp.get_name() == "cache_hit" && lp.get_value() == "false")
            })
            .expect("cache_hit=false metric not found");

        assert_eq!(miss_metric.get_histogram().get_sample_count(), 1);
        // 12s should fall in the 15s bucket
        assert!(miss_metric.get_histogram().get_sample_sum() >= 12.0);
    }

    #[test]
    fn test_embedding_duration_model_label() {
        let metrics = Metrics::new();
        metrics.record_embedding_duration("my-model", true, std::time::Duration::from_millis(1));

        let mfs = metrics.registry.gather();
        let mf = mfs
            .iter()
            .find(|mf| mf.get_name() == "rustbrain_embedding_duration_seconds")
            .expect("metric family not found");

        let model_metric = mf.get_metric().iter().find(|m| {
            m.get_label()
                .iter()
                .any(|lp| lp.get_name() == "model" && lp.get_value() == "my-model")
        });
        assert!(model_metric.is_some(), "model label not recorded");
    }

    #[test]
    fn test_embedding_duration_hit_and_miss_are_distinct() {
        let metrics = Metrics::new();
        metrics.record_embedding_duration(
            "qwen3-embedding:4b",
            true,
            std::time::Duration::from_micros(100),
        );
        metrics.record_embedding_duration(
            "qwen3-embedding:4b",
            false,
            std::time::Duration::from_secs(5),
        );

        let mfs = metrics.registry.gather();
        let mf = mfs
            .iter()
            .find(|mf| mf.get_name() == "rustbrain_embedding_duration_seconds")
            .expect("metric family not found");

        // Two distinct label-sets must be recorded
        let hit_count = mf
            .get_metric()
            .iter()
            .filter(|m| {
                m.get_label()
                    .iter()
                    .any(|lp| lp.get_name() == "cache_hit" && lp.get_value() == "true")
            })
            .count();
        let miss_count = mf
            .get_metric()
            .iter()
            .filter(|m| {
                m.get_label()
                    .iter()
                    .any(|lp| lp.get_name() == "cache_hit" && lp.get_value() == "false")
            })
            .count();

        assert_eq!(hit_count, 1);
        assert_eq!(miss_count, 1);
    }
}
