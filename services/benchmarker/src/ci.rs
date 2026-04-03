//! CI advisory output — formats benchmark results for CI pipelines.
//!
//! All functions in this module are non-blocking: they return `Ok(())` even
//! when the benchmark run reveals regressions. The caller is always expected
//! to exit with code 0.
//!
//! # Output Formats
//!
//! - [`format_json`]  — machine-parseable JSON (for downstream tooling)
//! - [`format_human`] — ANSI-free text suitable for CI log viewers
//! - [`print_advisory`] — prints both formats to stdout / stderr and always
//!   returns `Ok(())`

use serde::{Deserialize, Serialize};

use crate::reporter::{BenchReport, ReleaseComparison};
use crate::run_manager::BenchRunResult;

// =============================================================================
// CI output types
// =============================================================================

/// Machine-parseable CI advisory payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiAdvisory {
    /// Advisory format version (semver).
    pub version: &'static str,
    /// Benchmark suite name.
    pub suite: String,
    /// Release tag if provided during the run.
    pub release: Option<String>,
    /// Overall pass rate for this run (0.0 – 1.0).
    pub pass_rate: f64,
    /// Mean composite score across all runs.
    pub mean_composite: f64,
    /// Total validator cost in USD.
    pub total_cost_usd: f64,
    /// Number of eval cases that completed.
    pub completed_cases: usize,
    /// Number of passing cases.
    pub pass_count: usize,
    /// Advisory status: `"ok"`, `"degraded"`, or `"failing"`.
    pub status: &'static str,
    /// Human-readable summary line.
    pub summary: String,
    /// Optional regression details (populated when a baseline is provided).
    pub regressions: Vec<CiRegression>,
}

/// Regression entry in the CI advisory payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiRegression {
    /// Eval case identifier.
    pub case_id: String,
    /// Score in the baseline run.
    pub baseline_score: f64,
    /// Score in the current run.
    pub current_score: f64,
    /// Score delta (current − baseline).
    pub delta: f64,
}

// =============================================================================
// Formatters
// =============================================================================

/// Build a [`CiAdvisory`] from a completed [`BenchRunResult`].
///
/// Optionally enriches the advisory with regression data from a
/// [`ReleaseComparison`] when a baseline run was provided.
pub fn build_advisory(
    result: &BenchRunResult,
    release: Option<&str>,
    comparison: Option<&ReleaseComparison>,
) -> CiAdvisory {
    let status = advisory_status(result.pass_rate);

    let summary = format!(
        "Suite '{}': {}/{} cases passed ({:.1}% pass rate, mean composite {:.3})",
        result.suite_name,
        result.pass_count,
        result.completed_cases,
        result.pass_rate * 100.0,
        result.mean_composite,
    );

    let regressions: Vec<CiRegression> = comparison
        .map(|cmp| {
            cmp.regressions
                .iter()
                .map(|r| CiRegression {
                    case_id: r.eval_case_id.clone(),
                    baseline_score: r.baseline_composite,
                    current_score: r.current_composite,
                    delta: r.delta,
                })
                .collect()
        })
        .unwrap_or_default();

    CiAdvisory {
        version: "1.0.0",
        suite: result.suite_name.clone(),
        release: release.map(str::to_string),
        pass_rate: result.pass_rate,
        mean_composite: result.mean_composite,
        total_cost_usd: result.total_cost_usd,
        completed_cases: result.completed_cases,
        pass_count: result.pass_count,
        status,
        summary,
        regressions,
    }
}

/// Determine the advisory status string from the pass rate.
///
/// | Pass rate | Status |
/// |---|---|
/// | ≥ 0.80 | `"ok"` |
/// | ≥ 0.50 | `"degraded"` |
/// | < 0.50 | `"failing"` |
pub fn advisory_status(pass_rate: f64) -> &'static str {
    if pass_rate >= 0.80 {
        "ok"
    } else if pass_rate >= 0.50 {
        "degraded"
    } else {
        "failing"
    }
}

/// Serialize a [`CiAdvisory`] as pretty-printed JSON.
///
/// # Errors
///
/// Propagates [`serde_json`] serialization errors (in practice infallible for
/// this type).
pub fn format_json(advisory: &CiAdvisory) -> anyhow::Result<String> {
    serde_json::to_string_pretty(advisory).map_err(anyhow::Error::from)
}

