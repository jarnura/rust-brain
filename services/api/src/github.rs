//! GitHub CLI wrapper for repository operations.
//!
//! [`GithubClient`] wraps the `gh` CLI tool and `git` for cloning repos,
//! extracting PR metadata, and checking out specific commits.
//!
//! Auth is detected from environment variables:
//! - `GH_TOKEN` → PAT mode
//! - `GITHUB_APP_ID` + `GITHUB_APP_PRIVATE_KEY` → App mode
//! - Neither set → unauthenticated (public repos only)

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;
use tracing::warn;

// =============================================================================
// Auth
// =============================================================================

/// Authentication method used for GitHub CLI operations.
#[derive(Debug, Clone)]
pub enum GithubAuthMethod {
    /// Personal Access Token — reads `GH_TOKEN` environment variable.
    Pat,
    /// GitHub App — reads `GITHUB_APP_ID` + `GITHUB_APP_PRIVATE_KEY` env vars.
    App,
    /// No authentication (public repositories only).
    None,
}

// =============================================================================
// Client
// =============================================================================

/// GitHub CLI wrapper for repository and pull request operations.
///
/// All subprocess calls use [`tokio::process::Command`]. Non-zero exit codes
/// are converted to `anyhow::Error` values containing the full stderr output.
/// All stderr output is logged at `WARN` level regardless of exit code.
#[derive(Debug, Clone)]
pub struct GithubClient {
    /// Authentication method injected into subprocess environment.
    pub auth_method: GithubAuthMethod,
}

impl GithubClient {
    /// Create a new client with an explicit auth method.
    pub fn new(auth_method: GithubAuthMethod) -> Self {
        Self { auth_method }
    }

    /// Detect auth method from environment variables.
    ///
    /// Prefers `App` mode if `GITHUB_APP_ID` is set, then `Pat` if `GH_TOKEN`
    /// is set, otherwise falls back to `None`.
    pub fn from_env() -> Self {
        let auth_method = if std::env::var("GITHUB_APP_ID").is_ok() {
            GithubAuthMethod::App
        } else if std::env::var("GH_TOKEN").is_ok() {
            GithubAuthMethod::Pat
        } else {
            GithubAuthMethod::None
        };
        Self { auth_method }
    }

    /// Build the environment variable pairs to inject into subprocess calls.
    fn build_env(&self) -> Vec<(String, String)> {
        match &self.auth_method {
            GithubAuthMethod::Pat => {
                if let Ok(token) = std::env::var("GH_TOKEN") {
                    vec![("GH_TOKEN".to_string(), token)]
                } else {
                    vec![]
                }
            }
            // GitHub App tokens are typically exchanged externally and injected
            // as GH_TOKEN by the calling process. Pass through both vars so
            // any wrapper scripts can pick them up.
            GithubAuthMethod::App => {
                let mut env = vec![];
                if let Ok(id) = std::env::var("GITHUB_APP_ID") {
                    env.push(("GITHUB_APP_ID".to_string(), id));
                }
                if let Ok(key) = std::env::var("GITHUB_APP_PRIVATE_KEY") {
                    env.push(("GITHUB_APP_PRIVATE_KEY".to_string(), key));
                }
                env
            }
            GithubAuthMethod::None => vec![],
        }
    }

