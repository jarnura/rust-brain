//! Health check and metrics endpoints.
//!
//! - `GET /health` — checks all dependencies and returns aggregate status
//! - `GET /metrics` — Prometheus-format metrics
//! - `GET /api/snapshot` — snapshot manifest metadata

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
use crate::state::AppState;

/// JSON body returned by `GET /health`.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// `"healthy"` if all dependencies are up, `"degraded"` otherwise
    pub status: String,
    /// ISO 8601 timestamp of this check
    pub timestamp: String,
    /// Crate version from `Cargo.toml`
    pub version: String,
    /// Per-dependency health status
    pub dependencies: HashMap<String, DependencyStatus>,
    /// Process uptime in seconds
    pub uptime_secs: u64,
}

/// Health status of a single external dependency.
#[derive(Debug, Serialize)]
pub struct DependencyStatus {
    /// `"healthy"` or `"unhealthy"`
    pub status: String,
    /// Round-trip latency in milliseconds (if reachable)
    pub latency_ms: Option<u64>,
    /// Error message (if unhealthy)
    pub error: Option<String>,
    /// Qdrant-specific: number of indexed points
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points_count: Option<u64>,
    /// Postgres-specific: number of extracted_items rows
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items_count: Option<u64>,
    /// Neo4j-specific: number of nodes
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nodes_count: Option<u64>,
    /// Neo4j-specific: number of relationships (edges)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges_count: Option<u64>,
}

/// Checks all external dependencies and returns aggregate health status.
///
/// Dependencies checked: Postgres, Qdrant, Ollama, Neo4j, OpenCode.
/// Status is `"healthy"` only if **all** are reachable; otherwise `"degraded"`.
/// Includes per-store counts and process uptime.
///
/// # Errors
///
/// This handler does not return errors — individual dependency failures are
/// reported within the response body.
pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError> {
    let (postgres, qdrant, ollama, neo4j, opencode) = tokio::join!(
        check_postgres(&state),
        check_qdrant(&state),
        check_ollama(&state),
        check_neo4j_with_counts(&state),
        check_opencode(&state),
    );

    let mut dependencies = HashMap::new();
    dependencies.insert("postgres".to_string(), postgres);
    dependencies.insert("qdrant".to_string(), qdrant);
    dependencies.insert("ollama".to_string(), ollama);
    dependencies.insert("neo4j".to_string(), neo4j);
    dependencies.insert("opencode".to_string(), opencode);

    let all_healthy = dependencies.values().all(|d| d.status == "healthy");
    let status = if all_healthy { "healthy" } else { "degraded" };
    let uptime_secs = state.start_time.elapsed().as_secs();

    Ok(Json(HealthResponse {
        status: status.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        dependencies,
        uptime_secs,
    }))
}

async fn check_postgres(state: &AppState) -> DependencyStatus {
    let start = std::time::Instant::now();
    match sqlx::query("SELECT 1").execute(&state.pg_pool).await {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let items_count = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM extracted_items")
                .fetch_one(&state.pg_pool)
                .await
                .ok()
                .map(|(count,)| count as u64);

            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(latency_ms),
                error: None,
                points_count: None,
                items_count,
                nodes_count: None,
                edges_count: None,
            }
        }
        Err(e) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: None,
            error: Some(e.to_string()),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
    }
}

async fn check_qdrant(state: &AppState) -> DependencyStatus {
    let start = std::time::Instant::now();
    match state
        .http_client
        .get(format!(
            "{}/collections/{}",
            state.config.qdrant_host, state.config.collection_name
        ))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let latency = start.elapsed().as_millis() as u64;
            let points_count = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|v| v["result"]["points_count"].as_u64());
            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(latency),
                error: None,
                points_count,
                items_count: None,
                nodes_count: None,
                edges_count: None,
            }
        }
        Ok(resp) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("Status: {}", resp.status())),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
        Err(e) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: None,
            error: Some(e.to_string()),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
    }
}

async fn check_ollama(state: &AppState) -> DependencyStatus {
    let start = std::time::Instant::now();
    match state
        .http_client
        .get(format!("{}/api/tags", state.config.ollama_host))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => DependencyStatus {
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
        Ok(resp) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(format!("Status: {}", resp.status())),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
        Err(e) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: None,
            error: Some(e.to_string()),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
    }
}

