//! Execution runner: container spawn, orchestrator flow, and event bridge.
//!
//! [`run_execution`] is the single entry point. It is spawned as a background
//! `tokio::task` by the `POST /workspaces/:id/execute` handler and drives the
//! full lifecycle:
//!
//! 1. Spawn an ephemeral OpenCode container with the workspace volume mounted.
//! 2. Create an OpenCode session inside that container.
//! 3. Drive four sequential agent phases (orchestrating → researching →
//!    planning → developing), writing `phase_change` events for each.
//! 4. Bridge OpenCode response parts into `agent_events` rows.
//! 5. Mark the execution `completed` (or `failed`) and clean up the container.

use std::time::Duration;

use chrono;
use serde_json::json;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use super::models::{
    complete_execution, fail_execution, insert_agent_event, set_agent_phase,
    set_container_expires_at, set_container_id, set_runtime_info, set_session_id,
    timeout_execution,
};
use crate::docker::{DockerClient, ExecutionConfig};
use crate::opencode::OpenCodeClient;

// =============================================================================
// Phase definitions
// =============================================================================

/// Agent phases executed in order.
const PHASES: &[(&str, &str)] = &[
    ("orchestrating", ORCHESTRATOR_PROMPT),
    ("researching", RESEARCH_PROMPT),
    ("planning", PLANNING_PROMPT),
    ("developing", DEVELOPMENT_PROMPT),
];

const ORCHESTRATOR_PROMPT: &str = "You are the orchestrator agent for a Rust codebase assistant. \
     Analyse the workspace and the user's request. \
     Identify which files are relevant, which architectural concerns apply, \
     and produce a concise task breakdown for the specialist agents.";

const RESEARCH_PROMPT: &str = "You are the research agent. Based on the orchestrator's analysis, \
     explore the relevant Rust source files in /workspace. \
     Summarise function signatures, module structure, and any existing tests \
     that touch the area described in the task.";

const PLANNING_PROMPT: &str =
    "You are the planning agent. Given the orchestrator's task breakdown and \
     the research summary, produce a step-by-step implementation plan. \
     Be precise about which files to create or modify and what changes to make.";

const DEVELOPMENT_PROMPT: &str = "You are the development agent. Execute the implementation plan. \
     Write idiomatic Rust code, update tests, and ensure `cargo check` passes. \
     Commit your changes to the workspace when done.";

// =============================================================================
// Container readiness check
// =============================================================================

/// Poll the OpenCode health endpoint until it responds or the deadline passes.
async fn wait_for_container(client: &OpenCodeClient, retries: u32, delay: Duration) -> bool {
    for _ in 0..retries {
        if client.health_check().await.unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(delay).await;
    }
    false
}

// =============================================================================
// Config for a single execution run
// =============================================================================

/// Parameters passed to [`run_execution`].
pub struct RunParams {
    pub execution_id: Uuid,
    /// Docker volume name containing the cloned workspace (e.g. `rustbrain-ws-abc12345`).
    pub volume_name: String,
    /// User-supplied prompt driving the execution.
    pub prompt: String,
    /// Docker network the container is started on (default: `rustbrain`).
    pub docker_network: String,
    /// OpenCode Docker image (default: `opencode:latest`).
    pub opencode_image: String,
    /// Optional Basic Auth username for the spawned OpenCode container.
    pub opencode_user: Option<String>,
    /// Optional Basic Auth password for the spawned OpenCode container.
    pub opencode_pass: Option<String>,
    /// Timeout in seconds for the entire execution (default: from config, typically 7200).
    pub timeout_secs: u32,
    /// Public hostname or IP for constructing externally-reachable container URLs.
    ///
    /// When set, the container's port is published to a random host port and the
    /// public endpoint is `http://{public_host}:{mapped_port}`.
    pub public_host: Option<String>,
    /// Keep-alive TTL in seconds. When > 0, the container is kept alive for this
    /// duration after successful execution instead of being removed immediately.
    pub keep_alive_secs: u64,
    /// Container readiness timeout in seconds (default: 60).
    pub ready_timeout_secs: u32,
    /// Host-side path to the OpenCode config directory for bind-mounting
    /// `opencode.json` and `.opencode/agents/` into execution containers.
    pub opencode_config_host_path: Option<String>,
    /// MCP-SSE URL passed to execution containers as an environment variable.
    pub mcp_sse_url: Option<String>,
}

