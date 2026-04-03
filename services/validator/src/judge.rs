//! LLM-as-judge module for evaluating rust-brain generated code.
//!
//! [`JudgeClient`] calls the litellm proxy (model: `open-large`) with a
//! structured 6-dimension rubric and parses the JSON response into
//! [`JudgeOutput`].
//!
//! Auth: reads `LITELLM_API_KEY` from environment. Base URL: `LITELLM_BASE_URL`.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::warn;

// =============================================================================
// Rubric constants
// =============================================================================

/// Ordered list of scoring dimensions.
pub const DIMENSIONS: [&str; 6] = [
    "File Precision",
    "File Recall",
    "Logical Equivalence",
    "Code Quality",
    "Edge Case Handling",
    "Approach Validity",
];

/// Weights for each dimension (must sum to 1.0).
pub const WEIGHTS: [f32; 6] = [0.15, 0.15, 0.30, 0.20, 0.10, 0.10];

/// Pass threshold for normal (non-inverted) evaluation.
pub const PASS_THRESHOLD: f32 = 3.0;

/// Pass threshold for inverted (expected-reject) evaluation.
pub const INVERTED_PASS_THRESHOLD: f32 = 2.0;

/// Compile-time assertion that weights sum to 1.0 within float tolerance.
///
/// We check at test time since const float operations have limited support.
pub fn assert_weights_sum_to_one() {
    let sum: f32 = WEIGHTS.iter().sum();
    assert!(
        (sum - 1.0).abs() < 1e-5,
        "Dimension weights must sum to 1.0, got {sum}"
    );
}

// =============================================================================
// Types
// =============================================================================

/// Score for a single evaluation dimension.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DimensionScore {
    /// One of the six rubric dimensions.
    pub dimension: String,
    /// Score from 1.0 (worst) to 5.0 (best).
    pub score: f32,
    /// Judge's reasoning for this score.
    pub reasoning: String,
}

/// Complete output from the LLM judge for one evaluation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeOutput {
    /// Per-dimension scores (6 entries, matching [`DIMENSIONS`] order).
    pub dimensions: Vec<DimensionScore>,
    /// Weighted composite score = Σ(score × weight).
    pub composite: f32,
    /// Overall reasoning summary from the judge.
    pub overall_reasoning: String,
    /// Whether this run passes the threshold.
    pub pass: bool,
}

/// Input provided to the LLM judge.
#[derive(Debug, Clone)]
pub struct JudgeInput {
    /// PR title and description (requirements).
    pub pr_description: String,
    /// Linked issue descriptions (additional requirements context).
    pub linked_issues: Vec<String>,
    /// Unified diff of the merged PR (ground truth).
    pub expected_diff: String,
    /// Unified diff produced by rust-brain agent (what to evaluate).
    pub actual_diff: String,
    /// Summary of relevant code context from the repository.
    pub repo_context: String,
    /// When true, the PR was reverted/rejected — low score = pass.
    pub inverted: bool,
}

// =============================================================================
// Client
// =============================================================================

/// LLM judge client using the litellm proxy.
///
/// Configure via environment variables:
/// - `LITELLM_BASE_URL` — e.g. `https://grid.juspay.in/litellm`
/// - `LITELLM_API_KEY` — bearer token
/// - `LITELLM_MODEL` — defaults to `open-large`
#[derive(Clone)]
pub struct JudgeClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl JudgeClient {
    /// Create from environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if `LITELLM_BASE_URL` or `LITELLM_API_KEY` are not set.
    pub fn from_env() -> Result<Self> {
        let base_url = std::env::var("LITELLM_BASE_URL")
            .context("LITELLM_BASE_URL environment variable not set")?;
        let api_key = std::env::var("LITELLM_API_KEY")
            .context("LITELLM_API_KEY environment variable not set")?;
        let model =
            std::env::var("LITELLM_MODEL").unwrap_or_else(|_| "open-large".to_string());

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            api_key,
        })
    }

    /// Run the LLM judge evaluation for a single run.
    ///
    /// Retries up to 3 times with exponential backoff on transient errors.
    pub async fn evaluate(&self, input: &JudgeInput) -> Result<JudgeOutput> {
        let prompt = build_judge_prompt(input);

        let mut last_err = None;
        for attempt in 0..3 {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * (1 << attempt));
                tokio::time::sleep(delay).await;
                warn!("Judge retry attempt {}", attempt + 1);
            }

            match self.call_litellm(&prompt).await {
                Ok(raw_response) => {
                    let output = parse_judge_response(&raw_response, input.inverted)
                        .with_context(|| {
                            format!("Failed to parse judge response: {}", &raw_response[..200.min(raw_response.len())])
                        })?;
                    return Ok(output);
                }
                Err(e) => {
                    warn!("Judge call attempt {} failed: {}", attempt + 1, e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Judge call failed after retries")))
    }

    /// POST to litellm `/v1/chat/completions`.
    async fn call_litellm(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": JUDGE_SYSTEM_PROMPT
                },
                {
                    "role": "user",
                    "content": prompt
                }
            ],
            "temperature": 0.1,
            "response_format": { "type": "json_object" }
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("HTTP request to litellm failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("litellm returned {}: {}", status, &text[..200.min(text.len())]);
        }

        let json: serde_json::Value = resp.json().await.context("Failed to parse litellm response JSON")?;

        // Extract content from OpenAI-compatible response
        let content = json
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .context("litellm response missing choices[0].message.content")?
            .to_string();

        Ok(content)
    }
}

