//! Composite score computation and statistics across multiple runs.
//!
//! The composite score is a weighted average of 6 dimensions (see [`judge::WEIGHTS`]).
//! Statistics (mean, stddev) are computed across all runs for a PR.

use crate::judge::{DIMENSIONS, INVERTED_PASS_THRESHOLD, PASS_THRESHOLD};
use crate::models::{RunResult, ValidationResult};

/// Compute mean composite and standard deviation from a set of run results.
///
/// Returns `(mean, std_dev)`. Returns `(0.0, 0.0)` for empty input.
pub fn compute_stats(runs: &[RunResult]) -> (f32, f32) {
    if runs.is_empty() {
        return (0.0, 0.0);
    }

    let composites: Vec<f32> = runs.iter().map(|r| r.composite).collect();
    let mean = composites.iter().sum::<f32>() / composites.len() as f32;

    let variance =
        composites.iter().map(|c| (c - mean).powi(2)).sum::<f32>() / composites.len() as f32;

    let std_dev = variance.sqrt();
    (mean, std_dev)
}

/// Aggregate run results into a [`ValidationResult`].
///
/// - `repo` — GitHub `owner/repo` string
/// - `pr_number` — PR number
/// - `runs` — individual run results (must be non-empty)
/// - `inverted` — true if this PR was expected to be rejected
///
/// # Panics
///
/// Panics if `runs` is empty.
pub fn aggregate(
    repo: String,
    pr_number: u32,
    runs: Vec<RunResult>,
    inverted: bool,
) -> ValidationResult {
    assert!(!runs.is_empty(), "Cannot aggregate empty run list");

    let (mean_composite, std_dev) = compute_stats(&runs);

    let pass = if inverted {
        mean_composite < INVERTED_PASS_THRESHOLD
    } else {
        mean_composite >= PASS_THRESHOLD
    };

    let cost_usd: f32 = runs.iter().map(|r| r.cost_usd).sum();

    ValidationResult {
        id: None,
        repo,
        pr_number,
        mean_composite,
        std_dev,
        pass,
        inverted,
        cost_usd,
        runs,
    }
}

/// Compute per-dimension statistics across multiple runs.
///
/// Returns a list of `(dimension_name, mean_score, std_dev)`.
pub fn dimension_stats(runs: &[RunResult]) -> Vec<(String, f32, f32)> {
    DIMENSIONS
        .iter()
        .map(|&dim| {
            let scores: Vec<f32> = runs
                .iter()
                .flat_map(|r| r.dimensions.iter())
                .filter(|d| d.dimension == dim)
                .map(|d| d.score)
                .collect();

            let mean = if scores.is_empty() {
                0.0
            } else {
                scores.iter().sum::<f32>() / scores.len() as f32
            };

            let variance = scores
                .iter()
                .map(|s| (s - mean).powi(2))
                .sum::<f32>()
                .checked_div(scores.len() as f32)
                .unwrap_or(0.0);

            (dim.to_string(), mean, variance.sqrt())
        })
        .collect()
}

// Extend f32 with checked_div since it doesn't exist natively
trait CheckedDiv: Sized {
    fn checked_div(self, other: f32) -> Option<f32>;
}

impl CheckedDiv for f32 {
    fn checked_div(self, other: f32) -> Option<f32> {
        if other == 0.0 {
            None
        } else {
            Some(self / other)
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::{compute_composite, DimensionScore, DIMENSIONS};

    fn make_run(index: u8, scores: &[f32]) -> RunResult {
        let dimensions: Vec<DimensionScore> = DIMENSIONS
            .iter()
            .zip(scores.iter())
            .map(|(&d, &s)| DimensionScore {
                dimension: d.to_string(),
                score: s,
                reasoning: String::new(),
            })
            .collect();
        let composite = compute_composite(&dimensions);
        RunResult {
            run_index: index,
            dimensions,
            composite,
            pass: composite >= PASS_THRESHOLD,
            tokens_used: 1000,
            cost_usd: 0.01,
        }
    }

    #[test]
    fn compute_stats_single_run() {
        let runs = vec![make_run(0, &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0])];
        let (mean, std_dev) = compute_stats(&runs);
        assert!((mean - 4.0).abs() < 1e-4, "Mean should be 4.0, got {mean}");
        assert!(
            std_dev.abs() < 1e-4,
            "Stddev of 1 run should be 0, got {std_dev}"
        );
    }

    #[test]
    fn compute_stats_two_runs() {
        let runs = vec![
            make_run(0, &[5.0, 5.0, 5.0, 5.0, 5.0, 5.0]), // composite = 5.0
            make_run(1, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0]), // composite = 1.0
        ];
        let (mean, _) = compute_stats(&runs);
        assert!(
            (mean - 3.0).abs() < 1e-4,
            "Mean of 5.0 and 1.0 should be 3.0, got {mean}"
        );
    }

    #[test]
    fn compute_stats_empty_returns_zeros() {
        let (mean, std_dev) = compute_stats(&[]);
        assert_eq!(mean, 0.0);
        assert_eq!(std_dev, 0.0);
    }

    #[test]
    fn aggregate_pass_normal() {
        let runs = vec![make_run(0, &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0])];
        let result = aggregate("org/repo".to_string(), 42, runs, false);
        assert!(result.pass, "Score 4.0 should pass normal threshold 3.0");
        assert_eq!(result.pr_number, 42);
        assert!(!result.inverted);
    }

    #[test]
    fn aggregate_fail_normal() {
        let runs = vec![make_run(0, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0])];
        let result = aggregate("org/repo".to_string(), 42, runs, false);
        assert!(!result.pass, "Score 1.0 should fail normal threshold 3.0");
    }

    #[test]
    fn aggregate_pass_inverted() {
        let runs = vec![make_run(0, &[1.0, 1.0, 1.0, 1.0, 1.0, 1.0])];
        let result = aggregate("org/repo".to_string(), 7, runs, true);
        assert!(
            result.pass,
            "Score 1.0 should pass inverted threshold (< 2.0)"
        );
        assert!(result.inverted);
    }

    #[test]
    fn aggregate_fail_inverted() {
        let runs = vec![make_run(0, &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0])];
        let result = aggregate("org/repo".to_string(), 7, runs, true);
        assert!(!result.pass, "Score 4.0 should fail inverted threshold");
    }

    #[test]
    fn aggregate_cost_summed() {
        let runs = vec![
            make_run(0, &[3.0, 3.0, 3.0, 3.0, 3.0, 3.0]),
            make_run(1, &[3.0, 3.0, 3.0, 3.0, 3.0, 3.0]),
        ];
        let result = aggregate("org/repo".to_string(), 1, runs, false);
        assert!(
            (result.cost_usd - 0.02).abs() < 1e-5,
            "Total cost should be 0.02"
        );
    }

    #[test]
    fn dimension_stats_consistent() {
        let runs = vec![
            make_run(0, &[3.0, 4.0, 5.0, 2.0, 3.0, 4.0]),
            make_run(1, &[5.0, 2.0, 3.0, 4.0, 5.0, 2.0]),
        ];
        let stats = dimension_stats(&runs);
        assert_eq!(stats.len(), 6);

        // File Precision: mean of 3.0 and 5.0 = 4.0
        let fp = stats
            .iter()
            .find(|(d, _, _)| d == "File Precision")
            .unwrap();
        assert!(
            (fp.1 - 4.0).abs() < 1e-4,
            "File Precision mean should be 4.0, got {}",
            fp.1
        );
    }
}
