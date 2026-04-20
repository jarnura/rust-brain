//! Execution runner: container spawn, orchestrator flow, and event bridge.
//!
//! [`run_execution`] is the single entry point. It is spawned as a background
//! `tokio::task` by the `POST /workspaces/:id/execute` handler and drives the
//! full lifecycle:
//!
//! 1. Spawn an ephemeral OpenCode container with the workspace volume mounted.
//! 2. Create an OpenCode session inside that container.
//! 3. Send the user prompt to the OpenCode orchestrator agent (blocking call)
//!    and bridge response parts into `agent_dispatch` events.
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
// Polling configuration
// =============================================================================

/// Interval between polling calls to OpenCode for new message parts.
const POLL_INTERVAL_SECS: u64 = 2;

/// Maximum time to wait for new parts before logging a stall warning.
const STALL_TIMEOUT_SECS: u64 = 60;

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
    let (container_id, internal_url, mapped_port) =
        match docker.spawn_execution_container(&exec_cfg).await {
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

    // 4. Send prompt to orchestrator and poll for intermediate events
    //
    // OpenCode's POST /session/{id}/message blocks until the LLM finishes.
    // Intermediate ToolInvocation parts (subagent dispatches) are only visible
    // via GET /session/{id}/message while the response is being generated.
    // We spawn the blocking send in a background task and poll concurrently.
    if let Err(e) = set_agent_phase(&pool, exec_id, "orchestrator").await {
        warn!(execution_id = %exec_id, error = %e, "Failed to set initial agent_phase");
    }

    let send_opencode = opencode.clone();
    let send_session = session_id.clone();
    let send_prompt = params.prompt.clone();
    let send_exec_id = exec_id;
    let send_task = tokio::spawn(async move {
        send_opencode
            .send_message(&send_session, &send_prompt)
            .await
            .map_err(|e| {
                warn!(execution_id = %send_exec_id, error = %e, "Background send_message failed");
                e
            })
    });

    info!(execution_id = %exec_id, "Message send spawned, starting polling loop");

    // Poll for intermediate parts while the send runs in the background
    let mut last_seen_part_count: usize = 0;
    let mut current_agent = "orchestrator".to_string();
    let mut last_new_part_time = std::time::Instant::now();
    let mut stall_logged = false;

    loop {
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_SECS)).await;

        // Check if the background send completed (success or failure)
        if send_task.is_finished() {
            // Do one final poll to capture any remaining parts from ALL assistant messages.
            if let Ok(messages) = opencode.get_messages(&session_id).await {
                let all_parts: Vec<&crate::opencode::MessagePart> = messages
                    .iter()
                    .filter(|m| m.role == "assistant")
                    .flat_map(|m| m.parts.iter())
                    .collect();
                if all_parts.len() > last_seen_part_count {
                    let new_parts = &all_parts[last_seen_part_count..];
                    // Use owned slice for bridge_new_parts
                    let owned: Vec<crate::opencode::MessagePart> =
                        new_parts.iter().map(|p| (*p).clone()).collect();
                    if let Some(new_agent) =
                        bridge_new_parts(&pool, exec_id, &current_agent, &owned).await
                    {
                        current_agent = new_agent;
                    }
                }
            }
            // Now check the send result
            match send_task.await {
                Ok(Ok(_)) => {
                    info!(execution_id = %exec_id, "Orchestrator send completed");
                }
                Ok(Err(e)) => {
                    let _ =
                        fail_execution(&pool, exec_id, &format!("Orchestrator failed: {e}")).await;
                    return Err(Some(container_id));
                }
                Err(e) => {
                    let _ =
                        fail_execution(&pool, exec_id, &format!("Send task panicked: {e}")).await;
                    return Err(Some(container_id));
                }
            }
            break;
        }

        // Poll for intermediate message parts across ALL assistant messages.
        //
        // OpenCode produces multiple assistant messages in a multi-turn flow
        // (one per orchestrator turn). We count total parts across all assistant
        // messages so we bridge events from every turn.
        let messages = match opencode.get_messages(&session_id).await {
            Ok(msgs) => msgs,
            Err(e) => {
                warn!(execution_id = %exec_id, error = %e, "Failed to get messages");
                continue;
            }
        };

        let all_parts: Vec<&crate::opencode::MessagePart> = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .flat_map(|m| m.parts.iter())
            .collect();
        let total_part_count = all_parts.len();

        if total_part_count > last_seen_part_count {
            last_new_part_time = std::time::Instant::now();
            stall_logged = false;

            let new_parts: Vec<&crate::opencode::MessagePart> =
                all_parts[last_seen_part_count..].to_vec();
            // bridge_new_parts expects &[MessagePart], collect owned refs
            for part in &new_parts {
                let (event_type, content) = match *part {
                    crate::opencode::MessagePart::Text { ref text } => (
                        "reasoning",
                        json!({ "agent": &current_agent, "text": text }),
                    ),
                    crate::opencode::MessagePart::Reasoning { ref text } => (
                        "reasoning",
                        json!({ "agent": &current_agent, "reasoning": text }),
                    ),
                    crate::opencode::MessagePart::ToolInvocation {
                        ref tool_name,
                        ref args,
                        ref result,
                        ref state,
                    } => {
                        if let Some(name) = tool_name {
                            if name == "task" {
                                if let Some(dispatched_agent) =
                                    extract_dispatched_agent_name(args, state)
                                {
                                    if let Err(e) =
                                        set_agent_phase(&pool, exec_id, &dispatched_agent).await
                                    {
                                        warn!(execution_id = %exec_id, error = %e, "Failed to set agent_phase in poll loop");
                                    }
                                    match insert_agent_event(
                                        &pool,
                                        exec_id,
                                        "agent_dispatch",
                                        json!({ "agent": &dispatched_agent }),
                                    )
                                    .await
                                    {
                                        Ok(ev) => {
                                            info!(execution_id = %exec_id, event_id = ev.id, agent = %dispatched_agent, "Inserted agent_dispatch in poll loop");
                                        }
                                        Err(e) => {
                                            warn!(execution_id = %exec_id, error = %e, agent = %dispatched_agent, "Failed to insert agent_dispatch in poll loop");
                                        }
                                    }
                                    current_agent = dispatched_agent;
                                }
                            }
                        }
                        (
                            "tool_call",
                            json!({
                                "agent": &current_agent,
                                "tool": tool_name,
                                "args": args.as_ref().or(state.as_ref()
                                    .and_then(|s| s.get("input"))),
                                "result": result.as_ref().or(state.as_ref()
                                    .and_then(|s| s.get("output"))),
                            }),
                        )
                    }
                    crate::opencode::MessagePart::StepStart { ref id } => (
                        "reasoning",
                        json!({ "agent": &current_agent, "step_start": id }),
                    ),
                    crate::opencode::MessagePart::StepFinish { ref reason } => (
                        "reasoning",
                        json!({ "agent": &current_agent, "step_finish": reason }),
                    ),
                    crate::opencode::MessagePart::Unknown {
                        ref raw_type,
                        ref raw,
                    } => {
                        warn!(
                            execution_id = %exec_id,
                            raw_type = %raw_type,
                            "Unknown MessagePart type encountered, persisting as opaque event"
                        );
                        ("unknown", json!({ "raw_type": raw_type, "raw": raw }))
                    }
                };

                if let Err(e) = insert_agent_event(&pool, exec_id, event_type, content).await {
                    warn!(execution_id = %exec_id, error = %e, "Failed to insert agent_event");
                }
            }
            last_seen_part_count = total_part_count;
        } else if last_new_part_time.elapsed().as_secs() > STALL_TIMEOUT_SECS && !stall_logged {
            stall_logged = true;
            warn!(execution_id = %exec_id, "No new parts for {}s, send still running", STALL_TIMEOUT_SECS);
        }
    }

    // 4b. Post-completion agent detection pass.
    //
    // During polling, ToolInvocation parts may have incomplete `state` (the
    // subagent hasn't finished yet, so `state.input.subagent_type` is absent).
    // Now that the send is complete, re-scan ALL parts for agent dispatches
    // with fully populated state data.
    //
    // IMPORTANT: OpenCode returns multiple assistant messages (one per turn).
    // Tool dispatches may be in any assistant message, so scan ALL of them.
    if let Ok(final_messages) = opencode.get_messages(&session_id).await {
        let all_assistant_parts: Vec<&crate::opencode::MessagePart> = final_messages
            .iter()
            .filter(|m| m.role == "assistant")
            .flat_map(|m| m.parts.iter())
            .collect();
        let detected = detect_agent_dispatches(&pool, exec_id, &all_assistant_parts).await;
        if !detected.is_empty() {
            info!(
                execution_id = %exec_id,
                agents = ?detected,
                "Post-completion agent detection found dispatches"
            );
        }
    }

    info!(execution_id = %exec_id, final_agent = %current_agent, "Execution events bridged");

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

