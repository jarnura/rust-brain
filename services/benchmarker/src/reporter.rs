//! Reporter — aggregate statistics, release comparison, and regression detection.
//!
//! [`generate_report`] queries the database for all case results in a
//! `bench_run` and computes suite-level metrics.
//!
//! [`compare_runs`] accepts two [`BenchReport`]s and produces a
//! [`ReleaseComparison`] highlighting improvements and regressions.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

// =============================================================================
// Public types
// =============================================================================

/// Aggregate metrics for a single benchmark suite run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    /// `bench_runs.id`.
    pub bench_run_id: Uuid,
    /// Suite name.
    pub suite_name: String,
    /// Optional release tag (e.g. `"v1.2.0"`).
    pub release_tag: Option<String>,
    /// Total eval cases in the suite.
    pub total_cases: i64,
    /// Cases with at least one passing run.
    pub pass_count: i64,
    /// `pass_count / total_cases` (0.0 if no cases).
    pub pass_rate: f64,
    /// Mean composite score across all individual runs.
    pub mean_composite: f64,
    /// Sample variance of composite scores across all individual runs.
    pub composite_variance: f64,
    /// Standard deviation of composite scores.
    pub composite_std_dev: f64,
    /// Total cost across all validator runs.
    pub total_cost_usd: f64,
    /// Per-case summary (one entry per eval_case_id, averaged over runs).
    pub cases: Vec<CaseSummary>,
}

/// Per-case summary within a bench report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSummary {
    /// `eval_cases.id`.
    pub eval_case_id: String,
    /// GitHub repo.
    pub repo: String,
    /// PR number.
    pub pr_number: i32,
    /// Expected outcome (`"pass"` or `"reject"`).
    pub expected_outcome: String,
    /// Mean composite score across all runs for this case.
    pub mean_composite: f64,
    /// Whether the case is counted as passing (any run passed).
    pub pass: bool,
    /// Total cost for this case across all runs.
    pub cost_usd: f64,
}

/// Comparison between two benchmark suite runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseComparison {
    /// Run that serves as the baseline (older / golden).
    pub baseline_run_id: Uuid,
    /// Run being compared against the baseline (current).
    pub current_run_id: Uuid,
    /// Absolute change in pass rate (current − baseline).
    pub pass_rate_delta: f64,
    /// Absolute change in mean composite (current − baseline).
    pub composite_delta: f64,
    /// Cases that regressed: were passing in baseline but fail in current.
    pub regressions: Vec<RegressionEntry>,
    /// Cases that improved: were failing in baseline but pass in current.
    pub improvements: Vec<RegressionEntry>,
    /// Cases present in both runs with an unchanged outcome.
    pub stable: Vec<String>,
    /// Cases only in the current run (new cases).
    pub new_cases: Vec<String>,
    /// Cases only in the baseline run (removed cases).
    pub removed_cases: Vec<String>,
}

/// A single regression or improvement entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionEntry {
    /// `eval_cases.id`.
    pub eval_case_id: String,
    /// Composite score in the baseline run.
    pub baseline_composite: f64,
    /// Composite score in the current run.
    pub current_composite: f64,
    /// Delta (current − baseline).
    pub delta: f64,
}

// =============================================================================
// Public API
// =============================================================================

/// Generate a [`BenchReport`] from results already stored in the database.
///
/// Queries `bench_case_results` and `eval_cases` for `bench_run_id` and
/// computes all aggregate metrics in-process.
pub async fn generate_report(pool: &PgPool, bench_run_id: Uuid) -> Result<BenchReport> {
    let run_row = fetch_bench_run_meta(pool, bench_run_id).await?;

    // Per-case results joined with eval_case metadata
    let case_rows = fetch_case_rows(pool, bench_run_id).await?;

    // Aggregate per-case (each case may have multiple runs)
    let cases = aggregate_cases(&case_rows);

    // Suite-level stats from individual run composites
    let all_composites: Vec<f64> = case_rows.iter().map(|r| r.composite).collect();
    let (mean_composite, composite_variance) = mean_and_variance(&all_composites);
    let composite_std_dev = composite_variance.sqrt();

    let total_cost_usd: f64 = case_rows.iter().map(|r| r.cost_usd).sum();
    let pass_count = cases.iter().filter(|c| c.pass).count() as i64;
    let total_cases = cases.len() as i64;
    let pass_rate = if total_cases == 0 {
        0.0
    } else {
        pass_count as f64 / total_cases as f64
    };

    Ok(BenchReport {
        bench_run_id,
        suite_name: run_row.suite_name,
        release_tag: run_row.release_tag,
        total_cases,
        pass_count,
        pass_rate,
        mean_composite,
        composite_variance,
        composite_std_dev,
        total_cost_usd,
        cases,
    })
}

