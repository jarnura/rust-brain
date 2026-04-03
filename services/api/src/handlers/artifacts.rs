//! Artifact CRUD handlers.
//!
//! Provides endpoints for managing artifacts in the inter-agent communication store:
//! - `POST /api/artifacts` — create
//! - `GET /api/artifacts/:id` — get by ID
//! - `GET /api/artifacts?task_id=X&type=Y&status=Z&producer=W` — list with filters
//! - `PUT /api/artifacts/:id` — update status, superseded_by, confidence

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::errors::AppError;
use crate::state::AppState;

const VALID_STATUSES: &[&str] = &["draft", "final", "superseded"];

// =============================================================================
// Types
// =============================================================================

/// Full artifact record.
#[derive(Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub task_id: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub producer: String,
    pub status: String,
    pub confidence: f64,
    pub summary: serde_json::Value,
    pub payload: serde_json::Value,
    pub created_at: Option<String>,
    pub superseded_by: Option<String>,
}

/// Request body for creating an artifact.
#[derive(Debug, Deserialize)]
pub struct CreateArtifactRequest {
    pub id: String,
    pub task_id: String,
    #[serde(rename = "type")]
    pub artifact_type: String,
    pub producer: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    pub summary: serde_json::Value,
    pub payload: serde_json::Value,
}

fn default_status() -> String {
    "draft".to_string()
}
fn default_confidence() -> f64 {
    1.0
}

/// Request body for updating an artifact.
#[derive(Debug, Deserialize)]
pub struct UpdateArtifactRequest {
    pub status: Option<String>,
    pub superseded_by: Option<String>,
    pub confidence: Option<f64>,
}