async fn bridge_new_parts(
    pool: &PgPool,
    execution_id: Uuid,
    current_agent: &str,
    parts: &[crate::opencode::MessagePart],
) -> Option<String> {
    use crate::opencode::MessagePart;

    let mut new_agent: Option<String> = None;

    for part in parts {
        let (event_type, content) = match part {
            MessagePart::Text { text } => {
                ("reasoning", json!({ "agent": current_agent, "text": text }))
            }
            MessagePart::Reasoning { text } => (
                "reasoning",
                json!({ "agent": current_agent, "reasoning": text }),
            ),
            MessagePart::ToolInvocation {
                tool_name,
                args,
                result,
                state,
            } => {
                if let Some(name) = tool_name {
                    if name == "task" {
                        if let Some(dispatched_agent) = extract_dispatched_agent_name(args, state) {
                            if let Err(e) =
                                set_agent_phase(pool, execution_id, &dispatched_agent).await
                            {
                                warn!(execution_id = %execution_id, error = %e, "Failed to set agent_phase in bridge");
                            }
                            if let Err(e) = insert_agent_event(
                                pool,
                                execution_id,
                                "agent_dispatch",
                                json!({ "agent": &dispatched_agent }),
                            )
                            .await
                            {
                                warn!(execution_id = %execution_id, error = %e, agent = %dispatched_agent, "Failed to insert agent_dispatch in bridge");
                            }
                            new_agent = Some(dispatched_agent);
                        }
                    }
                }

                (
                    "tool_call",
                    json!({
                        "agent": current_agent,
                        "tool": tool_name,
                        "args": args.as_ref().or(state.as_ref()
                            .and_then(|s| s.get("input"))),
                        "result": result.as_ref().or(state.as_ref()
                            .and_then(|s| s.get("output"))),
                    }),
                )
            }
            MessagePart::StepStart { id } => (
                "reasoning",
                json!({ "agent": current_agent, "step_start": id }),
            ),
            MessagePart::StepFinish { reason } => (
                "reasoning",
                json!({ "agent": current_agent, "step_finish": reason }),
            ),
            MessagePart::Unknown {
                ref raw_type,
                ref raw,
            } => {
                warn!(
                    execution_id = %execution_id,
                    raw_type = %raw_type,
                    "Unknown MessagePart type in bridge, persisting as opaque event"
                );
                ("unknown", json!({ "raw_type": raw_type, "raw": raw }))
            }
        };

        if let Err(e) = insert_agent_event(pool, execution_id, event_type, content).await {
            warn!(execution_id = %execution_id, error = %e, "Failed to insert agent_event");
        }
    }

    new_agent
}