/// Format a [`CiAdvisory`] as human-readable text for CI log output.
///
/// The output is free of ANSI escape codes so it renders cleanly in any log
/// viewer.
pub fn format_human(advisory: &CiAdvisory) -> String {
    let mut out = String::new();

    out.push_str("=== rustbrain benchmark advisory ===\n");
    out.push_str(&format!("Suite   : {}\n", advisory.suite));
    if let Some(release) = &advisory.release {
        out.push_str(&format!("Release : {release}\n"));
    }
    out.push_str(&format!("Status  : {}\n", advisory.status.to_uppercase()));
    out.push_str(&format!(
        "Results : {}/{} cases passed ({:.1}%)\n",
        advisory.pass_count,
        advisory.completed_cases,
        advisory.pass_rate * 100.0,
    ));
    out.push_str(&format!(
        "Score   : mean composite {:.4}\n",
        advisory.mean_composite,
    ));
    out.push_str(&format!("Cost    : ${:.4} USD\n", advisory.total_cost_usd,));

    if !advisory.regressions.is_empty() {
        out.push_str(&format!("\nREGRESSIONS ({})\n", advisory.regressions.len()));
        out.push_str("  Case ID                          Baseline  Current   Delta\n");
        out.push_str("  ─────────────────────────────────────────────────────────\n");
        for r in &advisory.regressions {
            out.push_str(&format!(
                "  {:<32}  {:.4}    {:.4}    {:+.4}\n",
                &r.case_id[..r.case_id.len().min(32)],
                r.baseline_score,
                r.current_score,
                r.delta,
            ));
        }
    }

    out.push_str("\n[advisory only — this output does not block CI]\n");
    out
}

/// Build advisory from a [`BenchReport`] (for post-run reporting).
///
/// Useful when the [`BenchRunResult`] is not available but the report has been
/// queried from the database.
pub fn build_advisory_from_report(
    report: &BenchReport,
    comparison: Option<&ReleaseComparison>,
) -> CiAdvisory {
    let status = advisory_status(report.pass_rate);

    let summary = format!(
        "Suite '{}': {}/{} cases passed ({:.1}% pass rate, mean composite {:.3})",
        report.suite_name,
        report.pass_count,
        report.total_cases,
        report.pass_rate * 100.0,
        report.mean_composite,
    );

    let regressions: Vec<CiRegression> = comparison
        .map(|cmp| {
            cmp.regressions
                .iter()
                .map(|r| CiRegression {
                    case_id: r.eval_case_id.clone(),
                    baseline_score: r.baseline_composite,
                    current_score: r.current_composite,
                    delta: r.delta,
                })
                .collect()
        })
        .unwrap_or_default();

    CiAdvisory {
        version: "1.0.0",
        suite: report.suite_name.clone(),
        release: report.release_tag.clone(),
        pass_rate: report.pass_rate,
        mean_composite: report.mean_composite,
        total_cost_usd: report.total_cost_usd,
        completed_cases: report.total_cases as usize,
        pass_count: report.pass_count as usize,
        status,
        summary,
        regressions,
    }
}

