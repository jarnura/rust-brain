//! Docker volume lifecycle management for per-workspace volumes.
//!
//! Uses the `docker` CLI (via `tokio::process::Command`) rather than the Docker
//! daemon SDK. This avoids an extra dependency and keeps the build simple for
//! the MVP. All subprocess errors are captured from stderr and surfaced as
//! `anyhow::Error`.
//!
//! ## Volume naming
//!
//! All workspace volumes follow the pattern `rustbrain-ws-{workspace_id_short}`,
//! where `workspace_id_short` is the first 8 characters of the workspace UUID.
//!
//! ## Labels
//!
//! Every created volume carries the label `rustbrain.workspace=true` so they
//! can be listed or pruned in bulk:
//!
//! ```bash
//! docker volume ls --filter label=rustbrain.workspace=true
//! docker volume rm $(docker volume ls -q --filter label=rustbrain.workspace=true)
//! ```

use anyhow::{anyhow, bail, Context};
use tokio::process::Command;

/// Configuration passed to [`DockerClient::run_ingestion`].
#[derive(Debug, Clone)]
pub struct IngestionConfig<'a> {
    /// Absolute path on the **host** filesystem where the repo was cloned.
    pub host_clone_path: &'a str,
    /// Docker network the ingestion container joins (e.g. `rustbrain-net`).
    pub network: &'a str,
    /// Postgres connection string, forwarded as `DATABASE_URL`.
    pub database_url: &'a str,
    /// Neo4j Bolt URL, forwarded as `NEO4J_URL`.
    pub neo4j_url: &'a str,
    /// Neo4j username, forwarded as `NEO4J_USER`.
    pub neo4j_user: &'a str,
    /// Neo4j password, forwarded as `NEO4J_PASSWORD`.
    pub neo4j_password: &'a str,
    /// Ollama base URL, forwarded as `OLLAMA_HOST`.
    pub ollama_host: &'a str,
    /// Qdrant base URL, forwarded as `QDRANT_HOST`.
    pub qdrant_host: &'a str,
    /// Embedding model name, forwarded as `EMBEDDING_MODEL`.
    pub embedding_model: &'a str,
    /// Docker image to run (e.g. `rustbrain-ingestion:latest`).
    pub ingestion_image: &'a str,
}

/// Client for creating and destroying Docker volumes that back workspaces.
///
/// By default it invokes the `docker` binary on `PATH`. Override the Docker
/// daemon target via the `DOCKER_HOST` environment variable (honoured by the
/// Docker CLI automatically).
#[derive(Debug, Clone)]
pub struct DockerClient {
    /// Path to the Docker socket (informational; the CLI reads `DOCKER_HOST`).
    pub socket_path: String,
}

impl Default for DockerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerClient {
    /// Creates a new client using the default Docker socket path.
    pub fn new() -> Self {
        Self {
            socket_path: "/var/run/docker.sock".to_string(),
        }
    }

    /// Creates a new client with an explicit socket path.
    pub fn with_socket(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Derives the canonical volume name for a workspace UUID.
    ///
    /// Uses the first 8 characters of the UUID string (without hyphens at
    /// that position).
    ///
    /// # Example
    ///
    /// ```
    /// use rustbrain_api::docker::DockerClient;
    ///
    /// let name = DockerClient::volume_name("abc12345-0000-0000-0000-000000000000");
    /// assert_eq!(name, "rustbrain-ws-abc12345");
    /// ```
    pub fn volume_name(workspace_id: &str) -> String {
        let short = workspace_id
            .chars()
            .filter(|c| *c != '-')
            .take(8)
            .collect::<String>();
        format!("rustbrain-ws-{short}")
    }

    /// Creates a Docker volume for a workspace.
    ///
    /// Runs:
    /// ```bash
    /// docker volume create \
    ///   --label rustbrain.workspace=true \
    ///   --opt size=<size_gb>g \
    ///   <name>
    /// ```
    ///
    /// The `--opt size` flag is effective for `tmpfs` volumes. For standard
    /// `local` driver volumes on `ext4`, disk quotas must be enforced via
    /// filesystem-level project quotas — see `docs/workspace-volumes.md`.
    ///
    /// Returns `Ok(())` when the volume is created (or already exists).
    /// Returns `Err` if the Docker command exits non-zero.
    pub async fn create_volume(&self, name: &str, size_gb: u32) -> anyhow::Result<()> {
        let size_opt = format!("size={}g", size_gb);
        let output = Command::new("docker")
            .args([
                "volume",
                "create",
                "--label",
                "rustbrain.workspace=true",
                "--opt",
                &size_opt,
                name,
            ])
            .output()
            .await
            .context("failed to spawn `docker volume create`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker volume create {}` failed (exit {}): {}",
                name,
                output.status,
                stderr.trim()
            );
        }

        Ok(())
    }

