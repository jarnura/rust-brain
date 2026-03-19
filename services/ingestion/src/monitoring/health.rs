//! Health check HTTP endpoints for the ingestion pipeline.
//!
//! Provides Kubernetes-style probes and a Prometheus scrape endpoint:
//! - `GET /healthz`          — liveness probe (always returns "ok")
//! - `GET /readyz`           — readiness probe (can the pipeline accept work?)
//! - `GET /health/progress`  — JSON snapshot of current pipeline progress
//! - `GET /metrics`          — Prometheus text exposition format

use crate::monitoring::MetricsRegistry;
use crate::pipeline::{DegradationTier, ResilienceCoordinator};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

// =============================================================================
// SHARED STATE
// =============================================================================

/// Shared state exposed to health endpoints.
pub struct HealthState {
    pub resilience: Arc<ResilienceCoordinator>,
    pub metrics: Arc<MetricsRegistry>,
    pub progress: Arc<ProgressTracker>,
    /// Whether the pipeline is ready to accept work.
    pub ready: AtomicBool,
}

/// Tracks pipeline progress for the `/health/progress` endpoint.
pub struct ProgressTracker {
    pub current_stage: tokio::sync::RwLock<Option<String>>,
    /// Items processed (monotonically increasing).
    pub items_processed: AtomicU64,
    /// Total items expected (0 if unknown).
    pub items_total: AtomicU64,
    /// Pipeline start time (epoch millis, 0 if not started).
    pub started_at_ms: AtomicU64,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            current_stage: tokio::sync::RwLock::new(None),
            items_processed: AtomicU64::new(0),
            items_total: AtomicU64::new(0),
            started_at_ms: AtomicU64::new(0),
        }
    }

    pub async fn set_stage(&self, stage: impl Into<String>) {
        *self.current_stage.write().await = Some(stage.into());
    }

    pub async fn clear_stage(&self) {
        *self.current_stage.write().await = None;
    }

    pub fn record_items(&self, count: u64) {
        self.items_processed.fetch_add(count, Ordering::Relaxed);
    }

    pub fn set_total(&self, total: u64) {
        self.items_total.store(total, Ordering::Relaxed);
    }

    pub fn mark_started(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.started_at_ms.store(now, Ordering::Relaxed);
    }

    fn items_per_sec(&self) -> f64 {
        let started = self.started_at_ms.load(Ordering::Relaxed);
        if started == 0 {
            return 0.0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let elapsed_secs = (now.saturating_sub(started)) as f64 / 1000.0;
        if elapsed_secs < 0.001 {
            return 0.0;
        }
        self.items_processed.load(Ordering::Relaxed) as f64 / elapsed_secs
    }

    fn eta_seconds(&self) -> Option<f64> {
        let total = self.items_total.load(Ordering::Relaxed);
        let processed = self.items_processed.load(Ordering::Relaxed);
        if total == 0 || processed >= total {
            return None;
        }
        let rate = self.items_per_sec();
        if rate < 0.001 {
            return None;
        }
        Some((total - processed) as f64 / rate)
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// RESPONSE TYPES
// =============================================================================

/// JSON response for `/health/progress`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub tier: String,
    pub memory_pressure: String,
    pub current_stage: Option<String>,
    pub items_per_sec: f64,
    pub eta_seconds: Option<f64>,
}

// =============================================================================
// HANDLERS
// =============================================================================

/// `GET /healthz` — liveness probe. Always returns 200 "ok".
async fn healthz() -> &'static str {
    "ok"
}

/// `GET /readyz` — readiness probe. Returns 200 if pipeline can accept work,
/// 503 if not ready or in emergency degradation.
async fn readyz(State(state): State<Arc<HealthState>>) -> Response {
    let ready = state.ready.load(Ordering::Acquire);
    let tier = state.resilience.current_tier();

    if ready && tier != DegradationTier::Emergency {
        (StatusCode::OK, "ready").into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready").into_response()
    }
}

