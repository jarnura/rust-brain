//! PR Extractor — wraps [`GithubClient`] to produce a [`RequirementsText`].
//!
//! The requirements text is what gets fed to the executor. If the PR body is
//! non-empty it is used directly. If it is empty the extractor falls back to
//! concatenating the PR title with all commit message headlines.

use anyhow::Result;
use tracing::warn;

use crate::github::{GithubClient, PrContext};
use crate::models::RequirementsText;

/// Extract a PR's metadata and derive a [`RequirementsText`] for the executor.
///
/// # Fallback behaviour
///
/// When `pr.body` is empty (or whitespace-only) the requirements text becomes:
///
/// ```text
/// <title>
///
/// Commits:
/// - <commit message 1>
/// - <commit message 2>
/// ```
///
/// # Errors
///
/// Propagates errors from [`GithubClient::extract_pr`].
pub async fn extract_pr(
    client: &GithubClient,
    repo: &str,
    pr_number: u32,
) -> Result<(PrContext, RequirementsText)> {
    let ctx = client.extract_pr(repo, pr_number).await?;
    let requirements = build_requirements(&ctx);
    Ok((ctx, requirements))
}

/// Derive [`RequirementsText`] from a [`PrContext`] that has already been fetched.
///
/// This is split out so callers that already hold a [`PrContext`] can re-use it.
pub fn build_requirements(ctx: &PrContext) -> RequirementsText {
    let body = ctx.body.trim();

    if !body.is_empty() {
        return RequirementsText {
            text: body.to_string(),
            used_fallback: false,
        };
    }

    warn!(
        pr_title = %ctx.title,
        "PR body is empty — falling back to title + commit messages"
    );

    let commit_lines: Vec<String> = ctx
        .commits
        .iter()
        .filter(|c| !c.message_headline.is_empty())
        .map(|c| format!("- {}", c.message_headline))
        .collect();

    let text = if commit_lines.is_empty() {
        ctx.title.clone()
    } else {
        format!(
            "{}\n\nCommits:\n{}",
            ctx.title,
            commit_lines.join("\n")
        )
    };

    RequirementsText {
        text,
        used_fallback: true,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{ClosingIssue, CommitAuthor, PrCommit};

    fn make_ctx(title: &str, body: &str, commit_messages: &[&str]) -> PrContext {
        PrContext {
            title: title.to_string(),
            body: body.to_string(),
            commits: commit_messages
                .iter()
                .map(|m| PrCommit {
                    oid: "abc".to_string(),
                    message_headline: m.to_string(),
                    authors: vec![CommitAuthor {
                        login: "dev".to_string(),
                    }],
                })
                .collect(),
            closing_issues: vec![],
        }
    }

    #[test]
    fn uses_body_when_present() {
        let ctx = make_ctx("Fix bug", "Detailed description here.", &["fix: the bug"]);
        let req = build_requirements(&ctx);
        assert_eq!(req.text, "Detailed description here.");
        assert!(!req.used_fallback);
    }

    #[test]
    fn falls_back_when_body_empty() {
        let ctx = make_ctx("Add feature", "", &["feat: add X", "test: cover X"]);
        let req = build_requirements(&ctx);
        assert!(req.used_fallback);
        assert!(req.text.starts_with("Add feature"));
        assert!(req.text.contains("feat: add X"));
        assert!(req.text.contains("test: cover X"));
    }

    #[test]
    fn falls_back_when_body_whitespace_only() {
        let ctx = make_ctx("Fix lint", "   \n\t  ", &["chore: fix lint"]);
        let req = build_requirements(&ctx);
        assert!(req.used_fallback);
        assert!(req.text.contains("Fix lint"));
    }

    #[test]
    fn fallback_with_no_commits_uses_title_only() {
        let ctx = PrContext {
            title: "Orphan PR".to_string(),
            body: "".to_string(),
            commits: vec![],
            closing_issues: vec![],
        };
        let req = build_requirements(&ctx);
        assert!(req.used_fallback);
        assert_eq!(req.text, "Orphan PR");
    }

    #[test]
    fn fallback_skips_empty_commit_messages() {
        let ctx = make_ctx("Improve perf", "", &["", "perf: faster parse", ""]);
        let req = build_requirements(&ctx);
        assert!(req.used_fallback);
        assert!(req.text.contains("perf: faster parse"));
        // Empty commit messages must not appear as bare hyphens
        let lines: Vec<&str> = req.text.lines().collect();
        for line in &lines {
            let trimmed = line.trim();
            assert!(
                trimmed != "-",
                "Bare '-' line should not appear in fallback text"
            );
        }
    }

    #[test]
    fn body_leading_trailing_whitespace_trimmed() {
        let ctx = make_ctx("PR", "  \n  Real description  \n  ", &[]);
        let req = build_requirements(&ctx);
        assert_eq!(req.text, "Real description");
        assert!(!req.used_fallback);
    }

    #[test]
    fn closing_issues_preserved_in_context() {
        let mut ctx = make_ctx("Fix", "desc", &[]);
        ctx.closing_issues = vec![ClosingIssue {
            number: 7,
            title: "Issue 7".to_string(),
            url: "https://github.com/org/repo/issues/7".to_string(),
        }];
        let req = build_requirements(&ctx);
        // Closing issues don't affect requirements text — they're on the context
        assert_eq!(req.text, "desc");
        assert_eq!(ctx.closing_issues.len(), 1);
    }
}