/// Compare a `current` run against a `baseline` run.
///
/// Both reports must already have been generated via [`generate_report`].
/// Cases are matched by `eval_case_id`.
pub fn compare_runs(baseline: &BenchReport, current: &BenchReport) -> ReleaseComparison {
    use std::collections::HashMap;

    let baseline_map: HashMap<&str, &CaseSummary> = baseline
        .cases
        .iter()
        .map(|c| (c.eval_case_id.as_str(), c))
        .collect();
    let current_map: HashMap<&str, &CaseSummary> = current
        .cases
        .iter()
        .map(|c| (c.eval_case_id.as_str(), c))
        .collect();

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    let mut stable = Vec::new();
    let mut new_cases = Vec::new();
    let mut removed_cases = Vec::new();

    // Cases in both runs
    for (id, cur) in &current_map {
        match baseline_map.get(id) {
            Some(base) => {
                let delta = cur.mean_composite - base.mean_composite;
                let entry = RegressionEntry {
                    eval_case_id: id.to_string(),
                    baseline_composite: base.mean_composite,
                    current_composite: cur.mean_composite,
                    delta,
                };
                match (base.pass, cur.pass) {
                    (true, false) => regressions.push(entry),
                    (false, true) => improvements.push(entry),
                    _ => stable.push(id.to_string()),
                }
            }
            None => new_cases.push(id.to_string()),
        }
    }

    // Cases only in baseline (removed)
    for id in baseline_map.keys() {
        if !current_map.contains_key(id) {
            removed_cases.push(id.to_string());
        }
    }

    // Sort for deterministic output
    regressions.sort_by(|a, b| {
        a.delta
            .partial_cmp(&b.delta)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    improvements.sort_by(|a, b| {
        b.delta
            .partial_cmp(&a.delta)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    stable.sort();
    new_cases.sort();
    removed_cases.sort();

    ReleaseComparison {
        baseline_run_id: baseline.bench_run_id,
        current_run_id: current.bench_run_id,
        pass_rate_delta: current.pass_rate - baseline.pass_rate,
        composite_delta: current.mean_composite - baseline.mean_composite,
        regressions,
        improvements,
        stable,
        new_cases,
        removed_cases,
    }
}

// =============================================================================
// Internal: statistics helpers
// =============================================================================

/// Compute mean and population variance of a slice of `f64` values.
///
/// Returns `(0.0, 0.0)` for empty slices.
pub fn mean_and_variance(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    (mean, variance)
}

/// Aggregate raw case rows into per-case summaries.
fn aggregate_cases(rows: &[CaseRow]) -> Vec<CaseSummary> {
    use std::collections::HashMap;

    // Group rows by eval_case_id
    let mut by_case: HashMap<&str, Vec<&CaseRow>> = HashMap::new();
    for row in rows {
        by_case.entry(&row.eval_case_id).or_default().push(row);
    }

    let mut summaries: Vec<CaseSummary> = by_case
        .into_iter()
        .map(|(id, case_rows)| {
            let composites: Vec<f64> = case_rows.iter().map(|r| r.composite).collect();
            let (mean_composite, _) = mean_and_variance(&composites);
            let pass = case_rows.iter().any(|r| r.pass);
            let cost_usd = case_rows.iter().map(|r| r.cost_usd).sum();
            // All rows for the same case share the same eval_case metadata
            let first = case_rows[0];
            CaseSummary {
                eval_case_id: id.to_string(),
                repo: first.repo.clone(),
                pr_number: first.pr_number,
                expected_outcome: first.expected_outcome.clone(),
                mean_composite,
                pass,
                cost_usd,
            }
        })
        .collect();

    summaries.sort_by(|a, b| a.eval_case_id.cmp(&b.eval_case_id));
    summaries
}

// =============================================================================
// Internal: DB helpers
// =============================================================================

struct BenchRunMeta {
    suite_name: String,
    release_tag: Option<String>,
}

async fn fetch_bench_run_meta(pool: &PgPool, bench_run_id: Uuid) -> Result<BenchRunMeta> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        r#"
        SELECT suite_name, release_tag
        FROM bench_runs
        WHERE id = $1
        "#,
    )
    .bind(bench_run_id)
    .fetch_optional(pool)
    .await
    .context("Fetching bench_run metadata")?
    .ok_or_else(|| anyhow::anyhow!("bench_run not found: {bench_run_id}"))?;

    Ok(BenchRunMeta {
        suite_name: row.0,
        release_tag: row.1,
    })
}

struct CaseRow {
    eval_case_id: String,
    repo: String,
    pr_number: i32,
    expected_outcome: String,
    composite: f64,
    pass: bool,
    cost_usd: f64,
}

async fn fetch_case_rows(pool: &PgPool, bench_run_id: Uuid) -> Result<Vec<CaseRow>> {
    type RawRow = (String, String, i32, String, f64, bool, Option<f64>);

    let rows = sqlx::query_as::<_, RawRow>(
        r#"
        SELECT
            bcr.eval_case_id,
            ec.repo,
            ec.pr_number,
            ec.expected_outcome,
            bcr.composite,
            bcr.pass,
            CAST(bcr.cost_usd AS FLOAT8)
        FROM bench_case_results bcr
        JOIN eval_cases ec ON ec.id = bcr.eval_case_id
        WHERE bcr.bench_run_id = $1
        ORDER BY bcr.eval_case_id, bcr.run_index
        "#,
    )
    .bind(bench_run_id)
    .fetch_all(pool)
    .await
    .with_context(|| format!("Fetching case results for bench_run {bench_run_id}"))?;

    Ok(rows
        .into_iter()
        .map(|r| CaseRow {
            eval_case_id: r.0,
            repo: r.1,
            pr_number: r.2,
            expected_outcome: r.3,
            composite: r.4,
            pass: r.5,
            cost_usd: r.6.unwrap_or(0.0),
        })
        .collect())
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // mean_and_variance
    // -------------------------------------------------------------------------

    #[test]
    fn mean_and_variance_empty() {
        let (m, v) = mean_and_variance(&[]);
        assert_eq!(m, 0.0);
        assert_eq!(v, 0.0);
    }

    #[test]
    fn mean_and_variance_single() {
        let (m, v) = mean_and_variance(&[3.0]);
        assert!((m - 3.0).abs() < 1e-9);
        assert!((v - 0.0).abs() < 1e-9);
    }

    #[test]
    fn mean_and_variance_uniform() {
        let (m, v) = mean_and_variance(&[2.0, 2.0, 2.0]);
        assert!((m - 2.0).abs() < 1e-9);
        assert!((v - 0.0).abs() < 1e-9);
    }

    #[test]
    fn mean_and_variance_known_values() {
        // values: 1, 2, 3 → mean=2, variance = ((1)^2 + 0 + 1) / 3 = 2/3
        let (m, v) = mean_and_variance(&[1.0, 2.0, 3.0]);
        assert!((m - 2.0).abs() < 1e-9, "mean: {m}");
        let expected_variance = 2.0 / 3.0;
        assert!(
            (v - expected_variance).abs() < 1e-9,
            "variance: {v} expected: {expected_variance}"
        );
    }

    #[test]
    fn std_dev_is_sqrt_of_variance() {
        let values = [1.0, 3.0, 5.0, 7.0];
        let (_, variance) = mean_and_variance(&values);
        let std_dev = variance.sqrt();
        // mean=4, variance=(9+1+1+9)/4=5, std_dev=sqrt(5)
        assert!((std_dev - 5.0f64.sqrt()).abs() < 1e-9, "std_dev: {std_dev}");
    }

    // -------------------------------------------------------------------------
    // compare_runs
    // -------------------------------------------------------------------------

    fn make_report(
        id: &str,
        cases: Vec<(&str, f64, bool)>,
        pass_rate: f64,
        mean_composite: f64,
    ) -> BenchReport {
        let uuid = Uuid::parse_str(id).unwrap_or_else(|_| Uuid::new_v4());
        BenchReport {
            bench_run_id: uuid,
            suite_name: "test".to_string(),
            release_tag: None,
            total_cases: cases.len() as i64,
            pass_count: cases.iter().filter(|(_, _, p)| *p).count() as i64,
            pass_rate,
            mean_composite,
            composite_variance: 0.0,
            composite_std_dev: 0.0,
            total_cost_usd: 0.0,
            cases: cases
                .into_iter()
                .map(|(id, composite, pass)| CaseSummary {
                    eval_case_id: id.to_string(),
                    repo: "org/repo".to_string(),
                    pr_number: 1,
                    expected_outcome: "pass".to_string(),
                    mean_composite: composite,
                    pass,
                    cost_usd: 0.0,
                })
                .collect(),
        }
    }

    #[test]
    fn compare_detects_regression() {
        let baseline = make_report(
            "00000000-0000-0000-0000-000000000001",
            vec![("case-a", 0.8, true), ("case-b", 0.7, true)],
            1.0,
            0.75,
        );
        let current = make_report(
            "00000000-0000-0000-0000-000000000002",
            vec![("case-a", 0.3, false), ("case-b", 0.7, true)],
            0.5,
            0.5,
        );

        let cmp = compare_runs(&baseline, &current);
        assert_eq!(cmp.regressions.len(), 1, "expected one regression");
        assert_eq!(cmp.regressions[0].eval_case_id, "case-a");
        assert!((cmp.pass_rate_delta - (-0.5)).abs() < 1e-9);
    }

    #[test]
    fn compare_detects_improvement() {
        let baseline = make_report(
            "00000000-0000-0000-0000-000000000001",
            vec![("case-a", 0.3, false)],
            0.0,
            0.3,
        );
        let current = make_report(
            "00000000-0000-0000-0000-000000000002",
            vec![("case-a", 0.8, true)],
            1.0,
            0.8,
        );

        let cmp = compare_runs(&baseline, &current);
        assert_eq!(cmp.improvements.len(), 1);
        assert_eq!(cmp.improvements[0].eval_case_id, "case-a");
        assert!(cmp.regressions.is_empty());
    }

    #[test]
    fn compare_detects_new_and_removed_cases() {
        let baseline = make_report(
            "00000000-0000-0000-0000-000000000001",
            vec![("old-case", 0.8, true)],
            1.0,
            0.8,
        );
        let current = make_report(
            "00000000-0000-0000-0000-000000000002",
            vec![("new-case", 0.7, true)],
            1.0,
            0.7,
        );

        let cmp = compare_runs(&baseline, &current);
        assert_eq!(cmp.new_cases, vec!["new-case"]);
        assert_eq!(cmp.removed_cases, vec!["old-case"]);
        assert!(cmp.regressions.is_empty());
        assert!(cmp.improvements.is_empty());
    }

    #[test]
    fn compare_stable_when_outcomes_unchanged() {
        let baseline = make_report(
            "00000000-0000-0000-0000-000000000001",
            vec![("a", 0.8, true), ("b", 0.2, false)],
            0.5,
            0.5,
        );
        let current = make_report(
            "00000000-0000-0000-0000-000000000002",
            vec![("a", 0.9, true), ("b", 0.15, false)],
            0.5,
            0.525,
        );

        let cmp = compare_runs(&baseline, &current);
        assert!(cmp.regressions.is_empty());
        assert!(cmp.improvements.is_empty());
        assert_eq!(cmp.stable.len(), 2);
    }
}
