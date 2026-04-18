//! Docker resource leak detection.
//!
//! Detects orphaned Docker volumes and containers not tracked in Postgres.
//! Uses the Docker CLI via `tokio::process::Command` (same approach as
//! services/api/src/docker.rs) rather than a daemon SDK, to keep the build
//! simple and avoid extra dependencies.

use anyhow::{bail, Context};
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Result of a Docker leak detection scan.
#[derive(Debug, Default)]
pub struct LeakDetectionResult {
    /// Number of Docker volumes with label `rustbrain.workspace=true` not in the workspaces table.
    pub orphan_volumes: usize,
    /// Number of execution containers named `rustbrain-exec-*` not in the executions table.
    pub orphan_containers: usize,
    /// Number of orphaned volumes removed (only when dry_run=false).
    pub cleaned_volumes: usize,
    /// Number of orphaned containers removed (only when dry_run=false).
    pub cleaned_containers: usize,
    /// Names of detected orphan volumes (for audit logging).
    pub orphan_volume_names: Vec<String>,
    /// IDs of detected orphan containers (for audit logging).
    pub orphan_container_ids: Vec<String>,
    /// Names of removed volumes (for audit logging).
    pub cleaned_volume_names: Vec<String>,
    /// IDs of removed containers (for audit logging).
    pub cleaned_container_ids: Vec<String>,
}

pub async fn detect_docker_leaks(
    pool: &sqlx::PgPool,
    dry_run: bool,
) -> anyhow::Result<LeakDetectionResult> {
    let mut result = LeakDetectionResult::default();

    let all_volumes = list_rustbrain_volumes().await?;
    debug!("Found {} rustbrain volumes", all_volumes.len());

    let tracked_volumes = get_tracked_volumes(pool).await?;
    debug!(
        "Found {} tracked volumes in Postgres",
        tracked_volumes.len()
    );

    for vol in &all_volumes {
        if !tracked_volumes.contains(vol) {
            result.orphan_volume_names.push(vol.clone());
        }
    }
    result.orphan_volumes = result.orphan_volume_names.len();

    if result.orphan_volumes > 0 {
        info!("Detected {} orphan volumes", result.orphan_volumes);
        for vol in &result.orphan_volume_names {
            debug!("  Orphan volume: {}", vol);
        }

        if !dry_run {
            for vol in &result.orphan_volume_names {
                match remove_docker_volume(vol).await {
                    Ok(()) => {
                        result.cleaned_volumes += 1;
                        result.cleaned_volume_names.push(vol.clone());
                        info!("Removed orphan volume: {}", vol);
                    }
                    Err(e) => {
                        warn!("Failed to remove orphan volume {}: {}", vol, e);
                    }
                }
            }
        }
    }

    let all_containers = list_rustbrain_containers().await?;
    debug!(
        "Found {} rustbrain execution containers",
        all_containers.len()
    );

    let tracked_container_ids = get_tracked_containers(pool).await?;
    debug!(
        "Found {} tracked containers in Postgres",
        tracked_container_ids.len()
    );

    for (cid, _name) in &all_containers {
        if !tracked_container_ids.contains(cid) {
            result.orphan_container_ids.push(cid.clone());
        }
    }
    result.orphan_containers = result.orphan_container_ids.len();

    if result.orphan_containers > 0 {
        info!("Detected {} orphan containers", result.orphan_containers);
        for cid in &result.orphan_container_ids {
            debug!("  Orphan container: {}", cid);
        }

        if !dry_run {
            for cid in &result.orphan_container_ids {
                match remove_docker_container(cid).await {
                    Ok(()) => {
                        result.cleaned_containers += 1;
                        result.cleaned_container_ids.push(cid.clone());
                        info!("Removed orphan container: {}", cid);
                    }
                    Err(e) => {
                        warn!("Failed to remove orphan container {}: {}", cid, e);
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Lists all Docker volumes with label `rustbrain.workspace=true`.
async fn list_rustbrain_volumes() -> anyhow::Result<Vec<String>> {
    let output = Command::new("docker")
        .args([
            "volume",
            "ls",
            "-q",
            "--filter",
            "label=rustbrain.workspace=true",
        ])
        .output()
        .await
        .context("failed to spawn `docker volume ls`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`docker volume ls` failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("docker volume ls: stdout not UTF-8")?;
    Ok(stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Lists all Docker containers matching `name=rustbrain-exec-`.
/// Returns a list of (container_id, container_name) pairs.
async fn list_rustbrain_containers() -> anyhow::Result<Vec<(String, String)>> {
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            "name=rustbrain-exec-",
            "--format",
            "{{.ID}} {{.Names}}",
        ])
        .output()
        .await
        .context("failed to spawn `docker ps`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`docker ps` failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout).context("docker ps: stdout not UTF-8")?;
    let mut containers = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ' ');
        let cid = parts.next().unwrap_or("").to_string();
        let name = parts.next().unwrap_or("").to_string();
        if !cid.is_empty() {
            containers.push((cid, name));
        }
    }
    Ok(containers)
}

/// Returns volume names tracked in the workspaces table (non-archived workspaces).
async fn get_tracked_volumes(pool: &sqlx::PgPool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT volume_name FROM workspaces WHERE volume_name IS NOT NULL AND status != 'archived'",
    )
    .fetch_all(pool)
    .await
    .context("failed to query tracked volumes")?;

    Ok(rows)
}

/// Returns container IDs tracked in the executions table (running/pending executions).
async fn get_tracked_containers(pool: &sqlx::PgPool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT container_id FROM executions WHERE container_id IS NOT NULL AND status IN ('running', 'pending')",
    )
    .fetch_all(pool)
    .await
    .context("failed to query tracked containers")?;

    Ok(rows)
}

/// Removes a Docker volume by name.
async fn remove_docker_volume(name: &str) -> anyhow::Result<()> {
    let output = Command::new("docker")
        .args(["volume", "rm", name])
        .output()
        .await
        .context("failed to spawn `docker volume rm`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`docker volume rm {}` failed: {}", name, stderr.trim());
    }
    Ok(())
}

/// Force-removes a Docker container by ID.
async fn remove_docker_container(id: &str) -> anyhow::Result<()> {
    let output = Command::new("docker")
        .args(["rm", "-f", id])
        .output()
        .await
        .context("failed to spawn `docker rm`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`docker rm -f {}` failed: {}", id, stderr.trim());
    }
    Ok(())
}
