//! Benchmarker API endpoints.
//!
//! Exposes read/write access to benchmarker state stored in `eval_cases`,
//! `bench_runs`, and `bench_case_results`:
//!
//! - `GET /benchmarker/suites` — list eval suites with case counts
//! - `GET /benchmarker/runs?suite=<name>&status=<s>&limit=N` — list bench runs
//! - `GET /benchmarker/runs/:id` — get a single run with case results
//! - `POST /benchmarker/runs` — trigger a new bench run via the benchmarker binary

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::errors::AppError;
use crate::state::AppState;

// =============================================================================
// Types
// =============================================================================

/// Summary of a single eval suite (list view).
#[derive(Debug, Serialize)]
pub struct SuiteSummary {
    /// Suite name — matches `eval_cases.suite_name`.
    pub suite_name: String,
    /// Number of eval cases registered in this suite.
    pub case_count: i64,
}

/// Query parameters for listing bench runs.
#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    /// Filter by suite name.
    pub suite: Option<String>,
    /// Filter by status (`running`, `completed`, `failed`).
    pub status: Option<String>,
    /// Maximum number of rows to return (default 50, max 200).
    pub limit: Option<i64>,
}

/// Summary of a single bench run (list view).
#[derive(Debug, Serialize)]
pub struct BenchRunSummary {
    pub id: String,
    pub suite_name: String,
    pub release_tag: Option<String>,
    pub status: String,
    pub total_cases: i32,
    pub completed_cases: i32,
    pub pass_count: i32,
    pub pass_rate: Option<f64>,
    pub mean_composite: Option<f64>,
    pub total_cost_usd: Option<f64>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// A single per-case result nested inside a bench run detail response.
#[derive(Debug, Serialize)]
pub struct CaseResultDetail {
    pub id: String,
    pub eval_case_id: String,
    pub validator_run_id: Option<String>,
    pub run_index: i16,
    pub composite: f64,
    pub pass: bool,
    pub cost_usd: Option<f64>,
    pub created_at: String,
}

/// Full detail for a single bench run, including all per-case results.
#[derive(Debug, Serialize)]
pub struct BenchRunDetail {
    pub id: String,
    pub suite_name: String,
    pub release_tag: Option<String>,
    pub status: String,
    pub total_cases: i32,
    pub completed_cases: i32,
    pub pass_count: i32,
    pub pass_rate: Option<f64>,
    pub mean_composite: Option<f64>,
    pub total_cost_usd: Option<f64>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub case_results: Vec<CaseResultDetail>,
}

/// Request body for triggering a new bench run.
#[derive(Debug, Deserialize)]
pub struct TriggerRunRequest {
    /// Suite name — must already be synced via `benchmarker sync`.
    pub suite_name: String,
    /// Optional release tag stored on the `bench_runs` row.
    pub release_tag: Option<String>,
    /// Independent runs executed per eval case (default 2).
    pub runs_per_case: Option<u32>,
    /// Max number of cases to run concurrently (default 3).
    pub max_concurrent: Option<u32>,
}

/// Response body returned when a bench run is triggered.
#[derive(Debug, Serialize)]
pub struct TriggerRunResponse {
    pub message: String,
    pub suite_name: String,
}

// =============================================================================
// Row types (for sqlx tuple decoding)
// =============================================================================

type SuiteRow = (String, i64);

type RunSummaryRow = (
    Uuid,
    String,
    Option<String>,
    String,
    i32,
    i32,
    i32,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
);

type CaseResultRow = (
    Uuid,
    String,
    Option<Uuid>,
    i16,
    f64,
    bool,
    Option<f64>,
    chrono::DateTime<chrono::Utc>,
);

// =============================================================================
// Row → response type conversions
// =============================================================================

fn suite_from_row(r: SuiteRow) -> SuiteSummary {
    SuiteSummary {
        suite_name: r.0,
        case_count: r.1,
    }
}

fn run_summary_from_row(r: RunSummaryRow) -> BenchRunSummary {
    BenchRunSummary {
        id: r.0.to_string(),
        suite_name: r.1,
        release_tag: r.2,
        status: r.3,
        total_cases: r.4,
        completed_cases: r.5,
        pass_count: r.6,
        pass_rate: r.7,
        mean_composite: r.8,
        total_cost_usd: r.9,
        started_at: r.10.to_rfc3339(),
        completed_at: r.11.map(|t| t.to_rfc3339()),
    }
}

fn case_result_from_row(r: CaseResultRow) -> CaseResultDetail {
    CaseResultDetail {
        id: r.0.to_string(),
        eval_case_id: r.1,
        validator_run_id: r.2.map(|id| id.to_string()),
        run_index: r.3,
        composite: r.4,
        pass: r.5,
        cost_usd: r.6,
        created_at: r.7.to_rfc3339(),
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// List all eval suites with the number of cases registered in each.
///
/// `GET /benchmarker/suites`
pub async fn list_suites(
    State(state): State<AppState>,
) -> Result<Json<Vec<SuiteSummary>>, AppError> {
    state.metrics.record_request("list_benchmarker_suites", "GET");
    debug!("Listing eval suites");

    let rows = sqlx::query_as::<_, SuiteRow>(
        r#"
        SELECT suite_name, COUNT(*) AS case_count
        FROM eval_cases
        GROUP BY suite_name
        ORDER BY suite_name ASC
        "#,
    )
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list eval suites: {e}")))?;

    Ok(Json(rows.into_iter().map(suite_from_row).collect()))
}

/// List bench runs, optionally filtered by suite name and/or status.
///
/// `GET /benchmarker/runs?suite=<name>&status=<status>&limit=50`
pub async fn list_runs(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<Vec<BenchRunSummary>>, AppError> {
    state.metrics.record_request("list_benchmarker_runs", "GET");
    debug!(suite = ?query.suite, status = ?query.status, "Listing bench runs");

    let limit = query.limit.unwrap_or(50).min(200);

    let rows = sqlx::query_as::<_, RunSummaryRow>(
        r#"
        SELECT id, suite_name, release_tag, status,
               total_cases, completed_cases, pass_count,
               pass_rate, mean_composite, total_cost_usd,
               started_at, completed_at
        FROM bench_runs
        WHERE ($1::TEXT IS NULL OR suite_name = $1)
          AND ($2::TEXT IS NULL OR status     = $2)
        ORDER BY started_at DESC
        LIMIT $3
        "#,
    )
    .bind(&query.suite)
    .bind(&query.status)
    .bind(limit)
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list bench runs: {e}")))?;

    Ok(Json(rows.into_iter().map(run_summary_from_row).collect()))
}

/// Get full detail for a single bench run, including all per-case results.
///
/// `GET /benchmarker/runs/:id`
pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<BenchRunDetail>, AppError> {
    state.metrics.record_request("get_benchmarker_run", "GET");
    debug!(%id, "Getting bench run detail");

    let run = sqlx::query_as::<_, RunSummaryRow>(
        r#"
        SELECT id, suite_name, release_tag, status,
               total_cases, completed_cases, pass_count,
               pass_rate, mean_composite, total_cost_usd,
               started_at, completed_at
        FROM bench_runs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get bench run: {e}")))?
    .ok_or_else(|| AppError::NotFound(format!("Bench run not found: {id}")))?;

    let case_rows = sqlx::query_as::<_, CaseResultRow>(
        r#"
        SELECT id, eval_case_id, validator_run_id, run_index,
               composite, pass, cost_usd, created_at
        FROM bench_case_results
        WHERE bench_run_id = $1
        ORDER BY eval_case_id ASC, run_index ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get bench case results: {e}")))?;

    let summary = run_summary_from_row(run);
    let detail = BenchRunDetail {
        id: summary.id,
        suite_name: summary.suite_name,
        release_tag: summary.release_tag,
        status: summary.status,
        total_cases: summary.total_cases,
        completed_cases: summary.completed_cases,
        pass_count: summary.pass_count,
        pass_rate: summary.pass_rate,
        mean_composite: summary.mean_composite,
        total_cost_usd: summary.total_cost_usd,
        started_at: summary.started_at,
        completed_at: summary.completed_at,
        case_results: case_rows.into_iter().map(case_result_from_row).collect(),
    };

    Ok(Json(detail))
}

/// Trigger a new bench run by spawning the `benchmarker` binary in the background.
///
/// The suite must already be synced via `benchmarker sync`. Returns `202 Accepted`
/// immediately; the run appears in `GET /benchmarker/runs` once the benchmarker
/// process creates the `bench_runs` row.
///
/// The benchmarker binary path is read from the `BENCHMARKER_BIN` env var
/// (default: `"benchmarker"`).
///
/// `POST /benchmarker/runs`
pub async fn trigger_run(
    State(state): State<AppState>,
    Json(body): Json<TriggerRunRequest>,
) -> Result<(StatusCode, Json<TriggerRunResponse>), AppError> {
    state.metrics.record_request("trigger_benchmarker_run", "POST");

    // Validate: suite must have registered cases
    let (case_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM eval_cases WHERE suite_name = $1",
    )
    .bind(&body.suite_name)
    .fetch_one(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to check suite: {e}")))?;

    if case_count == 0 {
        return Err(AppError::BadRequest(format!(
            "No eval cases registered for suite '{}'. Run `benchmarker sync` first.",
            body.suite_name
        )));
    }

    let bin = std::env::var("BENCHMARKER_BIN").unwrap_or_else(|_| "benchmarker".to_string());
    let suite_name = body.suite_name.clone();
    let release_tag = body.release_tag.clone();
    let runs_per_case = body.runs_per_case.unwrap_or(2);
    let max_concurrent = body.max_concurrent.unwrap_or(3);

    info!(
        suite = %suite_name,
        ?release_tag,
        runs_per_case,
        max_concurrent,
        bin = %bin,
        "Triggering benchmarker run"
    );

    // Spawn as a detached background task — the process outlives this request.
    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.args(["bench", "--suite", &suite_name]);
        cmd.args(["--runs", &runs_per_case.to_string()]);
        cmd.args(["--concurrency", &max_concurrent.to_string()]);
        if let Some(tag) = &release_tag {
            cmd.args(["--release", tag]);
        }

        match cmd.output().await {
            Ok(output) if output.status.success() => {
                info!(suite = %suite_name, "Benchmarker run completed successfully");
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(
                    suite = %suite_name,
                    code = ?output.status.code(),
                    %stderr,
                    "Benchmarker run exited with error"
                );
            }
            Err(e) => {
                warn!(suite = %suite_name, error = %e, "Failed to spawn benchmarker binary");
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(TriggerRunResponse {
            message: "Benchmark run started".to_string(),
            suite_name: body.suite_name,
        }),
    ))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suite_summary_serialises() {
        let s = SuiteSummary {
            suite_name: "default".to_string(),
            case_count: 12,
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["suite_name"], "default");
        assert_eq!(json["case_count"], 12);
    }

    #[test]
    fn bench_run_summary_serialises() {
        let s = BenchRunSummary {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            suite_name: "default".to_string(),
            release_tag: Some("v1.0".to_string()),
            status: "completed".to_string(),
            total_cases: 10,
            completed_cases: 10,
            pass_count: 8,
            pass_rate: Some(0.8),
            mean_composite: Some(0.72),
            total_cost_usd: Some(0.05),
            started_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: Some("2026-01-01T01:00:00Z".to_string()),
        };
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["status"], "completed");
        assert_eq!(json["pass_count"], 8);
        assert!((json["pass_rate"].as_f64().unwrap() - 0.8).abs() < 1e-9);
    }

    #[test]
    fn bench_run_summary_null_optional_fields() {
        let s = BenchRunSummary {
            id: "00000000-0000-0000-0000-000000000000".to_string(),
            suite_name: "default".to_string(),
            release_tag: None,
            status: "running".to_string(),
            total_cases: 5,
            completed_cases: 2,
            pass_count: 1,
            pass_rate: None,
            mean_composite: None,
            total_cost_usd: None,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
        };
        let json = serde_json::to_value(&s).unwrap();
        assert!(json["release_tag"].is_null());
        assert!(json["pass_rate"].is_null());
        assert!(json["completed_at"].is_null());
    }

    #[test]
    fn case_result_detail_serialises() {
        let c = CaseResultDetail {
            id: "00000000-0000-0000-0000-000000000001".to_string(),
            eval_case_id: "add-workspace-endpoint".to_string(),
            validator_run_id: Some("00000000-0000-0000-0000-000000000002".to_string()),
            run_index: 0,
            composite: 0.75,
            pass: true,
            cost_usd: Some(0.01),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["eval_case_id"], "add-workspace-endpoint");
        assert_eq!(json["run_index"], 0);
        assert!(json["pass"].as_bool().unwrap());
    }

    #[test]
    fn trigger_run_request_deserialises_minimal() {
        let body = serde_json::json!({"suite_name": "default"});
        let req: TriggerRunRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.suite_name, "default");
        assert!(req.release_tag.is_none());
        assert!(req.runs_per_case.is_none());
        assert!(req.max_concurrent.is_none());
    }

    #[test]
    fn trigger_run_request_deserialises_full() {
        let body = serde_json::json!({
            "suite_name": "regression-set",
            "release_tag": "v2.0",
            "runs_per_case": 3,
            "max_concurrent": 5
        });
        let req: TriggerRunRequest = serde_json::from_value(body).unwrap();
        assert_eq!(req.suite_name, "regression-set");
        assert_eq!(req.release_tag.as_deref(), Some("v2.0"));
        assert_eq!(req.runs_per_case, Some(3));
        assert_eq!(req.max_concurrent, Some(5));
    }

    #[test]
    fn list_runs_query_all_optional() {
        let qs: ListRunsQuery =
            serde_json::from_str(r#"{"suite":"default","status":"completed","limit":10}"#)
                .unwrap();
        assert_eq!(qs.suite.as_deref(), Some("default"));
        assert_eq!(qs.status.as_deref(), Some("completed"));
        assert_eq!(qs.limit, Some(10));
    }

    #[test]
    fn list_runs_query_empty_is_valid() {
        let qs: ListRunsQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(qs.suite.is_none());
        assert!(qs.status.is_none());
        assert!(qs.limit.is_none());
    }
}
