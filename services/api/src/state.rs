//! Application state and metrics for the rust-brain API server.

use crate::config::Config;
use crate::opencode::OpenCodeClient;
use neo4rs::Graph;
use prometheus::Registry;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pg_pool: sqlx::postgres::PgPool,
    pub neo4j_graph: Arc<Graph>,
    pub http_client: reqwest::Client,
    pub metrics: Arc<Metrics>,
    pub opencode_client: OpenCodeClient,
}

pub struct Metrics {
    pub registry: Registry,
    pub requests_total: prometheus::CounterVec,
    pub request_duration: prometheus::HistogramVec,
    pub errors_total: prometheus::CounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let requests_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_requests_total", "Total API requests"),
            &["endpoint", "method"],
        ).expect("Failed to create requests_total metric");

        let request_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new("rustbrain_api_request_duration_seconds", "Request duration"),
            &["endpoint"],
        ).expect("Failed to create request_duration metric");

        let errors_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_errors_total", "Total API errors"),
            &["endpoint", "error_code"],
        ).expect("Failed to create errors_total metric");

        registry.register(Box::new(requests_total.clone())).expect("Failed to register requests_total");
        registry.register(Box::new(request_duration.clone())).expect("Failed to register request_duration");
        registry.register(Box::new(errors_total.clone())).expect("Failed to register errors_total");

        Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
        }
    }

    pub fn record_request(&self, endpoint: &str, method: &str) {
        self.requests_total.with_label_values(&[endpoint, method]).inc();
    }

    pub fn record_error(&self, endpoint: &str, error_code: &str) {
        self.errors_total.with_label_values(&[endpoint, error_code]).inc();
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
