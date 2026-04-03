//! Shared types for the validator pipeline.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use crate::judge::{DimensionScore, JudgeOutput};

// =============================================================================
// PR extraction
// =============================================================================

/// Extracted requirements text from a pull request.
///
/// Either the PR body or a concatenation of title + commit messages when the
/// body is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementsText {
    /// The resolved requirements text fed to the executor.
    pub text: String,
    /// True when the body was empty and we fell back to title + commits.
    pub used_fallback: bool,
}

// =============================================================================
// Diff comparison
// =============================================================================

/// A single contiguous change block within a file diff.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hunk {
    /// Starting line in the old file (1-based).
    pub old_start: u32,
    /// Number of lines in the old file.
    pub old_count: u32,
    /// Starting line in the new file (1-based).
    pub new_start: u32,
    /// Number of lines in the new file.
    pub new_count: u32,
    /// Raw lines of the hunk (including `+`/`-`/` ` prefixes).
    pub lines: Vec<String>,
}

/// A parsed file-level diff (all hunks for one file).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FilePatch {
    /// Relative file path (e.g. `src/main.rs`).
    pub path: String,
    /// All hunks belonging to this file.
    pub hunks: Vec<Hunk>,
    /// Total added lines across all hunks.
    pub lines_added: u32,
    /// Total removed lines across all hunks.
    pub lines_removed: u32,
}

/// Structural comparison result between expected and actual diffs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonResult {
    /// Fraction of actual-diff files that overlap with expected-diff files.
    ///
    /// `|expected ∩ actual| / |actual|`. Returns 0.0 when `actual` is empty.
    pub file_precision: f64,
    /// Fraction of expected-diff files that appear in the actual diff.
    ///
    /// `|expected ∩ actual| / |expected|`. Returns 0.0 when `expected` is empty.
    pub file_recall: f64,
    /// Average Jaro-Winkler similarity of added lines across matching files.
    ///
    /// Ranges from 0.0 (no similarity) to 1.0 (identical).
    pub line_similarity: f64,
    /// Files present in either diff that are not `.rs` files.
    pub non_rust_files: Vec<String>,
}

/// Result of a single run of the validator against a PR.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    /// Zero-indexed run number (0 = first run, 1 = second, etc.).
    pub run_index: u8,
    /// Per-dimension scores from the LLM judge.
    pub dimensions: Vec<DimensionScore>,
    /// Weighted composite score (1.0–5.0).
    pub composite: f32,
    /// Whether this run passed the threshold.
    pub pass: bool,
    /// Approximate tokens used (prompt + completion).
    pub tokens_used: u32,
    /// Estimated cost in USD.
    pub cost_usd: f32,
}

/// Aggregate result across all runs for a single PR validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// UUID assigned at save time (None before persisted).
    pub id: Option<Uuid>,
    /// GitHub repository in `owner/repo` form.
    pub repo: String,
    /// PR number that was validated.
    pub pr_number: u32,
    /// Individual run results.
    pub runs: Vec<RunResult>,
    /// Mean composite score across all runs.
    pub mean_composite: f32,
    /// Standard deviation of composite scores.
    pub std_dev: f32,
    /// Overall pass/fail (mean passes threshold).
    pub pass: bool,
    /// True when the PR was expected to be rejected (inverted rubric).
    pub inverted: bool,
    /// Total cost across all runs.
    pub cost_usd: f32,
}