/// Print the full advisory (JSON to stdout, human-readable to stderr) and
/// return `Ok(())` regardless of benchmark outcome.
///
/// Callers should exit with code 0 after this function returns.
pub fn print_advisory(
    result: &BenchRunResult,
    release: Option<&str>,
    comparison: Option<&ReleaseComparison>,
) -> anyhow::Result<()> {
    let advisory = build_advisory(result, release, comparison);

    // JSON to stdout (parseable by downstream tooling)
    let json = format_json(&advisory)?;
    println!("{json}");

    // Human-readable to stderr (visible in CI logs without polluting stdout)
    eprintln!("{}", format_human(&advisory));

    Ok(())
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_result(pass_rate: f64, pass_count: usize, completed: usize) -> BenchRunResult {
        BenchRunResult {
            bench_run_id: Uuid::new_v4(),
            suite_name: "default".to_string(),
            total_cases: completed,
            completed_cases: completed,
            pass_count,
            pass_rate,
            mean_composite: pass_rate * 0.9,
            total_cost_usd: 0.05,
            case_results: Vec::new(),
        }
    }

    // -------------------------------------------------------------------------
    // advisory_status
    // -------------------------------------------------------------------------

    #[test]
    fn status_ok_at_100_percent() {
        assert_eq!(advisory_status(1.0), "ok");
    }

    #[test]
    fn status_ok_at_80_percent() {
        assert_eq!(advisory_status(0.80), "ok");
    }

    #[test]
    fn status_degraded_at_79_percent() {
        assert_eq!(advisory_status(0.79), "degraded");
    }

    #[test]
    fn status_degraded_at_50_percent() {
        assert_eq!(advisory_status(0.50), "degraded");
    }

    #[test]
    fn status_failing_at_49_percent() {
        assert_eq!(advisory_status(0.49), "failing");
    }

    #[test]
    fn status_failing_at_zero() {
        assert_eq!(advisory_status(0.0), "failing");
    }

    // -------------------------------------------------------------------------
    // build_advisory
    // -------------------------------------------------------------------------

    #[test]
    fn build_advisory_no_regressions_when_no_comparison() {
        let result = make_result(0.9, 9, 10);
        let advisory = build_advisory(&result, Some("v1.0"), None);
        assert!(advisory.regressions.is_empty());
        assert_eq!(advisory.release.as_deref(), Some("v1.0"));
        assert_eq!(advisory.status, "ok");
    }

    #[test]
    fn build_advisory_populates_regressions_from_comparison() {
        use crate::reporter::RegressionEntry;
        let result = make_result(0.6, 6, 10);
        let cmp = ReleaseComparison {
            baseline_run_id: Uuid::new_v4(),
            current_run_id: Uuid::new_v4(),
            pass_rate_delta: -0.2,
            composite_delta: -0.1,
            regressions: vec![RegressionEntry {
                eval_case_id: "case-a".to_string(),
                baseline_composite: 0.8,
                current_composite: 0.3,
                delta: -0.5,
            }],
            improvements: Vec::new(),
            stable: Vec::new(),
            new_cases: Vec::new(),
            removed_cases: Vec::new(),
        };
        let advisory = build_advisory(&result, None, Some(&cmp));
        assert_eq!(advisory.regressions.len(), 1);
        assert_eq!(advisory.regressions[0].case_id, "case-a");
        assert!((advisory.regressions[0].delta - (-0.5)).abs() < 1e-9);
    }

    // -------------------------------------------------------------------------
    // format_human
    // -------------------------------------------------------------------------

    #[test]
    fn format_human_contains_suite_name() {
        let result = make_result(0.7, 7, 10);
        let advisory = build_advisory(&result, None, None);
        let text = format_human(&advisory);
        assert!(text.contains("default"), "missing suite name: {text}");
    }

    #[test]
    fn format_human_contains_advisory_disclaimer() {
        let result = make_result(1.0, 10, 10);
        let advisory = build_advisory(&result, None, None);
        let text = format_human(&advisory);
        assert!(
            text.contains("advisory only"),
            "missing advisory disclaimer: {text}"
        );
    }

    #[test]
    fn format_human_no_ansi_codes() {
        let result = make_result(0.5, 5, 10);
        let advisory = build_advisory(&result, None, None);
        let text = format_human(&advisory);
        assert!(
            !text.contains("\x1b["),
            "output must not contain ANSI escape codes"
        );
    }

    // -------------------------------------------------------------------------
    // format_json
    // -------------------------------------------------------------------------

    #[test]
    fn format_json_is_valid_json() {
        let result = make_result(0.8, 8, 10);
        let advisory = build_advisory(&result, None, None);
        let json = format_json(&advisory).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("must be valid JSON");
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["version"], "1.0.0");
    }

    #[test]
    fn format_json_includes_pass_rate() {
        let result = make_result(0.6, 6, 10);
        let advisory = build_advisory(&result, None, None);
        let json = format_json(&advisory).expect("should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let pr = parsed["pass_rate"].as_f64().unwrap();
        assert!((pr - 0.6).abs() < 1e-9);
    }
}
