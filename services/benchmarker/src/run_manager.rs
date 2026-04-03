//! Run manager — parallel suite executor with progress tracking and cost aggregation.
//!
//! [`run_suite`] orchestrates parallel execution of eval cases against the
//! `validator` binary. Up to `max_concurrent` cases run at the same time via a
//! [`tokio::sync::Semaphore`]. Each case is evaluated `runs_per_case` times;
//! results are stored in `bench_case_results` and progress is written to
//! `bench_runs` after every completed case.
//!
//! # Environment Variables
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `VALIDATOR_BIN` | `"validator"` | Path to the validator binary |
//! | `VALIDATOR_TIMEOUT` | `3600` | Per-run timeout in seconds |
//! | `OPENCODE_URL` | `"http://opencode:4096"` | OpenCode base URL forwarded to validator |
//! | `GITHUB_TOKEN` | — | GitHub auth token forwarded to validator |

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::registry::EvalCase;

// =============================================================================
// Public types
// =============================================================================

/// Aggregate result of a full benchmark suite run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRunResult {
    /// ID of the `bench_runs` row created for this execution.
    pub bench_run_id: Uuid,
    /// Suite name that was executed.
    pub suite_name: String,
    /// Number of eval cases in the suite.
    pub total_cases: usize,
    /// Number of cases that completed (may be less than total on errors).
    pub completed_cases: usize,
    /// Cases whose mean composite score exceeded the pass threshold.
    pub pass_count: usize,
    /// `pass_count / completed_cases`, or 0.0 if nothing completed.
    pub pass_rate: f64,
    /// Mean composite score across all completed case runs.
    pub mean_composite: f64,
    /// Total cost across every individual validator run.
    pub total_cost_usd: f64,
    /// Per-case results.
    pub case_results: Vec<CaseRunResult>,
}

/// Result for a single eval-case run attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRunResult {
    /// `eval_cases.id` value.
    pub eval_case_id: String,
    /// Zero-indexed run number within this case (0 = first, 1 = second, …).
    pub run_index: u8,
    /// `bench_case_results.id` assigned on DB insertion.
    pub bench_case_result_id: Uuid,
    /// Composite score in `[0.0, 1.0]`.
    pub composite: f64,
    /// Whether this individual run passed.
    pub pass: bool,
    /// Estimated cost in USD for this run.
    pub cost_usd: f64,
}

// =============================================================================
// Internal: validator subprocess output
// =============================================================================

/// JSON structure emitted by `validator validate --json` per run.
///
/// The validator prints one JSON object per line (one per `--runs N` run).
#[derive(Debug, Deserialize)]
struct ValidatorJsonLine {
    /// Fraction of actual files that match expected files (0.0 – 1.0).
    file_precision: f64,
    /// Fraction of expected files present in actual diff (0.0 – 1.0).
    file_recall: f64,
    /// Average Jaro-Winkler line similarity across matching files (0.0 – 1.0).
    line_similarity: f64,
}

impl ValidatorJsonLine {
    /// Compute a composite score in `[0.0, 1.0]` from the structural metrics.
    ///
    /// Weights: file_precision 30%, file_recall 30%, line_similarity 40%.
    fn composite(&self) -> f64 {
        self.file_precision * 0.30 + self.file_recall * 0.30 + self.line_similarity * 0.40
    }

    /// Determine pass/fail given the expected outcome.
    ///
    /// - `pass` expected: composite ≥ 0.50
    /// - `reject` expected (inverted): composite < 0.35 (low quality = correctly rejected)
    fn pass(&self, inverted: bool) -> bool {
        if inverted {
            self.composite() < 0.35
        } else {
            self.composite() >= 0.50
        }
    }
}

// =============================================================================
// Internal: config from env
// =============================================================================

#[derive(Debug, Clone)]
struct RunConfig {
    validator_bin: String,
    timeout_secs: u64,
    opencode_url: String,
    github_token: Option<String>,
}

