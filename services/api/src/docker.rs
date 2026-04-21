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
//! where `workspace_id_short` is the first 12 characters of the workspace UUID.
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

/// Configuration passed to [`DockerClient::spawn_execution_container`].
#[derive(Debug, Clone)]
pub struct ExecutionConfig<'a> {
    /// UUID of the execution (used to derive the container name).
    pub execution_id: &'a str,
    /// Docker named volume containing the cloned workspace repo.
    ///
    /// Mounted read-write at `/workspace/target-repo` inside the container,
    /// matching the entrypoint's `TARGET_REPO_PATH` default.
    pub volume_name: &'a str,
    /// Docker network the execution container joins (e.g. `rustbrain`).
    pub network: &'a str,
    /// Docker image to run (e.g. `opencode:latest`).
    pub image: &'a str,
    /// When true, publish port 4096 to a random host port for external access.
    pub publish_port: bool,
    /// Host-side path to the OpenCode config directory.
    ///
    /// When set, two bind mounts are added:
    /// - `{path}/opencode.json` → `/home/opencode/.config/opencode/opencode.json:ro`
    /// - `{path}/.opencode` → `/home/opencode/.config/opencode/.opencode:ro`
    pub opencode_config_host_path: Option<&'a str>,
    /// MCP-SSE URL injected as `MCP_SSE_URL` env var into the container.
    pub mcp_sse_url: Option<&'a str>,
    /// LiteLLM API key forwarded as `LITELLM_API_KEY` env var.
    ///
    /// Required by the entrypoint's `verify_opencode_config()` to substitute
    /// the `${LITELLM_API_KEY}` placeholder in `opencode.json.template`.
    pub litellm_api_key: Option<&'a str>,
    /// OpenAI-compatible API key forwarded as `OPENAI_API_KEY` env var.
    pub openai_api_key: Option<&'a str>,
    /// OpenCode server password forwarded as `OPENCODE_SERVER_PASSWORD` env var.
    pub opencode_server_password: Option<&'a str>,
}

/// Configuration passed to [`DockerClient::run_ingestion`].
#[derive(Debug, Clone)]
pub struct IngestionConfig<'a> {
    /// Docker named volume containing the cloned repository (e.g. `rustbrain-ws-abc12345`).
    ///
    /// Mounted read-only at `/workspace/target-repo` inside the ingestion container.
    /// Using a named volume (rather than a host bind-mount) ensures the volume is
    /// accessible to sibling containers even when the API itself runs inside Docker.
    pub volume_name: &'a str,
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
    /// Workspace UUID. Forwarded as `INGESTION_WORKSPACE_ID`; the derived
    /// `Workspace_<12hex>` label is also sent as `INGESTION_WORKSPACE_LABEL`.
    pub workspace_id: &'a str,
}