/// `GET /health/progress` — JSON snapshot of pipeline health and progress.
async fn health_progress(State(state): State<Arc<HealthState>>) -> Json<HealthResponse> {
    let tier = state.resilience.current_tier();
    let pressure = state.resilience.watchdog.current_pressure();
    let current_stage = state.progress.current_stage.read().await.clone();

    let status = match tier {
        DegradationTier::Full => "healthy",
        DegradationTier::Reduced => "degraded",
        DegradationTier::Minimal => "degraded",
        DegradationTier::Emergency => "emergency",
    };

    Json(HealthResponse {
        status: status.to_string(),
        tier: tier.to_string(),
        memory_pressure: pressure.to_string(),
        current_stage,
        items_per_sec: state.progress.items_per_sec(),
        eta_seconds: state.progress.eta_seconds(),
    })
}

/// `GET /metrics` — Prometheus text exposition format.
async fn metrics(State(state): State<Arc<HealthState>>) -> Response {
    let body = state.metrics.gather();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

// =============================================================================
// SERVER
// =============================================================================

/// Build the axum router with all health endpoints.
pub fn health_router(state: Arc<HealthState>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/health/progress", get(health_progress))
        .route("/metrics", get(metrics))
        .with_state(state)
}

/// Configuration for the health check server.
pub struct HealthServerConfig {
    /// Port to listen on (default: 9090).
    pub port: u16,
}

impl Default for HealthServerConfig {
    fn default() -> Self {
        Self { port: 9090 }
    }
}

impl HealthServerConfig {
    /// Read port from `HEALTH_PORT` env var, falling back to default.
    pub fn from_env() -> Self {
        let port = std::env::var("HEALTH_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(9090);
        Self { port }
    }
}

/// Spawn the health check HTTP server as a background tokio task.
///
/// Returns the `JoinHandle` so the caller can abort it on shutdown.
pub async fn spawn_health_server(
    config: HealthServerConfig,
    state: Arc<HealthState>,
) -> Result<tokio::task::JoinHandle<()>, std::io::Error> {
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    info!("Health server listening on {}", local_addr);

    let router = health_router(state);
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            error!("Health server error: {}", e);
        }
    });

    Ok(handle)
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> Arc<HealthState> {
        // Build a minimal resilience coordinator without a database pool.
        let resilience = Arc::new(
            ResilienceCoordinator::new(None, uuid::Uuid::new_v4())
                .expect("resilience coordinator"),
        );
        let metrics = Arc::new(MetricsRegistry::new().expect("metrics registry"));
        let progress = Arc::new(ProgressTracker::new());

        Arc::new(HealthState {
            resilience,
            metrics,
            progress,
            ready: AtomicBool::new(true),
        })
    }

    #[tokio::test]
    async fn test_healthz_returns_ok() {
        let state = test_state();
        let app = health_router(state);

        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn test_readyz_when_ready() {
        let state = test_state();
        let app = health_router(state);

        let resp = app
            .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_readyz_when_not_ready() {
        let state = test_state();
        state.ready.store(false, Ordering::Release);
        let app = health_router(state);

        let resp = app
            .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_health_progress_json() {
        let state = test_state();
        // Set started_at_ms to 5 seconds ago so items_per_sec() returns a
        // meaningful value and eta_seconds is Some.
        let five_secs_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 5_000;
        state
            .progress
            .started_at_ms
            .store(five_secs_ago, Ordering::Relaxed);
        state.progress.set_total(100);
        state.progress.record_items(10);
        let app = health_router(state);

        let resp = app
            .oneshot(
                Request::get("/health/progress")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(health.status, "healthy");
        assert_eq!(health.tier, "full");
        assert!(health.eta_seconds.is_some());
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let state = test_state();
        let app = health_router(state);

        let resp = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.contains("text/plain"));
    }

    #[test]
    fn test_progress_tracker_items_per_sec() {
        let tracker = ProgressTracker::new();
        // Not started yet — should be 0
        assert_eq!(tracker.items_per_sec(), 0.0);
        assert!(tracker.eta_seconds().is_none());
    }

    #[test]
    fn test_health_server_config_default() {
        let config = HealthServerConfig::default();
        assert_eq!(config.port, 9090);
    }

    #[test]
    fn test_health_response_serialization() {
        let resp = HealthResponse {
            status: "healthy".to_string(),
            tier: "full".to_string(),
            memory_pressure: "normal".to_string(),
            current_stage: Some("parse".to_string()),
            items_per_sec: 42.5,
            eta_seconds: Some(120.0),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"status\":\"healthy\""));
        assert!(json.contains("\"current_stage\":\"parse\""));
    }
}