    /// Clone a GitHub repository to `dest`.
    ///
    /// Uses `--depth=1` for a shallow clone. Returns the default branch name
    /// as detected by `git symbolic-ref --short HEAD` after cloning.
    ///
    /// When [`GithubAuthMethod::None`] is active (no token configured), falls
    /// back to plain `git clone` which works for public repos without requiring
    /// `gh auth login`. Authenticated methods continue to use `gh repo clone`.
    ///
    /// # Errors
    ///
    /// Returns an error if the subprocess exits non-zero or if the branch
    /// detection step fails.
    pub async fn clone_repo(&self, url: &str, dest: &Path) -> Result<String> {
        let dest_str = dest.to_string_lossy().to_string();

        match self.auth_method {
            GithubAuthMethod::None => {
                // No credentials — use plain git which works for public repos
                // without any gh CLI auth setup.
                let output = Command::new("git")
                    .args(["clone", "--depth=1", url, &dest_str])
                    .output()
                    .await?;
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !stderr.is_empty() {
                    warn!("git clone stderr: {}", stderr);
                }
                if !output.status.success() {
                    bail!("git clone failed (exit {}): {}", output.status, stderr);
                }
            }
            GithubAuthMethod::Pat | GithubAuthMethod::App => {
                let mut cmd = Command::new("gh");
                cmd.args(["repo", "clone", url, &dest_str, "--", "--depth=1"]);
                for (k, v) in self.build_env() {
                    cmd.env(k, v);
                }
                let output = cmd.output().await?;
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                if !stderr.is_empty() {
                    warn!("gh repo clone stderr: {}", stderr);
                }
                if !output.status.success() {
                    bail!("gh repo clone failed (exit {}): {}", output.status, stderr);
                }
            }
        };

        // Detect default branch
        let branch_out = Command::new("git")
            .args(["-C", &dest_str, "symbolic-ref", "--short", "HEAD"])
            .output()
            .await?;

        let branch_stderr = String::from_utf8_lossy(&branch_out.stderr).to_string();
        if !branch_stderr.is_empty() {
            warn!("git symbolic-ref stderr: {}", branch_stderr);
        }
        if !branch_out.status.success() {
            bail!("Failed to detect default branch: {}", branch_stderr);
        }

        let branch = String::from_utf8_lossy(&branch_out.stdout)
            .trim()
            .to_string();
        Ok(branch)
    }

    /// Extract pull request metadata using `gh pr view`.
    ///
    /// Calls:
    /// ```text
    /// gh pr view <pr_number> --repo <repo> \
    ///     --json title,body,commits,closingIssuesReferences
    /// ```
    ///
    /// Parses the JSON output into a [`PrContext`].
    ///
    /// # Errors
    ///
    /// Returns an error if the subprocess exits non-zero or if JSON parsing
    /// fails.
    pub async fn extract_pr(&self, repo: &str, pr_number: u32) -> Result<PrContext> {
        let pr_str = pr_number.to_string();

        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "view",
            &pr_str,
            "--repo",
            repo,
            "--json",
            "title,body,commits,closingIssuesReferences",
        ]);
        for (k, v) in self.build_env() {
            cmd.env(k, v);
        }

        let output = cmd.output().await?;
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !stderr.is_empty() {
            warn!("gh pr view stderr: {}", stderr);
        }
        if !output.status.success() {
            bail!("gh pr view failed (exit {}): {}", output.status, stderr);
        }

        let raw: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        parse_pr_json(raw)
    }

    /// Check out a specific commit in a local repository clone.
    ///
    /// Calls `git -C <repo_path> checkout <commit>`.
    ///
    /// # Errors
    ///
    /// Returns an error if `git checkout` exits non-zero.
    pub async fn checkout_commit(&self, repo_path: &Path, commit: &str) -> Result<()> {
        let path_str = repo_path.to_string_lossy().to_string();

        let output = Command::new("git")
            .args(["-C", &path_str, "checkout", commit])
            .output()
            .await?;

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !stderr.is_empty() {
            warn!("git checkout stderr: {}", stderr);
        }
        if !output.status.success() {
            bail!("git checkout failed (exit {}): {}", output.status, stderr);
        }

        Ok(())
    }
}

// =============================================================================
// PR types
// =============================================================================

/// Pull request metadata extracted by [`GithubClient::extract_pr`].
#[derive(Debug, Serialize, Deserialize)]
pub struct PrContext {
    /// PR title.
    pub title: String,
    /// PR description body (markdown).
    pub body: String,
    /// Commits included in the PR.
    pub commits: Vec<PrCommit>,
    /// GitHub issues that will be closed when this PR merges.
    pub closing_issues: Vec<ClosingIssue>,
}

/// A single commit that is part of a pull request.
#[derive(Debug, Serialize, Deserialize)]
pub struct PrCommit {
    /// Full commit SHA.
    pub oid: String,
    /// First line of the commit message.
    pub message_headline: String,
    /// Commit authors.
    pub authors: Vec<CommitAuthor>,
}