/// Query params for listing artifacts.
#[derive(Debug, Deserialize)]
pub struct ListArtifactsQuery {
    pub task_id: Option<String>,
    #[serde(rename = "type")]
    pub artifact_type: Option<String>,
    pub status: Option<String>,
    pub producer: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

// =============================================================================
// Row mapping
// =============================================================================

type ArtifactRow = (
    String,
    String,
    String,
    String,
    String,
    f64,
    serde_json::Value,
    serde_json::Value,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

fn row_to_artifact(row: ArtifactRow) -> Artifact {
    Artifact {
        id: row.0,
        task_id: row.1,
        artifact_type: row.2,
        producer: row.3,
        status: row.4,
        confidence: row.5,
        summary: row.6,
        payload: row.7,
        created_at: row.8.map(|t| t.to_rfc3339()),
        superseded_by: row.9,
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new artifact.
pub async fn create_artifact(
    State(state): State<AppState>,
    Json(request): Json<CreateArtifactRequest>,
) -> Result<Json<Artifact>, AppError> {
    state.metrics.record_request("create_artifact", "POST");
    debug!("Creating artifact: {}", request.id);

    if !VALID_STATUSES.contains(&request.status.as_str()) {
        return Err(AppError::BadRequest(format!(
            "Invalid status '{}'. Must be one of: {:?}",
            request.status, VALID_STATUSES
        )));
    }

    let row = sqlx::query_as::<_, ArtifactRow>(
        r#"
        INSERT INTO artifacts (id, task_id, type, producer, status, confidence, summary, payload)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, task_id, type, producer, status, confidence, summary, payload, created_at, superseded_by
        "#,
    )
    .bind(&request.id)
    .bind(&request.task_id)
    .bind(&request.artifact_type)
    .bind(&request.producer)
    .bind(&request.status)
    .bind(request.confidence)
    .bind(&request.summary)
    .bind(&request.payload)
    .fetch_one(&state.pg_pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(ref db_err) = e {
            if db_err.code().as_deref() == Some("23505") {
                return AppError::Conflict(format!(
                    "Artifact with ID '{}' already exists",
                    request.id
                ));
            }
        }
        AppError::Database(format!("Failed to create artifact: {}", e))
    })?;

    Ok(Json(row_to_artifact(row)))
}

/// Get an artifact by ID.
pub async fn get_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Artifact>, AppError> {
    state.metrics.record_request("get_artifact", "GET");
    debug!("Getting artifact: {}", id);

    let row = sqlx::query_as::<_, ArtifactRow>(
        r#"
        SELECT id, task_id, type, producer, status, confidence, summary, payload, created_at, superseded_by
        FROM artifacts WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get artifact: {}", e)))?
    .ok_or_else(|| AppError::NotFound(format!("Artifact not found: {}", id)))?;

    Ok(Json(row_to_artifact(row)))
}

/// List artifacts with optional filters (dynamic WHERE clause).
pub async fn list_artifacts(
    State(state): State<AppState>,
    Query(query): Query<ListArtifactsQuery>,
) -> Result<Json<Vec<Artifact>>, AppError> {
    state.metrics.record_request("list_artifacts", "GET");
    debug!(
        "Listing artifacts: task_id={:?}, type={:?}, status={:?}, producer={:?}",
        query.task_id, query.artifact_type, query.status, query.producer
    );

    let limit = query.limit.min(100);

    let rows = sqlx::query_as::<_, ArtifactRow>(
        r#"
        SELECT id, task_id, type, producer, status, confidence, summary, payload, created_at, superseded_by
        FROM artifacts
        WHERE ($1::TEXT IS NULL OR task_id = $1)
          AND ($2::TEXT IS NULL OR type = $2)
          AND ($3::TEXT IS NULL OR status = $3)
          AND ($4::TEXT IS NULL OR producer = $4)
        ORDER BY created_at DESC
        LIMIT $5
        "#,
    )
    .bind(&query.task_id)
    .bind(&query.artifact_type)
    .bind(&query.status)
    .bind(&query.producer)
    .bind(limit)
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list artifacts: {}", e)))?;

    let artifacts: Vec<Artifact> = rows.into_iter().map(row_to_artifact).collect();
    Ok(Json(artifacts))
}

/// Update an artifact's status, superseded_by, and/or confidence.
pub async fn update_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateArtifactRequest>,
) -> Result<Json<Artifact>, AppError> {
    state.metrics.record_request("update_artifact", "PUT");
    debug!("Updating artifact: {}", id);

    if let Some(ref status) = request.status {
        if !VALID_STATUSES.contains(&status.as_str()) {
            return Err(AppError::BadRequest(format!(
                "Invalid status '{}'. Must be one of: {:?}",
                status, VALID_STATUSES
            )));
        }
    }

    let row = sqlx::query_as::<_, ArtifactRow>(
        r#"
        UPDATE artifacts
        SET status = COALESCE($2, status),
            superseded_by = COALESCE($3, superseded_by),
            confidence = COALESCE($4, confidence)
        WHERE id = $1
        RETURNING id, task_id, type, producer, status, confidence, summary, payload, created_at, superseded_by
        "#,
    )
    .bind(&id)
    .bind(&request.status)
    .bind(&request.superseded_by)
    .bind(request.confidence)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to update artifact: {}", e)))?
    .ok_or_else(|| AppError::NotFound(format!("Artifact not found: {}", id)))?;

    Ok(Json(row_to_artifact(row)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_artifact_request_deserialization() {
        let json = r#"{
            "id": "art-001",
            "task_id": "task-001",
            "type": "prd",
            "producer": "planner",
            "summary": {"title": "Test"},
            "payload": {"content": "Test content"}
        }"#;
        let req: CreateArtifactRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, "art-001");
        assert_eq!(req.task_id, "task-001");
        assert_eq!(req.artifact_type, "prd");
        assert_eq!(req.status, "draft");
        assert_eq!(req.confidence, 1.0);
    }

    #[test]
    fn test_list_artifacts_query_defaults() {
        let json = r#"{}"#;
        let query: ListArtifactsQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 20);
        assert!(query.task_id.is_none());
        assert!(query.artifact_type.is_none());
        assert!(query.status.is_none());
        assert!(query.producer.is_none());
    }

    #[test]
    fn test_valid_artifact_statuses() {
        for status in VALID_STATUSES {
            assert!(
                VALID_STATUSES.contains(status),
                "Should be valid: {}",
                status
            );
        }
        assert!(!VALID_STATUSES.contains(&"invalid"));
        assert!(!VALID_STATUSES.contains(&"active"));
        assert!(!VALID_STATUSES.contains(&"deleted"));
    }

    #[test]
    fn test_update_artifact_request_deserialization() {
        let json = r#"{"status": "final", "confidence": 0.85}"#;
        let req: UpdateArtifactRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.status, Some("final".to_string()));
        assert_eq!(req.confidence, Some(0.85));
        assert_eq!(req.superseded_by, None);
    }

    #[test]
    fn test_artifact_serialization() {
        let artifact = Artifact {
            id: "art-001".to_string(),
            task_id: "task-001".to_string(),
            artifact_type: "prd".to_string(),
            producer: "planner".to_string(),
            status: "draft".to_string(),
            confidence: 0.9,
            summary: serde_json::json!({"title": "Test"}),
            payload: serde_json::json!({}),
            created_at: None,
            superseded_by: None,
        };
        let json = serde_json::to_value(&artifact).unwrap();
        assert_eq!(json["id"], "art-001");
        assert_eq!(json["type"], "prd");
    }
}
