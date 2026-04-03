//! Background timeout sweeper for stale execution containers.
//!
//! [`start_sweeper`] launches a Tokio task that wakes every
//! `interval` seconds, queries for executions that have exceeded their
//! `timeout_config_secs`, kills each container, and marks the row `timeout`.

use std::time::Duration;

use sqlx::PgPool;
use tracing::{info, warn};

use super::models::{list_timed_out_executions, timeout_execution};
use crate::docker::DockerClient;

/// Launch the background sweeper as a detached Tokio task.
///
/// `interval` controls how often the sweeper checks for stale executions.
/// The recommended default is 30 seconds.
pub fn start_sweeper(pool: PgPool, docker: DockerClient, interval: Duration) {
    tokio::spawn(sweeper_loop(pool, docker, interval));
}

/// Inner sweep loop — runs indefinitely until the process exits.
async fn sweeper_loop(pool: PgPool, docker: DockerClient, interval: Duration) {
    info!(
        "Execution timeout sweeper started (interval={:?})",
        interval
    );
    loop {
        tokio::time::sleep(interval).await;
        sweep_once(&pool, &docker).await;
    }
}

/// Single sweep pass: find stale executions, kill containers, mark timeout.
async fn sweep_once(pool: &PgPool, docker: &DockerClient) {
    let stale = match list_timed_out_executions(pool).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "Sweeper: failed to query timed-out executions");
            return;
        }
    };

    for (exec_id, container_id) in stale {
        info!(execution_id = %exec_id, "Sweeper: killing timed-out execution");

        // Kill the container if we have an ID for it
        if let Some(cid) = &container_id {
            if let Err(e) = docker.remove_container(cid).await {
                warn!(
                    execution_id = %exec_id,
                    container_id = %cid,
                    error = %e,
                    "Sweeper: failed to remove container"
                );
            }
        }

        // Mark execution as timed out
        if let Err(e) = timeout_execution(pool, exec_id).await {
            warn!(execution_id = %exec_id, error = %e, "Sweeper: failed to mark execution timeout");
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_interval_is_reasonable() {
        // Document the expected default — callers should use 30 s.
        let interval = Duration::from_secs(30);
        assert!(
            interval.as_secs() >= 10,
            "sweeper interval should be at least 10 s"
        );
        assert!(
            interval.as_secs() <= 300,
            "sweeper interval should not exceed 5 min"
        );
    }
}
