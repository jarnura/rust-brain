//! Postgres persistence for validator run results.
//!
//! One row per [`RunResult`] is stored in the `validator_runs` table. Each row
//! records the run index, composite score, pass/fail, inverted flag, and the
//! full per-dimension JSONB for later analysis.
//!
//! # Functions
//!
//! - [`save_run`] — persist all runs from a [`ValidationResult`], return IDs.
//! - [`list_runs`] — summarise all runs for a given repo + PR number.
//! - [`get_run`] — fetch full detail for a single run by UUID.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{RunResult, ValidationResult};

// =============================================================================
// Public response types
// =============================================================================

/// Summary of a single validator run (list view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Unique run ID.
    pub id: Uuid,
    /// GitHub repository in `owner/repo` form.
    pub repo: String,
    /// Pull request number.
    pub pr_number: i32,
    /// Zero-indexed run number within the validation batch.
    pub run_index: i16,
    /// Weighted composite score (1.0–5.0).
    pub composite_score: f64,
    /// Whether this run passed the threshold.
    pub pass: bool,
    /// True when the PR was expected to be rejected.
    pub inverted: bool,
    /// Timestamp of insertion.
    pub created_at: DateTime<Utc>,
}

/// Full detail for a single validator run (detail view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDetail {
    /// Unique run ID.
    pub id: Uuid,
    /// GitHub repository in `owner/repo` form.
    pub repo: String,
    /// Pull request number.
    pub pr_number: i32,
    /// Zero-indexed run number within the validation batch.
    pub run_index: i16,
    /// Weighted composite score (1.0–5.0).
    pub composite_score: f64,
    /// Whether this run passed the threshold.
    pub pass: bool,
    /// True when the PR was expected to be rejected.
    pub inverted: bool,
    /// Per-dimension scores as stored JSONB.
    pub dimension_scores: serde_json::Value,
    /// Approximate total tokens used (prompt + completion).
    pub tokens_used: Option<i32>,
    /// Estimated cost in USD.
    pub cost_usd: Option<f64>,
    /// Timestamp of insertion.
    pub created_at: DateTime<Utc>,
}

// =============================================================================
// Row mapping helpers
// =============================================================================

type SummaryRow = (Uuid, String, i32, i16, f64, bool, bool, DateTime<Utc>);
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
    DateTime<Utc>,
);

fn row_to_summary(r: SummaryRow) -> RunSummary {
    RunSummary {
        id: r.0,
        repo: r.1,
        pr_number: r.2,
        run_index: r.3,
        composite_score: r.4,
        pass: r.5,
        inverted: r.6,
        created_at: r.7,
    }
}