// =============================================================================
// Prompt construction
// =============================================================================

const JUDGE_SYSTEM_PROMPT: &str = r#"You are an expert Rust code reviewer evaluating an AI agent's implementation quality.

You will be given:
1. PR requirements (description + linked issues)
2. The expected diff (human-written ground truth)
3. The actual diff (what the AI agent produced)

Score the actual diff on exactly 6 dimensions from 1 to 5 (5 = perfect, 1 = completely wrong):

- File Precision (1-5): Only the correct files were modified. No unrelated files changed.
- File Recall (1-5): All required files were modified. Nothing was missed.
- Logical Equivalence (1-5): The implementation logic matches the expected behavior semantically.
- Code Quality (1-5): Code is idiomatic Rust, readable, properly tested, follows project conventions.
- Edge Case Handling (1-5): Error paths, boundary conditions, and edge cases are addressed.
- Approach Validity (1-5): The architectural approach is sound, defensible, and maintainable.

Respond ONLY with valid JSON in this exact format:
{
  "dimensions": [
    {"dimension": "File Precision", "score": N, "reasoning": "..."},
    {"dimension": "File Recall", "score": N, "reasoning": "..."},
    {"dimension": "Logical Equivalence", "score": N, "reasoning": "..."},
    {"dimension": "Code Quality", "score": N, "reasoning": "..."},
    {"dimension": "Edge Case Handling", "score": N, "reasoning": "..."},
    {"dimension": "Approach Validity", "score": N, "reasoning": "..."}
  ],
  "overall_reasoning": "..."
}"#;