// =============================================================================
// Main entry point
// =============================================================================

/// Run the full multi-agent execution flow in the background.
///
/// Called via `tokio::spawn`. All errors are captured and written to Postgres;
/// they do not propagate to the caller.
pub async fn run_execution(pool: PgPool, docker: DockerClient, params: RunParams) {
    let exec_id = params.execution_id;
    let timeout_secs = params.timeout_secs;
    let keep_alive_secs = params.keep_alive_secs;
    let timeout_duration = Duration::from_secs(timeout_secs as u64);

    info!(execution_id = %exec_id, timeout_secs = timeout_secs, "Spawning OpenCode container");

    let result = tokio::time::timeout(
        timeout_duration,
        run_execution_inner(pool.clone(), docker.clone(), params),
    )
    .await;

    match result {
        Ok(Ok(container_id)) => {
            if keep_alive_secs > 0 {
                let expires_at =
                    chrono::Utc::now() + chrono::Duration::seconds(keep_alive_secs as i64);
                if let Err(e) = set_container_expires_at(&pool, exec_id, expires_at).await {
                    warn!(execution_id = %exec_id, error = %e, "Failed to set container_expires_at");
                }
                let _ = insert_agent_event(
                    &pool,
                    exec_id,
                    "container_kept_alive",
                    json!({ "expires_at": expires_at.to_rfc3339(), "keep_alive_secs": keep_alive_secs }),
                )
                .await;
                info!(
                    execution_id = %exec_id,
                    container_id = %container_id,
                    keep_alive_secs = keep_alive_secs,
                    "Container kept alive for debugging"
                );
            } else {
                cleanup_container(&docker, &container_id, exec_id).await;
            }
        }
        Ok(Err(container_id_opt)) => {
            if let Some(cid) = container_id_opt {
                cleanup_container(&docker, &cid, exec_id).await;
            }
        }
        Err(_) => {
            warn!(execution_id = %exec_id, "Execution timed out after {}s", timeout_secs);
            let _ = timeout_execution(&pool, exec_id).await;
            let _ = insert_agent_event(
                &pool,
                exec_id,
                "error",
                json!({ "error": format!("Execution timed out after {}s", timeout_secs) }),
            )
            .await;
            if let Ok(Some(exec)) = super::models::get_execution(&pool, exec_id).await {
                if let Some(cid) = exec.container_id {
                    cleanup_container(&docker, &cid, exec_id).await;
                }
            }
        }
    }
}

