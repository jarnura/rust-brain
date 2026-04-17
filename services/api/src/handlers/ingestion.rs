//! Ingestion progress endpoint.
//!
//! Serves `GET /api/ingestion/progress` which returns the status of the
//! most recent ingestion run from the `ingestion_runs` Postgres table.

use axum::{extract::State, Json};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use crate::errors::AppError;
use crate::extractors::OptionalWorkspaceId;
use crate::state::AppState;
use crate::workspace::acquire_conn;

/// Progress of a single pipeline stage (e.g., expand, parse, embed).
#[derive(Debug, Serialize)]
pub struct StageProgress {
    /// Stage name (e.g., `"expand"`, `"parse"`, `"embed"`)
    pub name: String,
    /// Stage status (`"success"`, `"running"`, `"failed"`)
    pub status: String,
    /// Number of items processed by this stage
    pub items_processed: i64,
}

/// Overall ingestion run progress returned by `GET /api/ingestion/progress`.
#[derive(Debug, Serialize)]
pub struct IngestionProgress {
    /// Run status (`"running"`, `"completed"`, `"failed"`)
    pub status: String,
    /// When the run started
    pub started_at: DateTime<Utc>,
    /// When the run completed (omitted if still running)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Number of crates processed
    pub crates_processed: i32,
    /// Total number of items extracted across all crates
    pub items_extracted: i32,
    /// Per-stage progress
    pub stages: Vec<StageProgress>,
    /// Errors encountered during the run (omitted if empty)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<Value>,
}

/// GET /api/ingestion/progress
///
/// Returns the latest ingestion run status with stage-level progress.
pub async fn ingestion_progress(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
) -> Result<Json<IngestionProgress>, AppError> {
    state.metrics.record_request("ingestion_progress", "GET");

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let row = sqlx::query_as::<_, IngestionRow>(
        r#"
        SELECT started_at, completed_at, status,
               crates_processed, items_extracted, errors, metadata
        FROM ingestion_runs
        ORDER BY started_at DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| AppError::Database(e.to_string()))?;

    let row = row.ok_or_else(|| AppError::NotFound("No ingestion runs found".to_string()))?;

    let stages = parse_stages(&row.metadata);

    Ok(Json(IngestionProgress {
        status: row.status,
        started_at: row.started_at,
        completed_at: row.completed_at,
        crates_processed: row.crates_processed.unwrap_or(0),
        items_extracted: row.items_extracted.unwrap_or(0),
        stages,
        errors: parse_errors(&row.errors),
    }))
}

#[derive(sqlx::FromRow)]
struct IngestionRow {
    started_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    status: String,
    crates_processed: Option<i32>,
    items_extracted: Option<i32>,
    errors: Option<Value>,
    metadata: Option<Value>,
}

fn parse_stages(metadata: &Option<Value>) -> Vec<StageProgress> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let Some(stages) = metadata.get("stages").and_then(|s| s.as_array()) else {
        return Vec::new();
    };
    stages
        .iter()
        .filter_map(|stage| {
            // The JSON uses "stage" as the key for the stage name
            let name = stage
                .get("stage")
                .or_else(|| stage.get("name"))
                .and_then(|s| s.as_str())?
                .to_string();
            let status = stage
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown")
                .to_string();
            let items_processed = stage
                .get("items_processed")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            Some(StageProgress {
                name,
                status,
                items_processed,
            })
        })
        .collect()
}

fn parse_errors(errors: &Option<Value>) -> Vec<Value> {
    errors
        .as_ref()
        .and_then(|e| e.as_array().cloned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_stages_with_valid_data() {
        let metadata = Some(json!({
            "stages": [
                {"stage": "expand", "status": "success", "items_processed": 5},
                {"stage": "parse", "status": "running", "items_processed": 100}
            ]
        }));
        let stages = parse_stages(&metadata);
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].name, "expand");
        assert_eq!(stages[0].status, "success");
        assert_eq!(stages[0].items_processed, 5);
        assert_eq!(stages[1].name, "parse");
        assert_eq!(stages[1].status, "running");
    }

    #[test]
    fn test_parse_stages_with_none() {
        assert!(parse_stages(&None).is_empty());
    }

    #[test]
    fn test_parse_stages_with_no_stages_key() {
        let metadata = Some(json!({"crate_path": "/tmp"}));
        assert!(parse_stages(&metadata).is_empty());
    }

    #[test]
    fn test_parse_stages_skips_entries_without_name() {
        let metadata = Some(json!({
            "stages": [
                {"status": "success", "items_processed": 5},
                {"stage": "parse", "status": "done", "items_processed": 10}
            ]
        }));
        let stages = parse_stages(&metadata);
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].name, "parse");
    }

    #[test]
    fn test_parse_errors_with_data() {
        let errors = Some(json!([{"stage": "embed", "message": "timeout"}]));
        let parsed = parse_errors(&errors);
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn test_parse_errors_with_none() {
        assert!(parse_errors(&None).is_empty());
    }

    #[test]
    fn test_ingestion_progress_serialization() {
        let progress = IngestionProgress {
            status: "running".to_string(),
            started_at: Utc::now(),
            completed_at: None,
            crates_processed: 3,
            items_extracted: 36000,
            stages: vec![StageProgress {
                name: "expand".to_string(),
                status: "success".to_string(),
                items_processed: 5,
            }],
            errors: vec![],
        };
        let json = serde_json::to_value(&progress).unwrap();
        assert_eq!(json["status"], "running");
        assert_eq!(json["items_extracted"], 36000);
        // completed_at and errors are omitted when None/empty
        assert!(json.get("completed_at").is_none());
        assert!(json.get("errors").is_none());
    }
}