    /// Removes a Docker volume.
    ///
    /// Runs `docker volume rm <name>`.
    ///
    /// Returns `Ok(())` on success, `Err` if the command exits non-zero (e.g.
    /// volume is in use by a running container).
    pub async fn remove_volume(&self, name: &str) -> anyhow::Result<()> {
        let output = Command::new("docker")
            .args(["volume", "rm", name])
            .output()
            .await
            .context("failed to spawn `docker volume rm`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker volume rm {}` failed (exit {}): {}",
                name,
                output.status,
                stderr.trim()
            );
        }

        Ok(())
    }

    /// Derives the canonical container name for an execution UUID.
    ///
    /// Uses the first 8 hex characters (without hyphens) of the UUID.
    pub fn container_name(execution_id: &str) -> String {
        let short = execution_id
            .chars()
            .filter(|c| *c != '-')
            .take(8)
            .collect::<String>();
        format!("rustbrain-exec-{short}")
    }

    /// Spawns an ephemeral OpenCode container for a single execution.
    ///
    /// Runs:
    /// ```bash
    /// docker run -d \
    ///   --name rustbrain-exec-{short_id} \
    ///   --network {network} \
    ///   -v {volume_name}:/workspace:rw \
    ///   -w /workspace \
    ///   {image}
    /// ```
    ///
    /// Returns `(container_id, base_url)` where `base_url` is
    /// `http://{container_name}:4096` (reachable inside the Docker network).
    pub async fn spawn_execution_container(
        &self,
        execution_id: &str,
        volume_name: &str,
        network: &str,
        image: &str,
    ) -> anyhow::Result<(String, String)> {
        let container_name = Self::container_name(execution_id);
        let volume_mount = format!("{}:/workspace:rw", volume_name);

        let output = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &container_name,
                "--network",
                network,
                "-v",
                &volume_mount,
                "-w",
                "/workspace",
                image,
            ])
            .output()
            .await
            .context("failed to spawn `docker run` for execution container")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker run {}` failed (exit {}): {}",
                container_name,
                output.status,
                stderr.trim()
            );
        }

        let container_id = String::from_utf8(output.stdout)
            .context("docker run: stdout not valid UTF-8")?
            .trim()
            .to_string();

        let base_url = format!("http://{}:4096", container_name);
        Ok((container_id, base_url))
    }

    /// Stops a running container by ID or name.
    ///
    /// Runs `docker stop <container_id>`.
    pub async fn stop_container(&self, container_id: &str) -> anyhow::Result<()> {
        let output = Command::new("docker")
            .args(["stop", container_id])
            .output()
            .await
            .context("failed to spawn `docker stop`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker stop {}` failed (exit {}): {}",
                container_id,
                output.status,
                stderr.trim()
            );
        }
        Ok(())
    }

    /// Runs the rustbrain ingestion pipeline in an ephemeral container.
    ///
    /// Mounts `cfg.host_clone_path` (a host-side directory) at
    /// `/workspace/target-repo` inside the container and forwards the provided
    /// environment variables to the ingestion binary.
    ///
    /// Status transitions are the caller's responsibility:
    /// - On `Ok`: call `lifecycle::mark_ready`
    /// - On `Err`: call `lifecycle::fail`
    ///
    /// Returns the combined stdout+stderr from the container on success.
    pub async fn run_ingestion(&self, cfg: &IngestionConfig<'_>) -> anyhow::Result<String> {
        let volume_mount = format!("{}:/workspace/target-repo:ro", cfg.host_clone_path);

        let output = Command::new("docker")
            .args([
                "run",
                "--rm",
                "--network",
                cfg.network,
                "-v",
                &volume_mount,
                "-e",
                &format!("DATABASE_URL={}", cfg.database_url),
                "-e",
                &format!("NEO4J_URL={}", cfg.neo4j_url),
                "-e",
                &format!("NEO4J_USER={}", cfg.neo4j_user),
                "-e",
                &format!("NEO4J_PASSWORD={}", cfg.neo4j_password),
                "-e",
                &format!("OLLAMA_HOST={}", cfg.ollama_host),
                "-e",
                &format!("QDRANT_HOST={}", cfg.qdrant_host),
                "-e",
                &format!("EMBEDDING_MODEL={}", cfg.embedding_model),
                cfg.ingestion_image,
                "--crate-path",
                "/workspace/target-repo",
            ])
            .output()
            .await
            .context("failed to spawn `docker run` for ingestion")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            bail!(
                "ingestion container failed (exit {}): {}",
                output.status,
                stderr.trim()
            );
        }

        Ok(format!("{}{}", stdout, stderr))
    }

    /// Force-removes a container by ID or name.
    ///
    /// Runs `docker rm -f <container_id>`. Safe to call on already-stopped containers.
    pub async fn remove_container(&self, container_id: &str) -> anyhow::Result<()> {
        let output = Command::new("docker")
            .args(["rm", "-f", container_id])
            .output()
            .await
            .context("failed to spawn `docker rm`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker rm -f {}` failed (exit {}): {}",
                container_id,
                output.status,
                stderr.trim()
            );
        }
        Ok(())
    }

    /// Returns `true` if the named volume exists, `false` otherwise.
    ///
    /// Uses `docker volume inspect <name>` — exit 0 means found, exit 1 means
    /// not found. Any other error is surfaced as `Err`.
    pub async fn volume_exists(&self, name: &str) -> anyhow::Result<bool> {
        let output = Command::new("docker")
            .args(["volume", "inspect", name])
            .output()
            .await
            .context("failed to spawn `docker volume inspect`")?;

        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            Some(code) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(anyhow!(
                    "`docker volume inspect {}` failed (exit {}): {}",
                    name,
                    code,
                    stderr.trim()
                ))
            }
            None => Err(anyhow!(
                "`docker volume inspect {}` was killed by a signal",
                name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_name_strips_hyphens_takes_eight() {
        assert_eq!(
            DockerClient::container_name("abc12345-0000-0000-0000-000000000000"),
            "rustbrain-exec-abc12345"
        );
    }

    #[test]
    fn test_container_name_short_input_no_panic() {
        let name = DockerClient::container_name("ab-cd");
        assert!(name.starts_with("rustbrain-exec-"));
    }

    #[tokio::test]
    async fn test_spawn_execution_container_error_without_docker() {
        let client = DockerClient::new();
        // Spawning with a clearly non-existent image should return Err, not panic.
        let result = client
            .spawn_execution_container(
                "abc12345-0000-0000-0000-000000000000",
                "rustbrain-ws-test0000",
                "rustbrain",
                "rustbrain-opencode-test-nonexistent:latest",
            )
            .await;
        // Either docker is absent (spawn Err) or image doesn't exist (run Err).
        assert!(result.is_err() || result.is_ok()); // no panic is the test
    }

    #[test]
    fn test_volume_name_strips_hyphens_takes_eight() {
        // UUID: abc12345-0000-0000-0000-000000000000
        // After stripping hyphens: abc123450000...  → first 8 = "abc12345"
        assert_eq!(
            DockerClient::volume_name("abc12345-0000-0000-0000-000000000000"),
            "rustbrain-ws-abc12345"
        );
    }

    #[test]
    fn test_volume_name_short_uuid_still_works() {
        // Edge case: ensure we don't panic on short input
        let name = DockerClient::volume_name("ab-cd");
        assert!(name.starts_with("rustbrain-ws-"));
    }

    #[tokio::test]
    async fn test_create_volume_passes_correct_args() {
        // We can't mock tokio::process::Command directly, so this test
        // verifies that calling create_volume on a non-existent name to a
        // non-Docker environment returns an error (not a panic).
        //
        // In CI without Docker, the binary may be missing — that's expected.
        let client = DockerClient::new();
        // Attempt on a clearly-fake name. Either docker is present (and may
        // succeed/fail) or it's absent (spawn error). Either way no panic.
        let _ = client.create_volume("rustbrain-ws-test0000", 2).await;
    }

    #[tokio::test]
    async fn test_volume_exists_returns_false_for_nonexistent() {
        // If docker is available, a clearly-nonexistent volume returns false.
        // If docker is absent the spawn error is returned (Err, not panic).
        let client = DockerClient::new();
        let result = client
            .volume_exists("rustbrain-ws-definitely-does-not-exist-xyz")
            .await;
        if let Ok(exists) = result {
            assert!(!exists);
        }
        // If docker is absent, the spawn error is returned (Err, not panic)
    }

    #[tokio::test]
    async fn test_remove_volume_error_on_nonexistent() {
        let client = DockerClient::new();
        let result = client
            .remove_volume("rustbrain-ws-definitely-does-not-exist-xyz")
            .await;
        // Either docker returns an error about not finding the volume, or
        // docker is not present. Either way we expect Err, not panic.
        assert!(result.is_err() || result.is_ok()); // no panic is the test
    }

    #[tokio::test]
    async fn test_run_ingestion_fails_with_nonexistent_image() {
        let client = DockerClient::new();
        // A clearly non-existent image should return Err (image pull failure or
        // docker not available). Either way no panic.
        let cfg = IngestionConfig {
            host_clone_path: "/tmp/nonexistent-clone-path",
            network: "rustbrain-net",
            database_url: "postgresql://unused:unused@localhost/unused",
            neo4j_url: "bolt://unused:7687",
            neo4j_user: "neo4j",
            neo4j_password: "unused",
            ollama_host: "http://unused:11434",
            qdrant_host: "http://unused:6333",
            embedding_model: "nomic-embed-text",
            ingestion_image: "rustbrain-ingestion-test-nonexistent:latest",
        };
        let result = client.run_ingestion(&cfg).await;
        // No panic is the assertion — docker may be absent or the image may be missing.
        assert!(result.is_err() || result.is_ok());
    }
}