/// Inner execution logic returning container_id for cleanup.
/// Returns `Ok(container_id)` on success, `Err(Some(container_id))` on failure.
async fn run_execution_inner(
    pool: PgPool,
    docker: DockerClient,
    params: RunParams,
) -> Result<String, Option<String>> {
    let exec_id = params.execution_id;

    // 1. Spawn container (publish port when public_host is configured)
    let publish_port = params.public_host.is_some();
    let exec_id_str = exec_id.to_string();
    let exec_cfg = ExecutionConfig {
        execution_id: &exec_id_str,
        volume_name: &params.volume_name,
        network: &params.docker_network,
        image: &params.opencode_image,
        publish_port,
        opencode_config_host_path: params.opencode_config_host_path.as_deref(),
        mcp_sse_url: params.mcp_sse_url.as_deref(),
    };
    let (container_id, internal_url, mapped_port) = match docker
        .spawn_execution_container(&exec_cfg)
        .await
    {
        Ok(tuple) => tuple,
        Err(e) => {
            let error_msg = classify_container_error(&e.to_string());
            warn!(execution_id = %exec_id, error = %error_msg, "Container spawn failed");
            let _ = insert_agent_event(
                &pool,
                exec_id,
                "error",
                json!({ "stage": "container_spawn", "error": &error_msg }),
            )
            .await;
            let _ = fail_execution(&pool, exec_id, &error_msg).await;
            return Err(None);
        }
    };

    // Record container_id immediately so the sweeper can kill it on timeout
    if let Err(e) = set_container_id(&pool, exec_id, &container_id).await {
        warn!(execution_id = %exec_id, error = %e, "Failed to persist container_id");
    }

    // Build public endpoint for external access (e.g. Tailscale)
    let public_endpoint = params
        .public_host
        .as_ref()
        .zip(mapped_port)
        .map(|(host, port)| format!("http://{}:{}", host, port));

    info!(
        execution_id = %exec_id,
        container_id = %container_id,
        internal_url = %internal_url,
        public_endpoint = ?public_endpoint,
        "Container started"
    );

    // 2. Wait for OpenCode to be ready (configurable timeout with retry)
    let opencode = OpenCodeClient::new(
        internal_url.clone(),
        params.opencode_user,
        params.opencode_pass,
    );
    let start = std::time::Instant::now();
    let ready =
        wait_for_container(&opencode, params.ready_timeout_secs, Duration::from_secs(1)).await;
    let elapsed = start.elapsed();

    if ready {
        info!(execution_id = %exec_id, elapsed_ms = elapsed.as_millis() as u64, "Container ready");
    } else {
        info!(execution_id = %exec_id, elapsed_ms = elapsed.as_millis() as u64, "Primary readiness check failed, retrying after 5s backoff");
        tokio::time::sleep(Duration::from_secs(5)).await;
        if !opencode.health_check().await.unwrap_or(false) {
            let total = start.elapsed();
            warn!(execution_id = %exec_id, elapsed_ms = total.as_millis() as u64, "Container readiness failed after retry");
            let _ = fail_execution(
                &pool,
                exec_id,
                &format!(
                    "OpenCode container did not become ready within {}s (+5s retry)",
                    params.ready_timeout_secs
                ),
            )
            .await;
            return Err(Some(container_id));
        }
        let total = start.elapsed();
        info!(execution_id = %exec_id, elapsed_ms = total.as_millis() as u64, "Container ready after retry");
    }

    // 3. Create an OpenCode session
    let (session_id, workspace_path) =
        match opencode.create_session(Some("rustbrain-execution")).await {
            Ok(s) => {
                if let Err(e) = set_session_id(&pool, exec_id, &s.id).await {
                    warn!(execution_id = %exec_id, error = %e, "Failed to persist session_id");
                }
                let dir = s.directory.clone();
                (s.id, dir)
            }
            Err(e) => {
                warn!(execution_id = %exec_id, error = %e, "Failed to create OpenCode session");
                let _ = fail_execution(
                    &pool,
                    exec_id,
                    &format!("OpenCode session creation failed: {e}"),
                )
                .await;
                return Err(Some(container_id));
            }
        };

    // 3b. Persist runtime info for the UI panel
    // Use public endpoint for opencode_endpoint so the UI can reach the container
    // from outside Docker (e.g. via Tailscale). Falls back to internal URL.
    let endpoint_for_db = public_endpoint
        .as_deref()
        .unwrap_or_else(|| opencode.base_url());
    if let Err(e) = set_runtime_info(
        &pool,
        exec_id,
        &params.volume_name,
        endpoint_for_db,
        workspace_path.as_deref(),
    )
    .await
    {
        warn!(execution_id = %exec_id, error = %e, "Failed to persist runtime info");
    }

    // 4. Drive each agent phase
    let full_prompt = &params.prompt;
    for (phase, phase_system_prompt) in PHASES {
        // Advance phase in DB and emit phase_change event
        if let Err(e) = set_agent_phase(&pool, exec_id, phase).await {
            warn!(execution_id = %exec_id, phase = phase, error = %e, "Failed to set agent_phase");
        }

        let _ = insert_agent_event(&pool, exec_id, "phase_change", json!({ "phase": phase })).await;

        info!(execution_id = %exec_id, phase = phase, "Running agent phase");

        // Build the prompt: prepend the system context then append the user task
        let message = format!("{}\n\n---\nUser task: {}", phase_system_prompt, full_prompt);

        match opencode.send_message(&session_id, &message).await {
            Ok(msg) => {
                // Bridge response parts as agent_events
                bridge_message(&pool, exec_id, phase, &msg.parts).await;
            }
            Err(e) => {
                warn!(execution_id = %exec_id, phase = phase, error = %e, "Agent phase failed");
                let _ = insert_agent_event(
                    &pool,
                    exec_id,
                    "error",
                    json!({ "phase": phase, "error": e.to_string() }),
                )
                .await;
                let _ =
                    fail_execution(&pool, exec_id, &format!("Phase '{phase}' failed: {e}")).await;
                return Err(Some(container_id));
            }
        }
    }

    // 5. Collect diff summary from the final session state
    let diff_summary = opencode
        .get_session(&session_id)
        .await
        .ok()
        .and_then(|s| s.summary)
        .map(|summary| {
            json!({
                "additions": summary.additions,
                "deletions": summary.deletions,
                "files": summary.files,
            })
        });

    // 6. Mark execution complete
    if let Err(e) = complete_execution(&pool, exec_id, diff_summary).await {
        warn!(execution_id = %exec_id, error = %e, "Failed to mark execution complete");
    }

    info!(execution_id = %exec_id, "Execution completed successfully");

    Ok(container_id)
}