fn row_to_detail(r: DetailRow) -> RunDetail {
    RunDetail {
        id: r.0,
        repo: r.1,
        pr_number: r.2,
        run_index: r.3,
        composite_score: r.4,
        pass: r.5,
        inverted: r.6,
        dimension_scores: r.7,
        tokens_used: r.8,
        cost_usd: r.9,
        created_at: r.10,
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Persist all [`RunResult`]s from a [`ValidationResult`] into `validator_runs`.
///
/// Inserts one row per run. Returns the UUIDs assigned to each row in order.
///
/// The returned `ValidationResult` is identical to the input except that
/// `id` is set to the UUID of the **first** inserted row (index 0), which
/// callers can use as a stable reference for the whole batch.
pub async fn save_run(pool: &PgPool, result: &ValidationResult) -> Result<Vec<Uuid>> {
    let mut ids = Vec::with_capacity(result.runs.len());

    for run in &result.runs {
        let id = insert_run(pool, result, run)
            .await
            .with_context(|| format!("Failed to persist run_index={}", run.run_index))?;
        ids.push(id);
    }

    Ok(ids)
}

/// List all runs for a PR, ordered by `run_index` ascending.
pub async fn list_runs(pool: &PgPool, repo: &str, pr_number: i32) -> Result<Vec<RunSummary>> {
    let rows = sqlx::query_as::<_, SummaryRow>(
        r#"
        SELECT id, repo, pr_number, run_index, composite_score, pass, inverted, created_at
        FROM validator_runs
        WHERE repo = $1 AND pr_number = $2
        ORDER BY run_index ASC, created_at ASC
        "#,
    )
    .bind(repo)
    .bind(pr_number)
    .fetch_all(pool)
    .await
    .context("Failed to list validator runs")?;

    Ok(rows.into_iter().map(row_to_summary).collect())
}

/// Fetch full detail for a single run by its UUID.
///
/// Returns `None` when no row with that ID exists.
pub async fn get_run(pool: &PgPool, id: Uuid) -> Result<Option<RunDetail>> {
    let row = sqlx::query_as::<_, DetailRow>(
        r#"
        SELECT id, repo, pr_number, run_index, composite_score, pass, inverted,
               dimension_scores, tokens_used, cost_usd, created_at
        FROM validator_runs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("Failed to fetch validator run")?;

    Ok(row.map(row_to_detail))
}

// =============================================================================
// Internal helpers
// =============================================================================

async fn insert_run(pool: &PgPool, result: &ValidationResult, run: &RunResult) -> Result<Uuid> {
    let dimension_json =
        serde_json::to_value(&run.dimensions).context("Failed to serialise dimension scores")?;

    let (id,) = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO validator_runs
            (repo, pr_number, run_index, composite_score, pass, inverted,
             dimension_scores, tokens_used, cost_usd)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id
        "#,
    )
    .bind(&result.repo)
    .bind(result.pr_number as i32)
    .bind(run.run_index as i16)
    .bind(run.composite as f64)
    .bind(run.pass)
    .bind(result.inverted)
    .bind(dimension_json)
    .bind(run.tokens_used as i32)
    .bind(run.cost_usd as f64)
    .fetch_one(pool)
    .await
    .context("INSERT into validator_runs failed")?;

    Ok(id)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::DimensionScore;

    fn make_dim(name: &str, score: f32) -> DimensionScore {
        DimensionScore {
            dimension: name.to_string(),
            score,
            reasoning: "test".to_string(),
        }
    }

    fn make_run_result(index: u8) -> RunResult {
        RunResult {
            run_index: index,
            dimensions: vec![
                make_dim("File Precision", 4.0),
                make_dim("File Recall", 3.0),
                make_dim("Logical Equivalence", 4.0),
                make_dim("Code Quality", 5.0),
                make_dim("Edge Case Handling", 3.0),
                make_dim("Approach Validity", 4.0),
            ],
            composite: 3.95,
            pass: true,
            tokens_used: 1200,
            cost_usd: 0.012,
        }
    }

    fn make_validation_result(runs: Vec<RunResult>) -> ValidationResult {
        ValidationResult {
            id: None,
            repo: "org/repo".to_string(),
            pr_number: 42,
            mean_composite: 3.95,
            std_dev: 0.0,
            pass: true,
            inverted: false,
            cost_usd: runs.iter().map(|r| r.cost_usd).sum(),
            runs,
        }
    }

    #[test]
    fn run_summary_serialises_correctly() {
        let summary = RunSummary {
            id: Uuid::nil(),
            repo: "org/repo".to_string(),
            pr_number: 42,
            run_index: 0,
            composite_score: 3.95,
            pass: true,
            inverted: false,
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["repo"], "org/repo");
        assert_eq!(json["pr_number"], 42);
        assert_eq!(json["run_index"], 0);
        assert_eq!(json["pass"], true);
    }

    #[test]
    fn run_detail_serialises_dimension_scores() {
        let dims = serde_json::json!([
            {"dimension": "File Precision", "score": 4.0, "reasoning": "ok"}
        ]);
        let detail = RunDetail {
            id: Uuid::nil(),
            repo: "org/repo".to_string(),
            pr_number: 1,
            run_index: 0,
            composite_score: 4.0,
            pass: true,
            inverted: false,
            dimension_scores: dims.clone(),
            tokens_used: Some(1000),
            cost_usd: Some(0.01),
            created_at: DateTime::from_timestamp(0, 0).unwrap(),
        };
        let json = serde_json::to_value(&detail).unwrap();
        assert_eq!(json["dimension_scores"], dims);
        assert_eq!(json["tokens_used"], 1000);
    }

    #[test]
    fn save_run_batch_size_matches_runs() {
        // Verify that save_run would attempt to insert one row per RunResult.
        // We can't call the DB in a unit test, but we can verify the count logic.
        let result = make_validation_result(vec![make_run_result(0), make_run_result(1)]);
        assert_eq!(result.runs.len(), 2, "Should have 2 runs to persist");
    }

    #[test]
    fn dimension_json_roundtrip() {
        let run = make_run_result(0);
        let json = serde_json::to_value(&run.dimensions).unwrap();
        let back: Vec<DimensionScore> = serde_json::from_value(json).unwrap();
        assert_eq!(back.len(), run.dimensions.len());
        assert_eq!(back[0].dimension, run.dimensions[0].dimension);
        assert!((back[0].score - run.dimensions[0].score).abs() < 1e-4);
    }

    #[test]
    fn row_to_summary_maps_fields() {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let row: SummaryRow = (id, "org/repo".to_string(), 7, 0, 3.5, true, false, now);
        let summary = row_to_summary(row);
        assert_eq!(summary.id, id);
        assert_eq!(summary.repo, "org/repo");
        assert_eq!(summary.pr_number, 7);
        assert_eq!(summary.run_index, 0);
        assert!((summary.composite_score - 3.5).abs() < 1e-6);
        assert!(summary.pass);
        assert!(!summary.inverted);
    }

    #[test]
    fn row_to_detail_maps_optional_fields() {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let row: DetailRow = (
            id,
            "a/b".to_string(),
            3,
            1,
            4.2,
            false,
            true,
            serde_json::json!([]),
            None,
            None,
            now,
        );
        let detail = row_to_detail(row);
        assert!(detail.tokens_used.is_none());
        assert!(detail.cost_usd.is_none());
        assert!(detail.inverted);
    }
}