async fn check_neo4j_with_counts(state: &AppState) -> DependencyStatus {
    let start = std::time::Instant::now();
    match crate::neo4j::check_neo4j(&state.neo4j_graph).await {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let count_result = crate::neo4j::execute_neo4j_query(
                state,
                "MATCH (n) WITH count(n) AS nodes OPTIONAL MATCH ()-[r]->() WITH nodes, count(r) AS rels RETURN nodes, rels",
                serde_json::json!({}),
            )
            .await
            .ok();

            let (nodes_count, edges_count) = count_result
                .and_then(|rows| rows.into_iter().next())
                .map(|row| {
                    let nodes = row.get("nodes").and_then(|v| v.as_u64());
                    let edges = row.get("rels").and_then(|v| v.as_u64());
                    (nodes, edges)
                })
                .unwrap_or((None, None));

            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(latency_ms),
                error: None,
                points_count: None,
                items_count: None,
                nodes_count,
                edges_count,
            }
        }
        Err(e) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: None,
            error: Some(e.to_string()),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
    }
}

async fn check_opencode(state: &AppState) -> DependencyStatus {
    let start = std::time::Instant::now();
    match state.opencode_client.health_check().await {
        Ok(true) => DependencyStatus {
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
        Ok(false) | Err(_) => DependencyStatus {
            status: "unhealthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            error: Some("OpenCode health check failed".to_string()),
            points_count: None,
            items_count: None,
            nodes_count: None,
            edges_count: None,
        },
    }
}

/// Returns snapshot metadata if a `.snapshot-manifest.json` exists in the working directory.
/// This file is written by `scripts/run-with-snapshot.sh` after restoring a snapshot.
pub async fn snapshot_info() -> Json<serde_json::Value> {
    let manifest_path = std::path::Path::new(".snapshot-manifest.json");
    if manifest_path.exists() {
        match std::fs::read_to_string(manifest_path) {
            Ok(contents) => match serde_json::from_str::<serde_json::Value>(&contents) {
                Ok(manifest) => Json(serde_json::json!({
                    "loaded": true,
                    "manifest": manifest,
                })),
                Err(_) => Json(serde_json::json!({ "loaded": false, "error": "invalid manifest" })),
            },
            Err(_) => Json(serde_json::json!({ "loaded": false })),
        }
    } else {
        Json(serde_json::json!({ "loaded": false }))
    }
}

/// Returns Prometheus metrics in text exposition format (`GET /metrics`).
///
/// Returns HTTP 500 if metric encoding fails.
pub async fn metrics_handler(State(state): State<AppState>) -> Response {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();
    let mut buffer = Vec::new();

    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to encode metrics",
        )
            .into_response();
    }

    match String::from_utf8(buffer) {
        Ok(metrics_text) => (StatusCode::OK, metrics_text).into_response(),
        Err(e) => {
            error!("Failed to convert metrics to string: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to encode metrics",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_serialization() {
        let mut deps = HashMap::new();
        deps.insert(
            "postgres".to_string(),
            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(5),
                error: None,
                points_count: None,
                items_count: Some(2285),
                nodes_count: None,
                edges_count: None,
            },
        );
        deps.insert(
            "neo4j".to_string(),
            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(10),
                error: None,
                points_count: None,
                items_count: None,
                nodes_count: Some(3200),
                edges_count: Some(5100),
            },
        );
        deps.insert(
            "qdrant".to_string(),
            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(3),
                error: None,
                points_count: Some(2285),
                items_count: None,
                nodes_count: None,
                edges_count: None,
            },
        );

        let resp = HealthResponse {
            status: "healthy".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            version: "0.1.0".to_string(),
            dependencies: deps,
            uptime_secs: 3600,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["uptime_secs"], 3600);
        assert_eq!(json["dependencies"]["postgres"]["status"], "healthy");
        assert_eq!(json["dependencies"]["postgres"]["latency_ms"], 5);
        assert_eq!(json["dependencies"]["postgres"]["items_count"], 2285);
        assert_eq!(json["dependencies"]["neo4j"]["nodes_count"], 3200);
        assert_eq!(json["dependencies"]["neo4j"]["edges_count"], 5100);
        assert_eq!(json["dependencies"]["qdrant"]["points_count"], 2285);
    }

    #[test]
    fn test_health_response_skip_empty_counts() {
        let mut deps = HashMap::new();
        deps.insert(
            "postgres".to_string(),
            DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(5),
                error: None,
                points_count: None,
                items_count: None,
                nodes_count: None,
                edges_count: None,
            },
        );

        let resp = HealthResponse {
            status: "healthy".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            version: "0.1.0".to_string(),
            dependencies: deps,
            uptime_secs: 60,
        };

        let json = serde_json::to_string_pretty(&resp).unwrap();
        assert!(!json.contains("items_count"));
        assert!(!json.contains("nodes_count"));
        assert!(!json.contains("edges_count"));
        assert!(!json.contains("points_count"));
    }
}