/// Author of a commit within a PR.
#[derive(Debug, Serialize, Deserialize)]
pub struct CommitAuthor {
    /// GitHub login of the author.
    pub login: String,
}

/// An issue that will be closed when a PR merges.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClosingIssue {
    /// Issue number.
    pub number: u32,
    /// Issue title.
    pub title: String,
    /// GitHub URL of the issue.
    pub url: String,
}

// =============================================================================
// JSON parsing
// =============================================================================

/// Parse the raw JSON from `gh pr view --json ...` into a [`PrContext`].
fn parse_pr_json(raw: serde_json::Value) -> Result<PrContext> {
    let empty_vec = vec![];

    let title = raw["title"].as_str().unwrap_or("").to_string();
    let body = raw["body"].as_str().unwrap_or("").to_string();

    let commits = raw["commits"]
        .as_array()
        .unwrap_or(&empty_vec)
        .iter()
        .map(|c| {
            let authors = c["authors"]
                .as_array()
                .unwrap_or(&empty_vec)
                .iter()
                .map(|a| CommitAuthor {
                    login: a["login"].as_str().unwrap_or("").to_string(),
                })
                .collect();
            PrCommit {
                oid: c["oid"].as_str().unwrap_or("").to_string(),
                message_headline: c["messageHeadline"].as_str().unwrap_or("").to_string(),
                authors,
            }
        })
        .collect();

    let closing_issues = raw["closingIssuesReferences"]
        .as_array()
        .unwrap_or(&empty_vec)
        .iter()
        .map(|i| ClosingIssue {
            number: i["number"].as_u64().unwrap_or(0) as u32,
            title: i["title"].as_str().unwrap_or("").to_string(),
            url: i["url"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(PrContext {
        title,
        body,
        commits,
        closing_issues,
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_no_auth() {
        temp_env::with_vars(
            [("GH_TOKEN", None::<&str>), ("GITHUB_APP_ID", None::<&str>)],
            || {
                let client = GithubClient::from_env();
                assert!(matches!(client.auth_method, GithubAuthMethod::None));
            },
        );
    }

    #[test]
    fn test_from_env_pat() {
        temp_env::with_vars(
            [
                ("GH_TOKEN", Some("ghp_test")),
                ("GITHUB_APP_ID", None::<&str>),
            ],
            || {
                let client = GithubClient::from_env();
                assert!(matches!(client.auth_method, GithubAuthMethod::Pat));
            },
        );
    }

    #[test]
    fn test_from_env_app_wins_over_pat() {
        temp_env::with_vars(
            [
                ("GH_TOKEN", Some("ghp_test")),
                ("GITHUB_APP_ID", Some("12345")),
            ],
            || {
                let client = GithubClient::from_env();
                assert!(matches!(client.auth_method, GithubAuthMethod::App));
            },
        );
    }

    #[test]
    fn test_build_env_pat() {
        temp_env::with_vars(
            [
                ("GH_TOKEN", Some("ghp_abc")),
                ("GITHUB_APP_ID", None::<&str>),
            ],
            || {
                let client = GithubClient::new(GithubAuthMethod::Pat);
                let env = client.build_env();
                assert_eq!(env.len(), 1);
                assert_eq!(env[0], ("GH_TOKEN".to_string(), "ghp_abc".to_string()));
            },
        );
    }

    #[test]
    fn test_build_env_none() {
        let client = GithubClient::new(GithubAuthMethod::None);
        let env = client.build_env();
        assert!(env.is_empty());
    }

    /// Verify that `clone_repo` with `GithubAuthMethod::None` invokes `git clone`
    /// (not `gh repo clone`). We test this indirectly: calling clone against a
    /// non-existent path with no git binary shimmed will fail, but the error
    /// message must contain "git clone failed" — not "gh repo clone failed".
    #[tokio::test]
    async fn test_clone_repo_none_uses_git_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");

        // Use a deliberately invalid URL so the command exits non-zero quickly.
        let client = GithubClient::new(GithubAuthMethod::None);
        let err = client
            .clone_repo("https://invalid.example/repo.git", &dest)
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("git clone failed"),
            "expected 'git clone failed' in error, got: {msg}"
        );
        assert!(
            !msg.contains("gh repo clone failed"),
            "None auth should not invoke gh repo clone, got: {msg}"
        );
    }

    /// Verify that `clone_repo` with `GithubAuthMethod::Pat` invokes `gh repo clone`.
    #[tokio::test]
    async fn test_clone_repo_pat_uses_gh_clone() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");

        let client = GithubClient::new(GithubAuthMethod::Pat);
        let err = client
            .clone_repo("owner/invalid-repo-xyz", &dest)
            .await
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("gh repo clone failed"),
            "expected 'gh repo clone failed' in error, got: {msg}"
        );
    }

    #[test]
    fn test_parse_pr_json_full() {
        let raw = serde_json::json!({
            "title": "Fix the thing",
            "body": "## Summary\n\nFixes the bug.",
            "commits": [
                {
                    "oid": "abc123",
                    "messageHeadline": "fix: the thing",
                    "authors": [{"login": "octocat"}]
                }
            ],
            "closingIssuesReferences": [
                {
                    "number": 42,
                    "title": "Bug: the thing",
                    "url": "https://github.com/org/repo/issues/42"
                }
            ]
        });

        let pr = parse_pr_json(raw).unwrap();
        assert_eq!(pr.title, "Fix the thing");
        assert_eq!(pr.body, "## Summary\n\nFixes the bug.");
        assert_eq!(pr.commits.len(), 1);
        assert_eq!(pr.commits[0].oid, "abc123");
        assert_eq!(pr.commits[0].message_headline, "fix: the thing");
        assert_eq!(pr.commits[0].authors[0].login, "octocat");
        assert_eq!(pr.closing_issues.len(), 1);
        assert_eq!(pr.closing_issues[0].number, 42);
        assert_eq!(pr.closing_issues[0].title, "Bug: the thing");
    }

    #[test]
    fn test_parse_pr_json_empty_arrays() {
        let raw = serde_json::json!({
            "title": "Empty PR",
            "body": "",
            "commits": [],
            "closingIssuesReferences": []
        });

        let pr = parse_pr_json(raw).unwrap();
        assert_eq!(pr.title, "Empty PR");
        assert!(pr.commits.is_empty());
        assert!(pr.closing_issues.is_empty());
    }

    #[test]
    fn test_parse_pr_json_missing_fields() {
        let raw = serde_json::json!({});
        let pr = parse_pr_json(raw).unwrap();
        assert_eq!(pr.title, "");
        assert_eq!(pr.body, "");
        assert!(pr.commits.is_empty());
        assert!(pr.closing_issues.is_empty());
    }

    #[test]
    fn test_parse_pr_json_multiple_commits() {
        let raw = serde_json::json!({
            "title": "Big feature",
            "body": "Implements X",
            "commits": [
                {"oid": "aaa", "messageHeadline": "feat: part 1", "authors": [{"login": "dev1"}]},
                {"oid": "bbb", "messageHeadline": "feat: part 2", "authors": [{"login": "dev2"}, {"login": "dev3"}]}
            ],
            "closingIssuesReferences": [
                {"number": 1, "title": "Issue 1", "url": "https://github.com/o/r/issues/1"},
                {"number": 2, "title": "Issue 2", "url": "https://github.com/o/r/issues/2"}
            ]
        });

        let pr = parse_pr_json(raw).unwrap();
        assert_eq!(pr.commits.len(), 2);
        assert_eq!(pr.commits[1].authors.len(), 2);
        assert_eq!(pr.closing_issues.len(), 2);
        assert_eq!(pr.closing_issues[1].number, 2);
    }

    #[test]
    fn test_pr_context_roundtrip_json() {
        let ctx = PrContext {
            title: "test".to_string(),
            body: "body".to_string(),
            commits: vec![PrCommit {
                oid: "abc".to_string(),
                message_headline: "msg".to_string(),
                authors: vec![CommitAuthor {
                    login: "user".to_string(),
                }],
            }],
            closing_issues: vec![],
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: PrContext = serde_json::from_str(&json).unwrap();
        assert_eq!(back.title, "test");
        assert_eq!(back.commits[0].oid, "abc");
    }
}