impl RunConfig {
    fn from_env() -> Self {
        Self {
            validator_bin: std::env::var("VALIDATOR_BIN")
                .unwrap_or_else(|_| "validator".to_string()),
            timeout_secs: std::env::var("VALIDATOR_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
            opencode_url: std::env::var("OPENCODE_URL")
                .unwrap_or_else(|_| "http://opencode:4096".to_string()),
            github_token: std::env::var("GITHUB_TOKEN").ok(),
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Execute a full benchmark suite and persist results.
///
/// # Arguments
///
/// - `pool` — Postgres connection pool (all tables assumed migrated)
/// - `suite_name` — name of the eval suite (matches `eval_cases.suite_name`)
/// - `runs_per_case` — how many independent runs to execute per eval case
/// - `max_concurrent` — maximum number of cases running at the same time
/// - `release_tag` — optional release label stored on the `bench_runs` row
///
/// # Errors
///
/// Returns an error if the suite has no registered cases or if the initial
/// DB insert fails. Per-case validator failures are logged but do not abort
/// the suite.
pub async fn run_suite(
    pool: &PgPool,
    suite_name: &str,
    runs_per_case: u32,
    max_concurrent: usize,
    release_tag: Option<&str>,
) -> Result<BenchRunResult> {
    let config = RunConfig::from_env();

    // Load eval cases from DB
    let cases = load_eval_cases(pool, suite_name).await?;
    if cases.is_empty() {
        anyhow::bail!(
            "No eval cases found for suite '{suite_name}'. Run `benchmarker sync` first."
        );
    }

    let total_cases = cases.len();
    info!(suite = suite_name, total_cases, "Starting benchmark suite");

    // Create the bench_run row
    let bench_run_id = create_bench_run(pool, suite_name, total_cases, release_tag).await?;
    info!(bench_run_id = %bench_run_id, "Created bench_run row");

    // Parallel execution with semaphore
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let config = Arc::new(config);
    let pool = pool.clone();

    let mut join_set = tokio::task::JoinSet::new();

    for case in cases {
        let sem = Arc::clone(&semaphore);
        let cfg = Arc::clone(&config);
        let pool = pool.clone();

        join_set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            run_case(&pool, bench_run_id, &case, runs_per_case, &cfg).await
        });
    }

    // Collect results
    let mut all_case_results: Vec<CaseRunResult> = Vec::new();
    let mut completed_cases = 0usize;
    let mut pass_count = 0usize;
    let mut total_cost_usd = 0.0f64;

    while let Some(join_result) = join_set.join_next().await {
        match join_result {
            Ok(Ok(case_results)) => {
                completed_cases += 1;
                let case_passes = case_results.iter().any(|r| r.pass);
                if case_passes {
                    pass_count += 1;
                }
                for r in &case_results {
                    total_cost_usd += r.cost_usd;
                }
                all_case_results.extend(case_results);
            }
            Ok(Err(e)) => {
                error!(error = %e, "Case run failed — skipping");
            }
            Err(e) => {
                error!(error = %e, "Join error for case run task");
            }
        }

        // Periodic progress update after each completed case
        let composites: Vec<f64> = all_case_results.iter().map(|r| r.composite).collect();
        let mean_composite = if composites.is_empty() {
            0.0
        } else {
            composites.iter().sum::<f64>() / composites.len() as f64
        };

        if let Err(e) = update_bench_run_progress(
            &pool,
            bench_run_id,
            completed_cases,
            pass_count,
            total_cost_usd,
            mean_composite,
        )
        .await
        {
            warn!(error = %e, "Failed to update bench_run progress");
        }
    }

    // Final stats
    let composites: Vec<f64> = all_case_results.iter().map(|r| r.composite).collect();
    let mean_composite = if composites.is_empty() {
        0.0
    } else {
        composites.iter().sum::<f64>() / composites.len() as f64
    };
    let pass_rate = if completed_cases == 0 {
        0.0
    } else {
        pass_count as f64 / completed_cases as f64
    };

    // Mark bench_run as completed
    finalize_bench_run(
        &pool,
        bench_run_id,
        completed_cases,
        pass_count,
        pass_rate,
        mean_composite,
        total_cost_usd,
    )
    .await?;

    info!(
        bench_run_id = %bench_run_id,
        completed_cases,
        pass_count,
        pass_rate,
        mean_composite,
        total_cost_usd,
        "Benchmark suite complete"
    );

    Ok(BenchRunResult {
        bench_run_id,
        suite_name: suite_name.to_string(),
        total_cases,
        completed_cases,
        pass_count,
        pass_rate,
        mean_composite,
        total_cost_usd,
        case_results: all_case_results,
    })
}

// =============================================================================
// Internal: per-case execution
// =============================================================================

/// Run a single eval case `runs_per_case` times sequentially, storing each
/// result in `bench_case_results`.
async fn run_case(
    pool: &PgPool,
    bench_run_id: Uuid,
    case: &EvalCase,
    runs_per_case: u32,
    config: &RunConfig,
) -> Result<Vec<CaseRunResult>> {
    info!(
        case_id = %case.id,
        repo = %case.repo,
        pr = case.pr,
        runs_per_case,
        "Running eval case"
    );

    let inverted = case.expected_outcome == crate::registry::ExpectedOutcome::Reject;
    let mut results = Vec::new();

    for run_idx in 0..runs_per_case {
        match run_validator(config, &case.repo, case.pr, inverted).await {
            Ok((composite, pass, cost_usd)) => {
                let bench_case_result_id = store_case_result(
                    pool,
                    bench_run_id,
                    &case.id,
                    run_idx as i16,
                    composite,
                    pass,
                    cost_usd,
                )
                .await?;

                info!(
                    case_id = %case.id,
                    run_idx,
                    composite,
                    pass,
                    cost_usd,
                    "Case run stored"
                );

                results.push(CaseRunResult {
                    eval_case_id: case.id.clone(),
                    run_index: run_idx as u8,
                    bench_case_result_id,
                    composite,
                    pass,
                    cost_usd,
                });
            }
            Err(e) => {
                warn!(
                    case_id = %case.id,
                    run_idx,
                    error = %e,
                    "Validator run failed — marking as failed result"
                );
                // Store a zero-score placeholder so the run is counted
                let bench_case_result_id = store_case_result(
                    pool,
                    bench_run_id,
                    &case.id,
                    run_idx as i16,
                    0.0,
                    false,
                    0.0,
                )
                .await?;
                results.push(CaseRunResult {
                    eval_case_id: case.id.clone(),
                    run_index: run_idx as u8,
                    bench_case_result_id,
                    composite: 0.0,
                    pass: false,
                    cost_usd: 0.0,
                });
            }
        }
    }

    Ok(results)
}

/// Invoke the `validator validate` binary for one PR and parse the JSON output.
///
/// Returns `(composite, pass, cost_usd)`.
async fn run_validator(
    config: &RunConfig,
    repo: &str,
    pr: u32,
    inverted: bool,
) -> Result<(f64, bool, f64)> {
    let mut cmd = tokio::process::Command::new(&config.validator_bin);
    cmd.args([
        "validate",
        repo,
        &pr.to_string(),
        "--runs",
        "1",
        "--json",
        "--timeout",
        &config.timeout_secs.to_string(),
        "--opencode-url",
        &config.opencode_url,
    ]);

    if inverted {
        cmd.arg("--inverted");
    }

    if let Some(token) = &config.github_token {
        cmd.env("GITHUB_TOKEN", token);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(config.timeout_secs + 60),
        cmd.output(),
    )
    .await
    .context("Validator invocation timed out")?
    .context("Failed to spawn validator binary")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "validator exited {:?}: {}",
            output.status.code(),
            stderr.trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_validator_output(&stdout, inverted)
}

/// Parse the validator's JSON output (one JSON object per line, one per run).
///
/// We take the first line; each invocation uses `--runs 1`.
fn parse_validator_output(stdout: &str, inverted: bool) -> Result<(f64, bool, f64)> {
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with('{'))
        .ok_or_else(|| anyhow::anyhow!("No JSON output from validator:\n{stdout}"))?;

    let parsed: ValidatorJsonLine =
        serde_json::from_str(line.trim()).context("Parsing validator JSON output")?;

    let composite = parsed.composite();
    let pass = parsed.pass(inverted);

    // Cost estimation: validator runs are free in structural-comparison mode
    // (no LLM calls in the ComparisonResult path); set to 0.0 unless a future
    // judge-integrated output includes a cost field.
    let cost_usd = 0.0f64;

    Ok((composite, pass, cost_usd))
}

// =============================================================================
// Internal: DB helpers
// =============================================================================

async fn load_eval_cases(pool: &PgPool, suite_name: &str) -> Result<Vec<EvalCase>> {
    use crate::registry::ExpectedOutcome;

    type Row = (String, String, i32, String, f64, Vec<String>);

    let rows = sqlx::query_as::<_, Row>(
        r#"
        SELECT id, repo, pr_number, expected_outcome, weight, tags
        FROM eval_cases
        WHERE suite_name = $1
        ORDER BY id
        "#,
    )
    .bind(suite_name)
    .fetch_all(pool)
    .await
    .with_context(|| format!("Loading eval cases for suite '{suite_name}'"))?;

    rows.into_iter()
        .map(|r| {
            let expected_outcome = match r.3.as_str() {
                "pass" => ExpectedOutcome::Pass,
                "reject" => ExpectedOutcome::Reject,
                other => anyhow::bail!("Unknown expected_outcome: {other}"),
            };
            Ok(EvalCase {
                id: r.0,
                repo: r.1,
                pr: r.2 as u32,
                expected_outcome,
                weight: r.4,
                tags: r.5,
            })
        })
        .collect()
}

async fn create_bench_run(
    pool: &PgPool,
    suite_name: &str,
    total_cases: usize,
    release_tag: Option<&str>,
) -> Result<Uuid> {
    let (id,) = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO bench_runs (suite_name, release_tag, total_cases, status)
        VALUES ($1, $2, $3, 'running')
        RETURNING id
        "#,
    )
    .bind(suite_name)
    .bind(release_tag)
    .bind(total_cases as i32)
    .fetch_one(pool)
    .await
    .context("Creating bench_run row")?;

