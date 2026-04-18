//! Eval case registry — YAML loader and Postgres persistence.
//!
//! # Example
//! ```no_run
//! use std::path::Path;
//! use rustbrain_benchmarker::registry;
//!
//! let cases = registry::load_suite(Path::new("eval/cases/rust-brain-v1.yaml")).unwrap();
//! ```

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[cfg(feature = "db")]
use sqlx::PgPool;

// =============================================================================
// Types
// =============================================================================

/// A single evaluation case describing one PR that should be judged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalCase {
    /// Unique string identifier within the suite (e.g. `"add-workspace-endpoint"`).
    pub id: String,
    /// GitHub repository in `owner/repo` form.
    pub repo: String,
    /// PR number to validate.
    pub pr: u32,
    /// Expected outcome when the validator runs against this PR.
    pub expected_outcome: ExpectedOutcome,
    /// Relative importance weight (default 1.0).
    #[serde(default = "default_weight")]
    pub weight: f64,
    /// Free-form tags for filtering (e.g. `["security", "neo4j"]`).
    #[serde(default)]
    pub tags: Vec<String>,
}

/// The expected validation outcome for a PR.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExpectedOutcome {
    Pass,
    Reject,
}

impl ExpectedOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            ExpectedOutcome::Pass => "pass",
            ExpectedOutcome::Reject => "reject",
        }
    }
}

impl std::fmt::Display for ExpectedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn default_weight() -> f64 {
    1.0
}

/// Top-level YAML envelope.
#[derive(Debug, Deserialize)]
struct SuiteFile {
    cases: Vec<EvalCase>,
}

// =============================================================================
// YAML loader
// =============================================================================

/// Parse a YAML suite file and return its eval cases.
///
/// Returns an error if the file cannot be read or the YAML is malformed.
pub fn load_suite(path: &Path) -> Result<Vec<EvalCase>> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let suite: SuiteFile =
        serde_yaml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    Ok(suite.cases)
}

// =============================================================================
// Postgres persistence (gated on the "db" feature so unit tests compile without
// a live database)
// =============================================================================

/// Upsert `cases` into the `eval_cases` table for `suite_name`.
///
/// Existing rows with the same `id` are updated in-place (no duplicates on
/// re-run). Returns the number of rows affected.
#[cfg(feature = "db")]
pub async fn sync_to_db(pool: &PgPool, suite_name: &str, cases: &[EvalCase]) -> Result<usize> {
    let mut affected = 0usize;

    for case in cases {
        let rows = sqlx::query(
            r#"
            INSERT INTO eval_cases
                (id, repo, pr_number, expected_outcome, weight, tags, suite_name)
            VALUES
                ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (id) DO UPDATE SET
                repo             = EXCLUDED.repo,
                pr_number        = EXCLUDED.pr_number,
                expected_outcome = EXCLUDED.expected_outcome,
                weight           = EXCLUDED.weight,
                tags             = EXCLUDED.tags,
                suite_name       = EXCLUDED.suite_name
            "#,
        )
        .bind(&case.id)
        .bind(&case.repo)
        .bind(case.pr as i32)
        .bind(case.expected_outcome.as_str())
        .bind(case.weight)
        .bind(&case.tags)
        .bind(suite_name)
        .execute(pool)
        .await
        .with_context(|| format!("upserting eval case {}", case.id))?;

        affected += rows.rows_affected() as usize;
    }

    Ok(affected)
}