// =============================================================================
// Helpers
// =============================================================================

/// Bridge OpenCode message parts into `agent_events` rows.
async fn bridge_message(
    pool: &PgPool,
    execution_id: Uuid,
    phase: &str,
    parts: &[crate::opencode::MessagePart],
) {
    use crate::opencode::MessagePart;

    for part in parts {
        let (event_type, content) = match part {
            MessagePart::Text { text } => ("reasoning", json!({ "phase": phase, "text": text })),
            MessagePart::Reasoning { text } => {
                ("reasoning", json!({ "phase": phase, "reasoning": text }))
            }
            MessagePart::ToolInvocation {
                tool_name,
                args,
                result,
            } => (
                "tool_call",
                json!({
                    "phase": phase,
                    "tool": tool_name,
                    "args": args,
                    "result": result,
                }),
            ),
            MessagePart::StepStart { id } => {
                ("reasoning", json!({ "phase": phase, "step_start": id }))
            }
            MessagePart::StepFinish { reason } => (
                "reasoning",
                json!({ "phase": phase, "step_finish": reason }),
            ),
            MessagePart::Unknown => continue,
        };

        if let Err(e) = insert_agent_event(pool, execution_id, event_type, content).await {
            warn!(execution_id = %execution_id, error = %e, "Failed to insert agent_event");
        }
    }
}

fn classify_container_error(error: &str) -> String {
    let error_lower = error.to_lowercase();
    if error_lower.contains("port is already allocated")
        || error_lower.contains("bind: address already in use")
    {
        "Port conflict: container could not start because a required port is already in use. \
         Check for conflicting Docker containers or services."
            .to_string()
    } else if error_lower.contains("no such image")
        || error_lower.contains("image") && error_lower.contains("not found")
    {
        "Image not found: the OpenCode Docker image is not available. \
         Pull or build the image before starting executions."
            .to_string()
    } else if error_lower.contains("oom")
        || error_lower.contains("out of memory")
        || error_lower.contains("memory")
    {
        "Out of memory: container was killed due to memory limits. \
         Consider increasing Docker memory allocation or reducing workload size."
            .to_string()
    } else if error_lower.contains("network") && error_lower.contains("not found") {
        "Network not found: the Docker network does not exist. \
         Create it with `docker network create rustbrain`."
            .to_string()
    } else if error_lower.contains("volume") && error_lower.contains("not found") {
        "Volume not found: the workspace volume does not exist. \
         Ensure the workspace was properly initialized."
            .to_string()
    } else if error_lower.contains("permission denied") {
        "Permission denied: Docker socket access denied. \
         Ensure the user is in the docker group or run with appropriate privileges."
            .to_string()
    } else if error_lower.contains("connection refused") || error_lower.contains("cannot connect") {
        "Docker daemon unreachable: ensure Docker is running and accessible.".to_string()
    } else {
        format!("Container spawn failed: {}", error.trim())
    }
}