    Ok(id)
}

async fn update_bench_run_progress(
    pool: &PgPool,
    bench_run_id: Uuid,
    completed_cases: usize,
    pass_count: usize,
    total_cost_usd: f64,
    mean_composite: f64,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE bench_runs
        SET completed_cases  = $2,
            pass_count       = $3,
            total_cost_usd   = $4,
            mean_composite   = $5
        WHERE id = $1
        "#,
    )
    .bind(bench_run_id)
    .bind(completed_cases as i32)
    .bind(pass_count as i32)
    .bind(total_cost_usd)
    .bind(mean_composite)
    .execute(pool)
    .await
    .context("Updating bench_run progress")?;

    Ok(())
}

async fn finalize_bench_run(
    pool: &PgPool,
    bench_run_id: Uuid,
    completed_cases: usize,
    pass_count: usize,
    pass_rate: f64,
    mean_composite: f64,
    total_cost_usd: f64,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE bench_runs
        SET status           = 'completed',
            completed_cases  = $2,
            pass_count       = $3,
            pass_rate        = $4,
            mean_composite   = $5,
            total_cost_usd   = $6,
            completed_at     = NOW()
        WHERE id = $1
        "#,
    )
    .bind(bench_run_id)
    .bind(completed_cases as i32)
    .bind(pass_count as i32)
    .bind(pass_rate)
    .bind(mean_composite)
    .bind(total_cost_usd)
    .execute(pool)
    .await
    .context("Finalizing bench_run")?;

    Ok(())
}