/// Fetch all eval cases registered for `suite_name` from the database.
#[cfg(feature = "db")]
pub async fn list_suite(pool: &PgPool, suite_name: &str) -> Result<Vec<EvalCase>> {
    type DbRow = (String, String, i32, String, f64, Vec<String>);

    let rows = sqlx::query_as::<_, DbRow>(
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
    .with_context(|| format!("listing eval cases for suite {suite_name}"))?;

    rows.into_iter()
        .map(|r| {
            let expected_outcome = match r.3.as_str() {
                "pass" => ExpectedOutcome::Pass,
                "reject" => ExpectedOutcome::Reject,
                other => anyhow::bail!("unknown expected_outcome: {other}"),
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

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::NamedTempFile;

    fn write_yaml(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("temp file");
        f.write_all(content.as_bytes()).expect("write yaml");
        f
    }

    // -------------------------------------------------------------------------
    // load_suite — happy paths
    // -------------------------------------------------------------------------

    #[test]
    fn load_suite_parses_minimal_case() {
        let yaml = r#"
cases:
  - id: "minimal"
    repo: "org/repo"
    pr: 1
    expected_outcome: pass
"#;
        let f = write_yaml(yaml);
        let cases = load_suite(f.path()).expect("should parse");
        assert_eq!(cases.len(), 1);
        let c = &cases[0];
        assert_eq!(c.id, "minimal");
        assert_eq!(c.repo, "org/repo");
        assert_eq!(c.pr, 1);
        assert_eq!(c.expected_outcome, ExpectedOutcome::Pass);
        assert_eq!(c.weight, 1.0, "default weight must be 1.0");
        assert!(c.tags.is_empty(), "default tags must be empty");
    }

    #[test]
    fn load_suite_parses_full_case() {
        let yaml = r#"
cases:
  - id: "full-case"
    repo: "org/rust-brain"
    pr: 42
    expected_outcome: reject
    weight: 0.5
    tags: [security, neo4j]
"#;
        let f = write_yaml(yaml);
        let cases = load_suite(f.path()).expect("should parse");
        assert_eq!(cases.len(), 1);
        let c = &cases[0];
        assert_eq!(c.expected_outcome, ExpectedOutcome::Reject);
        assert_eq!(c.weight, 0.5);
        assert_eq!(c.tags, vec!["security", "neo4j"]);
    }

    #[test]
    fn load_suite_parses_multiple_cases() {
        let yaml = r#"
cases:
  - id: "case-a"
    repo: "org/a"
    pr: 1
    expected_outcome: pass
  - id: "case-b"
    repo: "org/b"
    pr: 2
    expected_outcome: reject
    weight: 2.0
    tags: [api]
"#;
        let f = write_yaml(yaml);
        let cases = load_suite(f.path()).expect("should parse");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].id, "case-a");
        assert_eq!(cases[1].id, "case-b");
        assert_eq!(cases[1].weight, 2.0);
    }

    #[test]
    fn load_suite_accepts_empty_cases_list() {
        let yaml = "cases: []\n";
        let f = write_yaml(yaml);
        let cases = load_suite(f.path()).expect("should parse empty list");
        assert!(cases.is_empty());
    }

    // -------------------------------------------------------------------------
    // load_suite — error paths
    // -------------------------------------------------------------------------

    #[test]
    fn load_suite_errors_on_missing_file() {
        let result = load_suite(Path::new("/nonexistent/path/suite.yaml"));
        assert!(result.is_err(), "should fail on missing file");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("reading"),
            "error should mention reading: {msg}"
        );
    }

    #[test]
    fn load_suite_errors_on_malformed_yaml() {
        let yaml = "cases: [: not valid yaml\n";
        let f = write_yaml(yaml);
        let result = load_suite(f.path());
        assert!(result.is_err(), "should fail on malformed YAML");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("parsing"),
            "error should mention parsing: {msg}"
        );
    }

    #[test]
    fn load_suite_errors_on_unknown_expected_outcome() {
        let yaml = r#"
cases:
  - id: "bad"
    repo: "org/r"
    pr: 1
    expected_outcome: unknown_value
"#;
        let f = write_yaml(yaml);
        let result = load_suite(f.path());
        assert!(result.is_err(), "unknown expected_outcome must be rejected");
    }

    // -------------------------------------------------------------------------
    // ExpectedOutcome helpers
    // -------------------------------------------------------------------------

    #[test]
    fn expected_outcome_as_str() {
        assert_eq!(ExpectedOutcome::Pass.as_str(), "pass");
        assert_eq!(ExpectedOutcome::Reject.as_str(), "reject");
    }

    #[test]
    fn expected_outcome_display() {
        assert_eq!(ExpectedOutcome::Pass.to_string(), "pass");
        assert_eq!(ExpectedOutcome::Reject.to_string(), "reject");
    }

    // -------------------------------------------------------------------------
    // Round-trip serialization
    // -------------------------------------------------------------------------

    #[test]
    fn eval_case_roundtrips_json() {
        let original = EvalCase {
            id: "round-trip".into(),
            repo: "org/repo".into(),
            pr: 99,
            expected_outcome: ExpectedOutcome::Pass,
            weight: 1.5,
            tags: vec!["tag1".into()],
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: EvalCase = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, restored);
    }
}
