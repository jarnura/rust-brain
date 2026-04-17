//! Health check handler for the audit service.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

pub async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pg_healthy = sqlx::query("SELECT 1")
        .execute(&state.pg_pool)
        .await
        .is_ok();

    let status = if pg_healthy { "healthy" } else { "degraded" };
    let http_status = if pg_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = json!({
        "status": status,
        "service": "rustbrain-audit",
        "postgres": if pg_healthy { "ok" } else { "error" },
        "dry_run": state.config.dry_run,
        "interval_secs": state.config.audit_interval_secs,
    });

    (http_status, axum::Json(body))
}