/// Stop and remove the OpenCode container, logging any errors.
async fn cleanup_container(docker: &DockerClient, container_id: &str, execution_id: Uuid) {
    if let Err(e) = docker.remove_container(container_id).await {
        warn!(
            execution_id = %execution_id,
            container_id = %container_id,
            error = %e,
            "Failed to remove container on cleanup"
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_ordered_correctly() {
        let names: Vec<&str> = PHASES.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec!["orchestrating", "researching", "planning", "developing"]
        );
    }

    #[test]
    fn phase_prompts_non_empty() {
        for (phase, prompt) in PHASES {
            assert!(!prompt.is_empty(), "Prompt for phase '{phase}' is empty");
        }
    }

    #[test]
    fn run_params_volume_name_stored() {
        let p = RunParams {
            execution_id: Uuid::new_v4(),
            volume_name: "rustbrain-ws-abc12345".into(),
            prompt: "fix the bug".into(),
            docker_network: "rustbrain".into(),
            opencode_image: "opencode:latest".into(),
            opencode_user: None,
            opencode_pass: None,
            timeout_secs: 7200,
            public_host: None,
            keep_alive_secs: 0,
            ready_timeout_secs: 60,
            opencode_config_host_path: None,
            mcp_sse_url: None,
        };
        assert_eq!(p.volume_name, "rustbrain-ws-abc12345");
        assert_eq!(p.docker_network, "rustbrain");
        assert_eq!(p.timeout_secs, 7200);
        assert!(p.public_host.is_none());
        assert_eq!(p.keep_alive_secs, 0);
        assert_eq!(p.ready_timeout_secs, 60);
    }

    #[test]
    fn run_params_keep_alive_set() {
        let p = RunParams {
            execution_id: Uuid::new_v4(),
            volume_name: "rustbrain-ws-abc12345".into(),
            prompt: "fix the bug".into(),
            docker_network: "rustbrain".into(),
            opencode_image: "opencode:latest".into(),
            opencode_user: None,
            opencode_pass: None,
            timeout_secs: 7200,
            public_host: None,
            keep_alive_secs: 1800,
            ready_timeout_secs: 90,
            opencode_config_host_path: Some("/opt/configs/opencode".into()),
            mcp_sse_url: Some("http://mcp-sse:3001/sse".into()),
        };
        assert_eq!(p.keep_alive_secs, 1800);
        assert_eq!(p.ready_timeout_secs, 90);
        assert_eq!(
            p.opencode_config_host_path.as_deref(),
            Some("/opt/configs/opencode")
        );
        assert_eq!(p.mcp_sse_url.as_deref(), Some("http://mcp-sse:3001/sse"));
    }

    #[test]
    fn classify_port_conflict_error() {
        let msg = classify_container_error("Error: port is already allocated");
        assert!(msg.starts_with("Port conflict"));
    }

    #[test]
    fn classify_image_not_found_error() {
        let msg = classify_container_error("docker: no such image: opencode:latest");
        assert!(msg.starts_with("Image not found"));
    }

    #[test]
    fn classify_oom_error() {
        let msg = classify_container_error("OOMKilled");
        assert!(msg.starts_with("Out of memory"));
    }

    #[test]
    fn classify_network_not_found_error() {
        let msg = classify_container_error("Error: network rustbrain not found");
        assert!(msg.starts_with("Network not found"));
    }

    #[test]
    fn classify_permission_denied_error() {
        let msg = classify_container_error("permission denied while trying to connect");
        assert!(msg.starts_with("Permission denied"));
    }

    #[test]
    fn classify_unknown_error() {
        let msg = classify_container_error("something weird happened");
        assert!(msg.contains("something weird happened"));
    }
}
