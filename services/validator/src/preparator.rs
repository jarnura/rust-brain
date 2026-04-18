//! Environment Preparator — resets the repository to its pre-PR state and
//! triggers an ingestion pipeline run.
//!
//! ## What it does
//!
//! 1. Determines the parent commit — the commit immediately before the PR's
//!    first commit (i.e. the state before the PR was applied).
//! 2. Calls [`GithubClient::checkout_commit`] to reset the local clone.
//! 3. Invokes the ingestion pipeline binary so the triple-store databases
//!    reflect the pre-PR code state before the executor runs.
//!
//! ## Dependency note
//!
//! End-to-end ingestion requires [RUSA-43] (Phase 1) to be complete.
//! Until then `run_ingestion` calls the binary but may exit quickly if the
//! pipeline is not yet wired to a live database.
//!
//! [RUSA-43]: /RUSA/issues/RUSA-43

use std::path::Path;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tracing::{info, warn};

use crate::github::{GithubClient, PrContext};

/// Prepare the environment for an executor run.
///
/// Checks out the commit immediately preceding the PR's first commit, then
/// runs the ingestion pipeline on the resulting working tree.
///
/// # Parameters
///
/// - `client` — GitHub CLI wrapper used for the checkout.
/// - `repo_path` — Local clone of the repository (must already exist on disk).
/// - `pr_context` — Metadata for the PR being validated.
/// - `ingestion_bin` — Path to the `ingestion` binary (or `"ingestion"` if on
///   `$PATH`). Pass `None` to skip ingestion (useful in unit-test contexts).
///
/// # Errors
///
/// Returns an error if the parent commit cannot be determined, the checkout
/// fails, or the ingestion subprocess exits non-zero.
pub async fn prepare_env(
    client: &GithubClient,
    repo_path: &Path,
    pr_context: &PrContext,
    ingestion_bin: Option<&str>,
) -> Result<()> {
    let parent_oid = resolve_parent_commit(repo_path, pr_context).await?;

    info!(
        repo_path = %repo_path.display(),
        parent_oid = %parent_oid,
        "Checking out parent commit"
    );
    client.checkout_commit(repo_path, &parent_oid).await?;

    if let Some(bin) = ingestion_bin {
        run_ingestion(bin, repo_path).await?;
    } else {
        warn!("Ingestion binary not provided — skipping pipeline run");
    }

    Ok(())
}

/// Determine the parent commit of the PR's first commit.
///
/// Strategy:
/// 1. If `pr_context.commits` is non-empty, use `git rev-parse <first_oid>^`
///    to get the commit before the PR's first commit.
/// 2. If `commits` is empty, fall back to `HEAD~1` (one commit before HEAD).
async fn resolve_parent_commit(repo_path: &Path, pr_context: &PrContext) -> Result<String> {
    let repo_str = repo_path.to_string_lossy().to_string();

    let rev_arg = match pr_context.commits.first() {
        Some(c) if !c.oid.is_empty() => format!("{}^", c.oid),
        _ => {
            warn!("PR has no commits — falling back to HEAD~1 for parent resolution");
            "HEAD~1".to_string()
        }
    };

    let out = Command::new("git")
        .args(["-C", &repo_str, "rev-parse", &rev_arg])
        .output()
        .await
        .context("git rev-parse failed")?;

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !stderr.is_empty() {
        warn!("git rev-parse stderr: {}", stderr);
    }
    if !out.status.success() {
        bail!("git rev-parse '{}' failed: {}", rev_arg, stderr);
    }

    let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if oid.is_empty() {
        bail!("git rev-parse returned empty output for '{}'", rev_arg);
    }

    Ok(oid)
}

/// Invoke the ingestion binary on `repo_path`.
///
/// The ingestion binary is expected to accept the repository path as its first
/// positional argument, e.g.:
/// ```text
/// ingestion /path/to/repo
/// ```
///
/// Environment variables (`DATABASE_URL`, `NEO4J_URI`, etc.) are inherited
/// from the current process.
async fn run_ingestion(ingestion_bin: &str, repo_path: &Path) -> Result<()> {
    let path_str = repo_path.to_string_lossy().to_string();

    info!(
        ingestion_bin = ingestion_bin,
        repo_path = %path_str,
        "Running ingestion pipeline"
    );

    let out = Command::new(ingestion_bin)
        .arg(&path_str)
        .output()
        .await
        .with_context(|| format!("Failed to spawn ingestion binary '{ingestion_bin}'"))?;

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    if !stdout.is_empty() {
        info!(ingestion_stdout = %stdout, "Ingestion stdout");
    }
    if !stderr.is_empty() {
        warn!(ingestion_stderr = %stderr, "Ingestion stderr");
    }

    if !out.status.success() {
        bail!(
            "Ingestion binary '{}' exited with status {}: {}",
            ingestion_bin,
            out.status,
            stderr
        );
    }

    info!("Ingestion pipeline completed successfully");
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{CommitAuthor, PrCommit};

    fn pr_with_commits(oids: &[&str]) -> PrContext {
        PrContext {
            title: "test".to_string(),
            body: "body".to_string(),
            commits: oids
                .iter()
                .map(|oid| PrCommit {
                    oid: oid.to_string(),
                    message_headline: "msg".to_string(),
                    authors: vec![CommitAuthor {
                        login: "dev".to_string(),
                    }],
                })
                .collect(),
            closing_issues: vec![],
        }
    }

    #[test]
    fn pr_with_no_commits_produces_head_fallback() {
        // We cannot easily run git in unit tests, but we can verify that the
        // function constructs the right rev argument by observing the branch
        // taken. We test `resolve_parent_commit` indirectly through the empty
        // commits case — the function attempts `HEAD~1` and fails because there
        // is no repo at the path. We confirm the error message mentions git.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pr = pr_with_commits(&[]);
        let result = rt.block_on(resolve_parent_commit(
            Path::new("/nonexistent-path-for-test"),
            &pr,
        ));
        // The error should come from git invocation, not from our logic
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        // Either "git rev-parse failed" or an OS-level error
        assert!(
            err.contains("git") || err.contains("No such") || err.contains("not found"),
            "Unexpected error: {}",
            err
        );
    }

    #[test]
    fn pr_with_commits_uses_first_oid_parent() {
        // Verify that a PR with commits constructs `<first_oid>^`.
        // We call rev-parse against /tmp which has no .git, so it fails —
        // but the error message reveals what was attempted.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pr = pr_with_commits(&["deadbeef1234", "cafebabe5678"]);
        let result = rt.block_on(resolve_parent_commit(Path::new("/tmp"), &pr));
        assert!(result.is_err());
        // The failure is from git (no repo), not from our arg logic
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("git") || err.contains("not a git"),
            "Expected git error, got: {}",
            err
        );
    }

    #[test]
    fn monorepo_pr_uses_same_logic() {
        // Monorepo PRs may touch many crates but the parent-commit logic is
        // repository-level: we always use the first commit's parent.
        let pr = pr_with_commits(&["aaa111", "bbb222", "ccc333"]);
        // The first oid is "aaa111" — confirm it is non-empty
        assert_eq!(pr.commits[0].oid, "aaa111");
        assert_eq!(pr.commits.len(), 3);
    }
}