/// Extract the dispatched agent name from either the legacy `args` format
/// or the OpenCode `state.input` format.
fn extract_dispatched_agent_name(
    args: &Option<serde_json::Value>,
    state: &Option<serde_json::Value>,
) -> Option<String> {
    // Legacy format: args.subagent_type or args.category
    if let Some(name) = args.as_ref().and_then(|obj| {
        obj.get("subagent_type")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("category").and_then(|v| v.as_str()))
    }) {
        return Some(name.to_string());
    }
    // OpenCode format: state.input.subagent_type or state.input.agent
    if let Some(input) = state.as_ref().and_then(|s| s.get("input")) {
        if let Some(name) = input
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("agent").and_then(|v| v.as_str()))
        {
            return Some(name.to_string());
        }
    }
    None
}

/// Post-completion scan of ALL response parts for `task()` tool invocations.
///
/// During real-time polling, `state.input.subagent_type` may not yet be
/// populated (the subagent is still running). This function runs after the
/// blocking `send_message` completes, when all parts are fully resolved.
/// It emits `agent_dispatch` events for any dispatches not already recorded.
///
/// Accepts `&[&MessagePart]` to allow scanning parts from multiple messages.
async fn detect_agent_dispatches(
    pool: &PgPool,
    execution_id: Uuid,
    parts: &[&crate::opencode::MessagePart],
) -> Vec<String> {
    use crate::opencode::MessagePart;

    let mut detected = Vec::new();

    for part in parts {
        if let MessagePart::ToolInvocation {
            tool_name: Some(name),
            args,
            state,
            ..
        } = *part
        {
            if name == "task" {
                if let Some(agent) = extract_dispatched_agent_name(args, state) {
                    if !detected.contains(&agent) {
                        info!(execution_id = %execution_id, agent = %agent, "Detected agent dispatch");
                        if let Err(e) = set_agent_phase(pool, execution_id, &agent).await {
                            warn!(execution_id = %execution_id, error = %e, "Failed to set agent_phase in detect pass");
                        }
                        match insert_agent_event(
                            pool,
                            execution_id,
                            "agent_dispatch",
                            json!({ "agent": &agent }),
                        )
                        .await
                        {
                            Ok(ev) => {
                                info!(execution_id = %execution_id, event_id = ev.id, agent = %agent, "Inserted agent_dispatch event");
                            }
                            Err(e) => {
                                warn!(execution_id = %execution_id, error = %e, "Failed to insert agent_dispatch event");
                            }
                        }
                        detected.push(agent);
                    }
                }
            }
        }
    }

    detected
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
    fn extract_agent_name_from_subagent_type() {
        let args = serde_json::json!({ "subagent_type": "explorer" });
        assert_eq!(
            extract_dispatched_agent_name(&Some(args), &None),
            Some("explorer".to_string())
        );
    }

    #[test]
    fn extract_agent_name_from_category() {
        let args = serde_json::json!({ "category": "quick" });
        assert_eq!(
            extract_dispatched_agent_name(&Some(args), &None),
            Some("quick".to_string())
        );
    }

    #[test]
    fn extract_agent_name_prefers_subagent_type() {
        let args = serde_json::json!({ "subagent_type": "explorer", "category": "quick" });
        assert_eq!(
            extract_dispatched_agent_name(&Some(args), &None),
            Some("explorer".to_string())
        );
    }

    #[test]
    fn extract_agent_name_from_opencode_state() {
        let state = serde_json::json!({
            "status": "completed",
            "input": { "subagent_type": "explore", "prompt": "..." },
            "output": "result text"
        });
        assert_eq!(
            extract_dispatched_agent_name(&None, &Some(state)),
            Some("explore".to_string())
        );
    }

    #[test]
    fn extract_agent_name_args_takes_precedence_over_state() {
        let args = serde_json::json!({ "subagent_type": "explorer" });
        let state = serde_json::json!({
            "input": { "subagent_type": "explore" }
        });
        assert_eq!(
            extract_dispatched_agent_name(&Some(args), &Some(state)),
            Some("explorer".to_string())
        );
    }

    #[test]
    fn extract_agent_name_returns_none_for_missing_fields() {
        let args = serde_json::json!({ "other_field": "value" });
        assert_eq!(extract_dispatched_agent_name(&Some(args), &None), None);
    }

    #[test]
    fn extract_agent_name_returns_none_for_none_args() {
        assert_eq!(extract_dispatched_agent_name(&None, &None), None);
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

    #[test]
    fn unknown_message_part_classifies_as_unknown_event() {
        use crate::opencode::MessagePart;

        let part = MessagePart::Unknown {
            raw_type: "image".to_string(),
            raw: serde_json::json!({
                "type": "image",
                "url": "https://example.com/diagram.png"
            }),
        };

        let (event_type, content) = match &part {
            MessagePart::Text { text } => ("reasoning", json!({ "agent": "test", "text": text })),
            MessagePart::Reasoning { text } => {
                ("reasoning", json!({ "agent": "test", "reasoning": text }))
            }
            MessagePart::ToolInvocation { .. } => ("tool_call", json!({})),
            MessagePart::StepStart { id } => {
                ("reasoning", json!({ "agent": "test", "step_start": id }))
            }
            MessagePart::StepFinish { reason } => (
                "reasoning",
                json!({ "agent": "test", "step_finish": reason }),
            ),
            MessagePart::Unknown { raw_type, raw } => {
                ("unknown", json!({ "raw_type": raw_type, "raw": raw }))
            }
        };

        assert_eq!(event_type, "unknown");
        assert_eq!(
            content.get("raw_type").and_then(|v| v.as_str()),
            Some("image")
        );
        assert_eq!(
            content
                .get("raw")
                .and_then(|r| r.get("url"))
                .and_then(|v| v.as_str()),
            Some("https://example.com/diagram.png")
        );
    }

    #[test]
    fn unknown_message_part_is_not_silently_dropped() {
        use crate::opencode::MessagePart;

        let unknown_part = MessagePart::Unknown {
            raw_type: "future-type".to_string(),
            raw: serde_json::json!({ "type": "future-type", "data": 42 }),
        };

        let all_parts: Vec<MessagePart> = vec![
            MessagePart::Text {
                text: "hello".to_string(),
            },
            unknown_part.clone(),
            MessagePart::Reasoning {
                text: "thinking".to_string(),
            },
        ];

        let classified: Vec<&str> = all_parts
            .iter()
            .map(|p| match p {
                MessagePart::Text { .. } => "reasoning",
                MessagePart::Reasoning { .. } => "reasoning",
                MessagePart::ToolInvocation { .. } => "tool_call",
                MessagePart::StepStart { .. } => "reasoning",
                MessagePart::StepFinish { .. } => "reasoning",
                MessagePart::Unknown { .. } => "unknown",
            })
            .collect();

        assert_eq!(classified, vec!["reasoning", "unknown", "reasoning"]);
    }
}
