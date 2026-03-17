//! Health check and metrics endpoints.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use prometheus::{Encoder, TextEncoder};
use serde::Serialize;
use std::collections::HashMap;
use tracing::error;

use crate::errors::AppError;
use crate::neo4j::check_neo4j;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub version: String,
    pub dependencies: HashMap<String, DependencyStatus>,
}

#[derive(Debug, Serialize)]
pub struct DependencyStatus {
    pub status: String,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError> {
    let mut dependencies = HashMap::new();

    // Check Postgres
    let start = std::time::Instant::now();
    match sqlx::query("SELECT 1").execute(&state.pg_pool).await {
        Ok(_) => {
            dependencies.insert("postgres".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Err(e) => {
            dependencies.insert("postgres".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }

    // Check Qdrant
    let start = std::time::Instant::now();
    match state.http_client
        .get(format!("{}/collections/{}", state.config.qdrant_host, state.config.collection_name))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Ok(resp) => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("Status: {}", resp.status())),
            });
        }
        Err(e) => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }

    // Check Ollama
    let start = std::time::Instant::now();
    match state.http_client
        .get(format!("{}/api/tags", state.config.ollama_host))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Ok(resp) => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("Status: {}", resp.status())),
            });
        }
        Err(e) => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }

    // Check Neo4j
    let start = std::time::Instant::now();
    match check_neo4j(&state).await {
        Ok(_) => {
            dependencies.insert("neo4j".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Err(e) => {
            dependencies.insert("neo4j".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }

    // Check OpenCode
    let start = std::time::Instant::now();
    match state.opencode_client.health_check().await {
        Ok(true) => {
            dependencies.insert("opencode".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Ok(false) | Err(_) => {
            dependencies.insert("opencode".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: Some("OpenCode health check failed".to_string()),
            });
        }
    }

    let all_healthy = dependencies.values().all(|d| d.status == "healthy");
    let status = if all_healthy { "healthy" } else { "degraded" };

    Ok(Json(HealthResponse {
        status: status.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        dependencies,
    }))
}

pub async fn metrics_handler(State(state): State<AppState>) -> Response {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics").into_response();
    }

    match String::from_utf8(buffer) {
        Ok(metrics_text) => (StatusCode::OK, metrics_text).into_response(),
        Err(e) => {
            error!("Failed to convert metrics to string: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_serialization() {
        let mut deps = HashMap::new();
        deps.insert("postgres".to_string(), DependencyStatus {
            status: "healthy".to_string(),
            latency_ms: Some(5),
            error: None,
        });

        let resp = HealthResponse {
            status: "healthy".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            version: "0.1.0".to_string(),
            dependencies: deps,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["dependencies"]["postgres"]["status"], "healthy");
        assert_eq!(json["dependencies"]["postgres"]["latency_ms"], 5);
    }
}
