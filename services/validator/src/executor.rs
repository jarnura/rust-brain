//! Executor — drives an OpenCode orchestrator session and captures the
//! resulting git diff.
//!
//! ## Flow
//!
//! 1. Create an OpenCode session with a descriptive title.
//! 2. Send the requirements text to the session (with a configurable timeout).
//! 3. After the agent run completes, capture `git diff HEAD` from `repo_path`.
//! 4. Return the unified diff string for the comparator.

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{info, warn};

use crate::opencode::OpenCodeClient;

/// Default execution timeout (seconds).
pub const DEFAULT_TIMEOUT_SECS: u64 = 7200;

/// Parameters for a single executor run.
#[derive(Debug, Clone)]
pub struct ExecutorParams {
    /// Requirements text (PR description or fallback).
    pub requirements: String,
    /// Local path of the repository clone where the agent will work.
    pub repo_path: std::path::PathBuf,
    /// Maximum wall-clock time for the agent session.
    pub timeout_secs: u64,
    /// Optional title for the OpenCode session.
    pub session_title: Option<String>,
}

impl ExecutorParams {
    /// Create params with the default timeout.
    pub fn new(requirements: impl Into<String>, repo_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            requirements: requirements.into(),
            repo_path: repo_path.into(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            session_title: None,
        }
    }

    /// Override the timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Override the session title.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.session_title = Some(title.into());
        self
    }
}

/// Run the agent against `requirements` and return the `git diff HEAD` output.
///
/// # Errors
///
/// Returns an error if:
/// - The OpenCode session cannot be created.
/// - The message send times out or fails.
/// - The `git diff` subprocess exits non-zero.
pub async fn execute(client: &OpenCodeClient, params: &ExecutorParams) -> Result<String> {
    let title = params
        .session_title
        .as_deref()
        .unwrap_or("rustbrain-validator");

    info!(
        session_title = title,
        timeout_secs = params.timeout_secs,
        "Creating OpenCode session"
    );

    let session = client
        .create_session(Some(title))
        .await
        .context("Failed to create OpenCode session")?;

    info!(session_id = %session.id, "OpenCode session created");

    // Send requirements with a wall-clock timeout
    let send_future = client.send_message(&session.id, &params.requirements);
    match timeout(Duration::from_secs(params.timeout_secs), send_future).await {
        Ok(Ok(msg)) => {
            info!(
                session_id = %session.id,
                message_id = %msg.id,
                "Agent session completed"
            );
        }
        Ok(Err(e)) => {
            warn!(session_id = %session.id, error = %e, "Agent session returned error");
            // Attempt cleanup
            let _ = client.delete_session(&session.id).await;
            return Err(e).context("OpenCode agent session failed");
        }
        Err(_elapsed) => {
            warn!(
                session_id = %session.id,
                timeout_secs = params.timeout_secs,
                "Agent session timed out"
            );
            let _ = client.delete_session(&session.id).await;
            bail!(
                "Agent session timed out after {} seconds",
                params.timeout_secs
            );
        }
    }

    // Capture the diff produced by the agent
    let diff = capture_git_diff(&params.repo_path).await?;

    // Clean up the session
    if let Err(e) = client.delete_session(&session.id).await {
        warn!(session_id = %session.id, error = %e, "Failed to delete OpenCode session");
    }

    Ok(diff)
}

/// Run `git diff HEAD` and return the unified diff as a string.
async fn capture_git_diff(repo_path: &Path) -> Result<String> {
    let path_str = repo_path.to_string_lossy().to_string();

    info!(repo_path = %path_str, "Capturing git diff HEAD");

    let out = Command::new("git")
        .args(["-C", &path_str, "diff", "HEAD"])
        .output()
        .await
        .context("Failed to spawn git diff")?;

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !stderr.is_empty() {
        warn!("git diff stderr: {}", stderr);
    }
    if !out.status.success() {
        bail!("git diff HEAD failed (exit {}): {}", out.status, stderr);
    }

    let diff = String::from_utf8_lossy(&out.stdout).to_string();
    info!(diff_bytes = diff.len(), "git diff captured");

    Ok(diff)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_params_defaults() {
        let p = ExecutorParams::new("requirements text", "/tmp/repo");
        assert_eq!(p.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(p.session_title, None);
        assert_eq!(p.requirements, "requirements text");
    }

    #[test]
    fn executor_params_with_timeout() {
        let p = ExecutorParams::new("req", "/tmp/r").with_timeout(300);
        assert_eq!(p.timeout_secs, 300);
    }

    #[test]
    fn executor_params_with_title() {
        let p = ExecutorParams::new("req", "/tmp/r").with_title("my-session");
        assert_eq!(p.session_title, Some("my-session".to_string()));
    }

    #[test]
    fn executor_params_builder_chain() {
        let p = ExecutorParams::new("req", "/tmp/r")
            .with_timeout(600)
            .with_title("chained");
        assert_eq!(p.timeout_secs, 600);
        assert_eq!(p.session_title, Some("chained".to_string()));
    }

    #[tokio::test]
    async fn capture_git_diff_fails_gracefully_on_nonrepo() {
        // /tmp is not a git repository; git diff should fail cleanly.
        let result = capture_git_diff(Path::new("/tmp")).await;
        assert!(result.is_err(), "Expected error for non-git directory");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("git") || err.contains("not a git"),
            "Expected git-related error, got: {}",
            err
        );
    }

    #[test]
    fn default_timeout_is_reasonable() {
        // 7200 seconds = 2 hours — sanity check that it hasn't been accidentally
        // lowered to a value that would cause spurious timeouts in CI.
        const { assert!(DEFAULT_TIMEOUT_SECS >= 3600) };
        const { assert!(DEFAULT_TIMEOUT_SECS <= 86400) };
    }
}
