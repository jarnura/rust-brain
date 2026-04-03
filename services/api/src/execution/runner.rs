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

use serde_json::json;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::docker::DockerClient;
use crate::opencode::OpenCodeClient;
use super::models::{
    complete_execution, fail_execution, insert_agent_event, set_agent_phase,
    set_container_id, set_session_id,
};

// =============================================================================
// Phase definitions
// =============================================================================

/// Agent phases executed in order.
const PHASES: &[(&str, &str)] = &[
    ("orchestrating", ORCHESTRATOR_PROMPT),
    ("researching",   RESEARCH_PROMPT),
    ("planning",      PLANNING_PROMPT),
    ("developing",    DEVELOPMENT_PROMPT),
];

const ORCHESTRATOR_PROMPT: &str =
    "You are the orchestrator agent for a Rust codebase assistant. \
     Analyse the workspace and the user's request. \
     Identify which files are relevant, which architectural concerns apply, \
     and produce a concise task breakdown for the specialist agents.";

const RESEARCH_PROMPT: &str =
    "You are the research agent. Based on the orchestrator's analysis, \
     explore the relevant Rust source files in /workspace. \
     Summarise function signatures, module structure, and any existing tests \
     that touch the area described in the task.";

const PLANNING_PROMPT: &str =
    "You are the planning agent. Given the orchestrator's task breakdown and \
     the research summary, produce a step-by-step implementation plan. \
     Be precise about which files to create or modify and what changes to make.";

const DEVELOPMENT_PROMPT: &str =
    "You are the development agent. Execute the implementation plan. \
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

    info!(execution_id = %exec_id, "Spawning OpenCode container");

    // 1. Spawn container
    let (container_id, base_url) = match docker
        .spawn_execution_container(
            &exec_id.to_string(),
            &params.volume_name,
            &params.docker_network,
            &params.opencode_image,
        )
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            warn!(execution_id = %exec_id, error = %e, "Container spawn failed");
            let _ = fail_execution(&pool, exec_id, &format!("Container spawn failed: {e}")).await;
            return;
        }
    };

    // Record container_id immediately so the sweeper can kill it on timeout
    if let Err(e) = set_container_id(&pool, exec_id, &container_id).await {
        warn!(execution_id = %exec_id, error = %e, "Failed to persist container_id");
    }

    info!(execution_id = %exec_id, container_id = %container_id, base_url = %base_url, "Container started");

    // 2. Wait for OpenCode to be ready (up to 30 s)
    let opencode = OpenCodeClient::new(base_url, params.opencode_user, params.opencode_pass);
    if !wait_for_container(&opencode, 30, Duration::from_secs(1)).await {
        warn!(execution_id = %exec_id, "OpenCode container did not become ready");
        let _ = fail_execution(&pool, exec_id, "OpenCode container did not become ready within 30 s").await;
        cleanup_container(&docker, &container_id, exec_id).await;
        return;
    }

    // 3. Create an OpenCode session
    let session_id = match opencode.create_session(Some("rustbrain-execution")).await {
        Ok(s) => {
            if let Err(e) = set_session_id(&pool, exec_id, &s.id).await {
                warn!(execution_id = %exec_id, error = %e, "Failed to persist session_id");
            }
            s.id
        }
        Err(e) => {
            warn!(execution_id = %exec_id, error = %e, "Failed to create OpenCode session");
            let _ = fail_execution(&pool, exec_id, &format!("OpenCode session creation failed: {e}")).await;
            cleanup_container(&docker, &container_id, exec_id).await;
            return;
        }
    };

    // 4. Drive each agent phase
    let full_prompt = &params.prompt;
    for (phase, phase_system_prompt) in PHASES {
        // Advance phase in DB and emit phase_change event
        if let Err(e) = set_agent_phase(&pool, exec_id, phase).await {
            warn!(execution_id = %exec_id, phase = phase, error = %e, "Failed to set agent_phase");
        }

        let _ = insert_agent_event(
            &pool,
            exec_id,
            "phase_change",
            json!({ "phase": phase }),
        )
        .await;

        info!(execution_id = %exec_id, phase = phase, "Running agent phase");

        // Build the prompt: prepend the system context then append the user task
        let message = format!(
            "{}\n\n---\nUser task: {}",
            phase_system_prompt, full_prompt
        );

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
                let _ = fail_execution(&pool, exec_id, &format!("Phase '{phase}' failed: {e}")).await;
                cleanup_container(&docker, &container_id, exec_id).await;
                return;
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

    // 7. Clean up container
    cleanup_container(&docker, &container_id, exec_id).await;
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
            MessagePart::Text { text } => (
                "reasoning",
                json!({ "phase": phase, "text": text }),
            ),
            MessagePart::Reasoning { text } => (
                "reasoning",
                json!({ "phase": phase, "reasoning": text }),
            ),
            MessagePart::ToolInvocation { tool_name, args, result } => (
                "tool_call",
                json!({
                    "phase": phase,
                    "tool": tool_name,
                    "args": args,
                    "result": result,
                }),
            ),
            MessagePart::StepStart { id } => (
                "reasoning",
                json!({ "phase": phase, "step_start": id }),
            ),
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
        assert_eq!(names, vec!["orchestrating", "researching", "planning", "developing"]);
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
        };
        assert_eq!(p.volume_name, "rustbrain-ws-abc12345");
        assert_eq!(p.docker_network, "rustbrain");
    }
}