/// Build the user message for the judge.
fn build_judge_prompt(input: &JudgeInput) -> String {
    let issues_section = if input.linked_issues.is_empty() {
        "None".to_string()
    } else {
        input
            .linked_issues
            .iter()
            .enumerate()
            .map(|(i, iss)| format!("{}. {}", i + 1, iss))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let inverted_note = if input.inverted {
        "\n\n⚠️ NOTE: This PR was REVERTED or REJECTED in the ground truth. Evaluate the actual diff critically — a correct agent should have NOT made these changes or should have implemented them differently."
    } else {
        ""
    };

    format!(
        r#"## Requirements

### PR Description
{pr_description}

### Linked Issues
{issues_section}

### Repository Context
{repo_context}{inverted_note}

## Ground Truth (Expected Diff)

```diff
{expected_diff}
```

## Agent Output (Actual Diff)

```diff
{actual_diff}
```

Evaluate the agent output against the requirements and ground truth. Score each dimension 1-5."#,
        pr_description = input.pr_description,
        repo_context = input.repo_context,
        expected_diff = input.expected_diff,
        actual_diff = input.actual_diff,
    )
}

// =============================================================================
// Response parsing
// =============================================================================

/// Parse the LLM's JSON response into a [`JudgeOutput`].
fn parse_judge_response(raw: &str, inverted: bool) -> Result<JudgeOutput> {
    let json: serde_json::Value = serde_json::from_str(raw)
        .with_context(|| format!("Invalid JSON from judge: {}", &raw[..200.min(raw.len())]))?;

    let dims_arr = json["dimensions"]
        .as_array()
        .context("Missing 'dimensions' array in judge response")?;

    if dims_arr.len() != 6 {
        bail!(
            "Expected 6 dimensions, got {}",
            dims_arr.len()
        );
    }

    let dimensions: Vec<DimensionScore> = dims_arr
        .iter()
        .map(|d| {
            Ok(DimensionScore {
                dimension: d["dimension"]
                    .as_str()
                    .context("dimension name missing")?
                    .to_string(),
                score: d["score"]
                    .as_f64()
                    .context("dimension score missing or not a number")? as f32,
                reasoning: d["reasoning"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    // Validate scores are in range
    for d in &dimensions {
        if !(1.0..=5.0).contains(&d.score) {
            bail!("Dimension '{}' score {} is out of range [1, 5]", d.dimension, d.score);
        }
    }

    let composite = compute_composite(&dimensions);
    let overall_reasoning = json["overall_reasoning"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let pass = if inverted {
        composite < INVERTED_PASS_THRESHOLD
    } else {
        composite >= PASS_THRESHOLD
    };

    Ok(JudgeOutput {
        dimensions,
        composite,
        overall_reasoning,
        pass,
    })
}

/// Compute the weighted composite score.
///
/// Dimension order must match [`DIMENSIONS`] order.
pub fn compute_composite(dimensions: &[DimensionScore]) -> f32 {
    // Build a map for robustness (order may vary in LLM output)
    let mut composite = 0.0f32;
    for (i, dim_name) in DIMENSIONS.iter().enumerate() {
        if let Some(d) = dimensions.iter().find(|d| d.dimension == *dim_name) {
            composite += d.score * WEIGHTS[i];
        }
    }
    composite
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one() {
        assert_weights_sum_to_one();
    }

    #[test]
    fn compute_composite_all_fives() {
        let dims: Vec<DimensionScore> = DIMENSIONS
            .iter()
            .map(|&d| DimensionScore {
                dimension: d.to_string(),
                score: 5.0,
                reasoning: String::new(),
            })
            .collect();
        let composite = compute_composite(&dims);
        assert!((composite - 5.0).abs() < 1e-4, "All 5s should give 5.0, got {composite}");
    }

    #[test]
    fn compute_composite_all_ones() {
        let dims: Vec<DimensionScore> = DIMENSIONS
            .iter()
            .map(|&d| DimensionScore {
                dimension: d.to_string(),
                score: 1.0,
                reasoning: String::new(),
            })
            .collect();
        let composite = compute_composite(&dims);
        assert!((composite - 1.0).abs() < 1e-4, "All 1s should give 1.0, got {composite}");
    }

    #[test]
    fn parse_judge_response_valid() {
        let raw = serde_json::json!({
            "dimensions": [
                {"dimension": "File Precision", "score": 4, "reasoning": "Good"},
                {"dimension": "File Recall", "score": 3, "reasoning": "Missed one"},
                {"dimension": "Logical Equivalence", "score": 4, "reasoning": "Mostly correct"},
                {"dimension": "Code Quality", "score": 5, "reasoning": "Excellent"},
                {"dimension": "Edge Case Handling", "score": 3, "reasoning": "Some gaps"},
                {"dimension": "Approach Validity", "score": 4, "reasoning": "Sound"}
            ],
            "overall_reasoning": "Good implementation overall"
        })
        .to_string();

        let output = parse_judge_response(&raw, false).unwrap();
        assert_eq!(output.dimensions.len(), 6);
        assert!(!output.overall_reasoning.is_empty());
        // composite = 4*0.15 + 3*0.15 + 4*0.30 + 5*0.20 + 3*0.10 + 4*0.10
        // = 0.60 + 0.45 + 1.20 + 1.00 + 0.30 + 0.40 = 3.95
        assert!((output.composite - 3.95).abs() < 0.01, "Expected ~3.95, got {}", output.composite);
        assert!(output.pass, "Composite 3.95 should pass threshold 3.0");
    }

    #[test]
    fn parse_judge_response_inverted_low_score_passes() {
        let raw = serde_json::json!({
            "dimensions": [
                {"dimension": "File Precision", "score": 1, "reasoning": "Wrong files"},
                {"dimension": "File Recall", "score": 1, "reasoning": "Wrong"},
                {"dimension": "Logical Equivalence", "score": 2, "reasoning": "Wrong logic"},
                {"dimension": "Code Quality", "score": 2, "reasoning": "Bad"},
                {"dimension": "Edge Case Handling", "score": 1, "reasoning": "None"},
                {"dimension": "Approach Validity", "score": 1, "reasoning": "Wrong approach"}
            ],
            "overall_reasoning": "Correctly identified as bad change"
        })
        .to_string();

        let output = parse_judge_response(&raw, true).unwrap();
        assert!(output.composite < INVERTED_PASS_THRESHOLD,
            "Inverted low score should be < {INVERTED_PASS_THRESHOLD}, got {}", output.composite);
        assert!(output.pass, "Inverted: low score should be a pass");
    }

    #[test]
    fn parse_judge_response_inverted_high_score_fails() {
        let raw = serde_json::json!({
            "dimensions": [
                {"dimension": "File Precision", "score": 5, "reasoning": "Good"},
                {"dimension": "File Recall", "score": 5, "reasoning": "Good"},
                {"dimension": "Logical Equivalence", "score": 5, "reasoning": "Good"},
                {"dimension": "Code Quality", "score": 5, "reasoning": "Good"},
                {"dimension": "Edge Case Handling", "score": 5, "reasoning": "Good"},
                {"dimension": "Approach Validity", "score": 5, "reasoning": "Good"}
            ],
            "overall_reasoning": "Perfect but should not have been merged"
        })
        .to_string();

        let output = parse_judge_response(&raw, true).unwrap();
        assert_eq!(output.composite, 5.0);
        assert!(!output.pass, "Inverted: high score should be a fail");
    }

    #[test]
    fn parse_judge_response_out_of_range_score_rejected() {
        let raw = serde_json::json!({
            "dimensions": [
                {"dimension": "File Precision", "score": 6, "reasoning": "Impossible"},
                {"dimension": "File Recall", "score": 3, "reasoning": "OK"},
                {"dimension": "Logical Equivalence", "score": 3, "reasoning": "OK"},
                {"dimension": "Code Quality", "score": 3, "reasoning": "OK"},
                {"dimension": "Edge Case Handling", "score": 3, "reasoning": "OK"},
                {"dimension": "Approach Validity", "score": 3, "reasoning": "OK"}
            ],
            "overall_reasoning": "Test"
        })
        .to_string();

        assert!(parse_judge_response(&raw, false).is_err(), "Score 6 should be rejected");
    }

    #[test]
    fn parse_judge_response_wrong_dimension_count() {
        let raw = serde_json::json!({
            "dimensions": [
                {"dimension": "File Precision", "score": 3, "reasoning": "OK"}
            ],
            "overall_reasoning": "Only one dimension"
        })
        .to_string();

        assert!(parse_judge_response(&raw, false).is_err(), "Only 1 dimension should fail");
    }

    #[test]
    fn build_prompt_includes_inverted_note() {
        let input = JudgeInput {
            pr_description: "Fix bug".to_string(),
            linked_issues: vec![],
            expected_diff: "diff...".to_string(),
            actual_diff: "diff...".to_string(),
            repo_context: "context".to_string(),
            inverted: true,
        };
        let prompt = build_judge_prompt(&input);
        assert!(prompt.contains("REVERTED or REJECTED"), "Inverted prompt should warn about rejection");
    }

    #[test]
    fn build_prompt_includes_linked_issues() {
        let input = JudgeInput {
            pr_description: "Feature".to_string(),
            linked_issues: vec!["Issue 1: do X".to_string(), "Issue 2: do Y".to_string()],
            expected_diff: "diff".to_string(),
            actual_diff: "diff".to_string(),
            repo_context: "ctx".to_string(),
            inverted: false,
        };
        let prompt = build_judge_prompt(&input);
        assert!(prompt.contains("Issue 1: do X"));
        assert!(prompt.contains("Issue 2: do Y"));
    }

    #[test]
    fn dimensions_and_weights_consistent_length() {
        assert_eq!(DIMENSIONS.len(), WEIGHTS.len(), "DIMENSIONS and WEIGHTS must have same length");
    }
}