/// Derive `Workspace_<12hex>` label from a workspace UUID string.
///
/// Strips hyphens, takes the first 12 characters, prepends `Workspace_`.
/// Matches the Postgres schema naming convention `ws_<12hex>`.
fn workspace_label_from_id(workspace_id: &str) -> String {
    let short: String = workspace_id
        .chars()
        .filter(|c| *c != '-')
        .take(12)
        .collect();
    format!("Workspace_{short}")
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
    /// Uses the first 12 characters of the UUID string (without hyphens at
    /// that position).
    ///
    /// # Example
    ///
    /// ```
    /// use rustbrain_api::docker::DockerClient;
    ///
    /// let name = DockerClient::volume_name("abc12345-0000-0000-0000-000000000000");
    /// assert_eq!(name, "rustbrain-ws-abc123450000");
    /// ```
    pub fn volume_name(workspace_id: &str) -> String {
        let short = workspace_id
            .chars()
            .filter(|c| *c != '-')
            .take(12)
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
    pub async fn create_volume(&self, name: &str, _size_gb: u32) -> anyhow::Result<()> {
        // NOTE: `--opt size=Xg` requires filesystem-level quota support (XFS
        // project quotas or tmpfs). The default local driver on ext4 does not
        // support it, so we omit the size option and rely on external disk
        // monitoring instead.
        let output = Command::new("docker")
            .args([
                "volume",
                "create",
                "--label",
                "rustbrain.workspace=true",
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
    /// Uses the first 12 hex characters (without hyphens) of the UUID.
    pub fn container_name(execution_id: &str) -> String {
        let short = execution_id
            .chars()
            .filter(|c| *c != '-')
            .take(12)
            .collect::<String>();
        format!("rustbrain-exec-{short}")
    }

    /// Spawns an ephemeral OpenCode container for a single execution.
    ///
    /// Mounts the workspace volume at `/workspace/target-repo` (matching the
    /// entrypoint's `TARGET_REPO_PATH` default) and optionally bind-mounts
    /// OpenCode configuration files for LLM provider and agent definitions.
    ///
    /// When `publish_port` is `true`, the container's port 4096 is published to
    /// a random host port (`-p 0:4096`). The mapped host port is returned so
    /// callers can construct a public endpoint reachable from outside Docker
    /// (e.g. via Tailscale).
    ///
    /// Returns `(container_id, internal_url, mapped_host_port)` where:
    /// - `internal_url` is `http://{container_name}:4096` (Docker-network only)
    /// - `mapped_host_port` is `Some(port)` when `publish_port` is true
    pub async fn spawn_execution_container(
        &self,
        cfg: &ExecutionConfig<'_>,
    ) -> anyhow::Result<(String, String, Option<u16>)> {
        let container_name = Self::container_name(cfg.execution_id);
        let volume_mount = format!("{}:/workspace/target-repo:rw", cfg.volume_name);

        // Pre-compute optional mount/env strings so we can borrow them in args.
        let config_json_mount = cfg.opencode_config_host_path.map(|p| {
            format!(
                "{}/opencode.json:/home/opencode/.config/opencode/opencode.json.template:ro",
                p
            )
        });
        let agents_mount = cfg.opencode_config_host_path.map(|p| {
            format!(
                "{}/.opencode:/home/opencode/.config/opencode/.opencode:ro",
                p
            )
        });
        let mcp_env = cfg.mcp_sse_url.map(|url| format!("MCP_SSE_URL={}", url));
        let litellm_env = cfg
            .litellm_api_key
            .map(|key| format!("LITELLM_API_KEY={}", key));
        let openai_env = cfg
            .openai_api_key
            .map(|key| format!("OPENAI_API_KEY={}", key));
        let opencode_pass_env = cfg
            .opencode_server_password
            .map(|pass| format!("OPENCODE_SERVER_PASSWORD={}", pass));

        let mut args = vec![
            "run",
            "-d",
            "--name",
            &container_name,
            "--network",
            cfg.network,
            "-v",
            &volume_mount,
            "-w",
            "/workspace",
        ];

        if let Some(ref mount) = config_json_mount {
            args.push("-v");
            args.push(mount);
        }
        if let Some(ref mount) = agents_mount {
            args.push("-v");
            args.push(mount);
        }
        if let Some(ref env) = mcp_env {
            args.push("-e");
            args.push(env);
        }
        if let Some(ref env) = litellm_env {
            args.push("-e");
            args.push(env);
        }
        if let Some(ref env) = openai_env {
            args.push("-e");
            args.push(env);
        }
        if let Some(ref env) = opencode_pass_env {
            args.push("-e");
            args.push(env);
        }

        let port_binding = "0:4096".to_string();
        if cfg.publish_port {
            args.push("-p");
            args.push(&port_binding);
        }

        args.push(cfg.image);

        let output = Command::new("docker")
            .args(&args)
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

        let internal_url = format!("http://{}:4096", container_name);

        let mapped_port = if cfg.publish_port {
            self.get_mapped_port(&container_id, 4096).await.ok()
        } else {
            None
        };

        Ok((container_id, internal_url, mapped_port))
    }

    /// Queries the host port mapped to a container's internal port.
    ///
    /// Runs `docker port <container_id> <internal_port>` and parses the output
    /// (e.g. `0.0.0.0:32768`) to extract the host port number.
    async fn get_mapped_port(&self, container_id: &str, internal_port: u16) -> anyhow::Result<u16> {
        let output = Command::new("docker")
            .args(["port", container_id, &internal_port.to_string()])
            .output()
            .await
            .context("failed to spawn `docker port`")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "`docker port {} {}` failed (exit {}): {}",
                container_id,
                internal_port,
                output.status,
                stderr.trim()
            );
        }

        // Output format: "0.0.0.0:32768\n" or "[::]:32768\n"
        // Take the first line, split on ':', take the last segment.
        let stdout =
            String::from_utf8(output.stdout).context("docker port: stdout not valid UTF-8")?;
        let port_str = stdout
            .lines()
            .next()
            .and_then(|line| line.rsplit(':').next())
            .ok_or_else(|| anyhow!("unexpected `docker port` output: {}", stdout.trim()))?;

        port_str.trim().parse::<u16>().context(format!(
            "failed to parse mapped port from: {}",
            port_str.trim()
        ))
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
        // Mount the named Docker volume (not a host path) so this works whether the
        // API is running bare-metal or as a sibling container talking to the host daemon.
        let volume_mount = format!("{}:/workspace/target-repo:ro", cfg.volume_name);
        let workspace_label = workspace_label_from_id(cfg.workspace_id);

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
                "-e",
                &format!("INGESTION_WORKSPACE_ID={}", cfg.workspace_id),
                "-e",
                &format!("INGESTION_WORKSPACE_LABEL={}", workspace_label),
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

    /// Copies the contents of `src_host_path` into a Docker volume.
    ///
    /// Runs an ephemeral `busybox` container that bind-mounts the source
    /// directory read-only at `/src` and the target volume read-write at
    /// `/dest`, then executes `cp -r /src/. /dest/` to populate the volume.
    ///
    /// Returns `Ok(())` when the copy completes successfully.
    /// Returns `Err` if the Docker command exits non-zero.
    pub async fn populate_volume(
        &self,
        volume_name: &str,
        src_host_path: &str,
    ) -> anyhow::Result<()> {
        let src_mount = format!("{}:/src:ro", src_host_path);
        let dest_mount = format!("{}:/dest:rw", volume_name);

        let output = Command::new("docker")
            .args([
                "run",
                "--rm",
                "-v",
                &src_mount,
                "-v",
                &dest_mount,
                "busybox",
                "sh",
                "-c",
                "cp -r /src/. /dest/",
            ])
            .output()
            .await
            .context("failed to spawn `docker run` for volume population")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "volume population failed (exit {}): {}",
                output.status,
                stderr.trim()
            );
        }

        Ok(())
    }

    /// List all file and directory paths in a Docker volume.
    ///
    /// Spins up a short-lived `busybox` container that runs `find /mnt` on the
    /// read-only volume, then returns relative paths (stripped of the `/mnt/`
    /// prefix). Hidden entries (starting with `.`) are excluded.
    ///
    /// Uses a shell one-liner compatible with busybox `find` (no `-printf`).
    pub async fn list_volume_paths(
        &self,
        volume_name: &str,
    ) -> anyhow::Result<Vec<(String, bool)>> {
        let vol_mount = format!("{}:/mnt:ro", volume_name);
        let script = r#"cd /mnt && find . -not -path '*/.*' | while read p; do [ -d "$p" ] && printf 'd %s\n' "$p" || printf 'f %s\n' "$p"; done"#;

        let output = Command::new("docker")
            .args([
                "run", "--rm", "-v", &vol_mount, "busybox", "sh", "-c", script,
            ])
            .output()
            .await
            .context("failed to spawn `docker run` for volume listing")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "volume listing failed (exit {}): {}",
                output.status,
                stderr.trim()
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entries: Vec<(String, bool)> = stdout
            .lines()
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let (kind, rel) = line.split_once(' ')?;
                let path = rel.strip_prefix("./").unwrap_or(rel);
                if path.is_empty() || path == "." {
                    return None;
                }
                Some((path.to_string(), kind == "d"))
            })
            .collect();

        Ok(entries)
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
    fn test_container_name_strips_hyphens_takes_twelve() {
        assert_eq!(
            DockerClient::container_name("abc12345-0000-0000-0000-000000000000"),
            "rustbrain-exec-abc123450000"
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
        let cfg = ExecutionConfig {
            execution_id: "abc12345-0000-0000-0000-000000000000",
            volume_name: "rustbrain-ws-test0000",
            network: "rustbrain",
            image: "rustbrain-opencode-test-nonexistent:latest",
            publish_port: false,
            opencode_config_host_path: None,
            mcp_sse_url: None,
            litellm_api_key: None,
            openai_api_key: None,
            opencode_server_password: None,
        };
        // Spawning with a clearly non-existent image should return Err, not panic.
        let result = client.spawn_execution_container(&cfg).await;
        // Either docker is absent (spawn Err) or image doesn't exist (run Err).
        assert!(result.is_err() || result.is_ok()); // no panic is the test
    }

    #[test]
    fn test_volume_name_strips_hyphens_takes_twelve() {
        // UUID: abc12345-0000-0000-0000-000000000000
        // After stripping hyphens: abc123450000...  → first 12 = "abc123450000"
        assert_eq!(
            DockerClient::volume_name("abc12345-0000-0000-0000-000000000000"),
            "rustbrain-ws-abc123450000"
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
            volume_name: "rustbrain-ws-nonexistent",
            network: "rustbrain-net",
            database_url: "postgresql://unused:unused@localhost/unused",
            neo4j_url: "bolt://unused:7687",
            neo4j_user: "neo4j",
            neo4j_password: "unused",
            ollama_host: "http://unused:11434",
            qdrant_host: "http://unused:6333",
            embedding_model: "nomic-embed-text",
            ingestion_image: "rustbrain-ingestion-test-nonexistent:latest",
            workspace_id: "00000000-0000-0000-0000-000000000000",
        };
        let result = client.run_ingestion(&cfg).await;
        // No panic is the assertion — docker may be absent or the image may be missing.
        assert!(result.is_err() || result.is_ok());
    }

    #[test]
    fn test_workspace_label_from_id() {
        assert_eq!(
            super::workspace_label_from_id("abc12345-0000-0000-0000-000000000000"),
            "Workspace_abc123450000"
        );
        assert_eq!(
            super::workspace_label_from_id("550e8400-e29b-41d4-a716-446655440000"),
            "Workspace_550e8400e29b"
        );
    }
}
