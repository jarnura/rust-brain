//! Validator run query endpoints.
//!
//! Exposes read-only access to validator results stored in `validator_runs`:
//!
//! - `GET /validator/runs?repo=<repo>&pr=<num>` — list all runs for a PR
//! - `GET /validator/runs/:id` — get full detail for a single run

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use crate::errors::AppError;
use crate::state::AppState;

// =============================================================================
// Types
// =============================================================================

/// Query parameters for listing validator runs.
#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    /// GitHub repository in `owner/repo` form.
    pub repo: String,
    /// Pull request number.
    pub pr: i32,
}

/// Summary of a single validator run (list view).
#[derive(Debug, Serialize)]
pub struct RunSummary {
    pub id: String,
    pub repo: String,
    pub pr_number: i32,
    pub run_index: i16,
    pub composite_score: f64,
    pub pass: bool,
    pub inverted: bool,
    pub created_at: String,
}

/// Full detail for a single validator run (detail view).
#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub id: String,
    pub repo: String,
    pub pr_number: i32,
    pub run_index: i16,
    pub composite_score: f64,
    pub pass: bool,
    pub inverted: bool,
    pub dimension_scores: serde_json::Value,
    pub tokens_used: Option<i32>,
    pub cost_usd: Option<f64>,
    pub created_at: String,
}

// =============================================================================
// Row types
// =============================================================================

type SummaryRow = (
    Uuid,
    String,
    i32,
    i16,
    f64,
    bool,
    bool,
    chrono::DateTime<chrono::Utc>,
);

type DetailRow = (
    Uuid,
    String,
    i32,
    i16,
    f64,
    bool,
    bool,
    serde_json::Value,
    Option<i32>,
    Option<f64>,
    chrono::DateTime<chrono::Utc>,
);

fn summary_from_row(r: SummaryRow) -> RunSummary {
    RunSummary {
        id: r.0.to_string(),
        repo: r.1,
        pr_number: r.2,
        run_index: r.3,
        composite_score: r.4,
        pass: r.5,
        inverted: r.6,
        created_at: r.7.to_rfc3339(),
    }
}

fn detail_from_row(r: DetailRow) -> RunDetail {
    RunDetail {
        id: r.0.to_string(),
        repo: r.1,
        pr_number: r.2,
        run_index: r.3,
        composite_score: r.4,
        pass: r.5,
        inverted: r.6,
        dimension_scores: r.7,
        tokens_used: r.8,
        cost_usd: r.9,
        created_at: r.10.to_rfc3339(),
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// List all validator runs for a given repo + PR, ordered by run_index.
pub async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<Vec<RunSummary>>, AppError> {
    debug!(repo = %query.repo, pr = query.pr, "Listing validator runs");

    let rows = sqlx::query_as::<_, SummaryRow>(
        r#"
        SELECT id, repo, pr_number, run_index, composite_score, pass, inverted, created_at
        FROM validator_runs
        WHERE repo = $1 AND pr_number = $2
        ORDER BY run_index ASC, created_at ASC
        "#,
    )
    .bind(&query.repo)
    .bind(query.pr)
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list validator runs: {e}")))?;

    Ok(Json(rows.into_iter().map(summary_from_row).collect()))
}

/// Get full detail for a single validator run by UUID.
pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunDetail>, AppError> {
    debug!(%id, "Getting validator run");

    let row = sqlx::query_as::<_, DetailRow>(
        r#"
        SELECT id, repo, pr_number, run_index, composite_score, pass, inverted,
               dimension_scores, tokens_used, cost_usd, created_at
        FROM validator_runs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get validator run: {e}")))?
    .ok_or_else(|| AppError::NotFound(format!("Validator run not found: {id}")))?;

    Ok(Json(detail_from_row(row)))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_runs_query_deserialises() {
        let qs: ListRunsQuery = serde_json::from_str(r#"{"repo":"org/repo","pr":42}"#).unwrap();
        assert_eq!(qs.repo, "org/repo");
        assert_eq!(qs.pr, 42);
    }

    #[test]
    fn run_summary_serialises() {
        let s = RunSummary {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            repo: "org/repo".to_string(),
            pr_number: 1,
            run_index: 0,
            composite_score: 3.5,
            pass: true,
            inverted: false,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["repo"], "org/repo");
        assert_eq!(json["pr_number"], 1);
        assert_eq!(json["pass"], true);
    }

    #[test]
    fn run_detail_serialises_dimension_scores() {
        let dims = serde_json::json!([{"dimension": "File Precision", "score": 4.0}]);
        let d = RunDetail {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            repo: "org/repo".to_string(),
            pr_number: 2,
            run_index: 1,
            composite_score: 4.0,
            pass: true,
            inverted: false,
            dimension_scores: dims.clone(),
            tokens_used: Some(800),
            cost_usd: Some(0.008),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["dimension_scores"], dims);
        assert_eq!(json["tokens_used"], 800);
    }

    #[test]
    fn run_detail_optional_fields_null() {
        let d = RunDetail {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            repo: "a/b".to_string(),
            pr_number: 3,
            run_index: 0,
            composite_score: 2.0,
            pass: false,
            inverted: true,
            dimension_scores: serde_json::json!([]),
            tokens_used: None,
            cost_usd: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&d).unwrap();
        assert!(json["tokens_used"].is_null());
        assert!(json["cost_usd"].is_null());
    }
}