async fn store_case_result(
    pool: &PgPool,
    bench_run_id: Uuid,
    eval_case_id: &str,
    run_index: i16,
    composite: f64,
    pass: bool,
    cost_usd: f64,
) -> Result<Uuid> {
    let (id,) = sqlx::query_as::<_, (Uuid,)>(
        r#"
        INSERT INTO bench_case_results
            (bench_run_id, eval_case_id, run_index, composite, pass, cost_usd)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(bench_run_id)
    .bind(eval_case_id)
    .bind(run_index)
    .bind(composite)
    .bind(pass)
    .bind(cost_usd)
    .fetch_one(pool)
    .await
    .with_context(|| format!("Storing case result for case {eval_case_id} run {run_index}"))?;

    Ok(id)
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // ValidatorJsonLine scoring
    // -------------------------------------------------------------------------

    fn make_line(fp: f64, fr: f64, ls: f64) -> ValidatorJsonLine {
        ValidatorJsonLine {
            file_precision: fp,
            file_recall: fr,
            line_similarity: ls,
        }
    }

    #[test]
    fn composite_zero_on_empty_diff() {
        let line = make_line(0.0, 0.0, 0.0);
        assert!((line.composite() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn composite_one_on_perfect_match() {
        let line = make_line(1.0, 1.0, 1.0);
        assert!((line.composite() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn composite_weighted_correctly() {
        // 0.30 * 0.8 + 0.30 * 0.6 + 0.40 * 0.5 = 0.24 + 0.18 + 0.20 = 0.62
        let line = make_line(0.8, 0.6, 0.5);
        let expected = 0.30 * 0.8 + 0.30 * 0.6 + 0.40 * 0.5;
        assert!(
            (line.composite() - expected).abs() < 1e-9,
            "composite mismatch: {} vs {}",
            line.composite(),
            expected
        );
    }

    #[test]
    fn pass_normal_above_threshold() {
        let line = make_line(0.7, 0.7, 0.7);
        // composite = 0.7 >= 0.50 → pass
        assert!(line.pass(false));
    }

    #[test]
    fn fail_normal_below_threshold() {
        let line = make_line(0.2, 0.2, 0.2);
        // composite = 0.2 < 0.50 → fail
        assert!(!line.pass(false));
    }

    #[test]
    fn pass_inverted_below_threshold() {
        let line = make_line(0.1, 0.1, 0.1);
        // composite = 0.1 < 0.35 → pass (correctly rejected)
        assert!(line.pass(true));
    }

    #[test]
    fn fail_inverted_above_threshold() {
        let line = make_line(0.8, 0.8, 0.8);
        // composite = 0.8 >= 0.35 → fail (should have been rejected but wasn't)
        assert!(!line.pass(true));
    }

    // -------------------------------------------------------------------------
    // parse_validator_output
    // -------------------------------------------------------------------------

    #[test]
    fn parses_valid_json_line() {
        let stdout =
            r#"{"file_precision":0.8,"file_recall":0.6,"line_similarity":0.7,"non_rust_files":[]}"#;
        let (composite, pass, cost) = parse_validator_output(stdout, false).unwrap();
        let expected_composite = 0.30 * 0.8 + 0.30 * 0.6 + 0.40 * 0.7;
        assert!((composite - expected_composite).abs() < 1e-9);
        assert!(pass); // composite > 0.50
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn parses_json_with_preamble_lines() {
        let stdout = "INFO starting\n{\"file_precision\":0.0,\"file_recall\":0.0,\"line_similarity\":0.0,\"non_rust_files\":[]}\n";
        let (composite, pass, _) = parse_validator_output(stdout, false).unwrap();
        assert!((composite - 0.0).abs() < 1e-9);
        assert!(!pass);
    }

    #[test]
    fn errors_on_no_json() {
        let result = parse_validator_output("no json here\n", false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("No JSON output"), "unexpected: {msg}");
    }

    #[test]
    fn errors_on_malformed_json() {
        let result = parse_validator_output("{not-valid}\n", false);
        assert!(result.is_err());
    }
}
