//! Execution and AgentEvent Postgres models + CRUD operations.
//!
//! An [`Execution`] represents one run of the OpenCode multi-agent flow
//! against a workspace. Each execution spawns an ephemeral OpenCode container,
//! drives four agent phases, and bridges events into [`AgentEvent`] rows.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

// =============================================================================
// Structs
// =============================================================================

/// A single execution of the OpenCode multi-agent flow for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Execution {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub prompt: String,
    pub branch_name: Option<String>,
    /// OpenCode session ID used for this execution.
    pub session_id: Option<String>,
    /// Docker container ID running OpenCode for this execution.
    pub container_id: Option<String>,
    /// Docker volume name containing the workspace source (e.g. `rustbrain-ws-abc12345`).
    pub volume_name: Option<String>,
    /// OpenCode container endpoint URL (e.g. `http://rustbrain-exec-<id>:4096`).
    pub opencode_endpoint: Option<String>,
    /// Session working directory inside the OpenCode container.
    pub workspace_path: Option<String>,
    /// `running` | `completed` | `failed` | `aborted` | `timeout`
    pub status: String,
    /// Current agent phase: `orchestrating` | `researching` | `planning` | `developing`
    pub agent_phase: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub diff_summary: Option<serde_json::Value>,
    pub error: Option<String>,
    pub timeout_config_secs: i32,
    /// When set, the execution container should be kept alive until this timestamp.
    /// After expiry the sweeper removes the container and clears this field.
    pub container_expires_at: Option<DateTime<Utc>>,
}

/// A single event emitted by an agent during an execution.
///
/// Stored in Postgres for SSE delivery to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AgentEvent {
    pub id: i64,
    pub execution_id: Uuid,
    pub timestamp: DateTime<Utc>,
    /// `reasoning` | `tool_call` | `file_edit` | `error` | `phase_change` | `agent_dispatch` | `container_kept_alive` | `unknown`
    pub event_type: String,
    pub content: serde_json::Value,
    /// Per-execution monotonic sequence number, starting at 1.
    /// Used as cursor for SSE backfill (FR-3, FR-17, FR-18).
    pub seq: i64,
}

/// Parameters for creating a new execution.
pub struct CreateExecutionParams {
    pub workspace_id: Uuid,
    pub prompt: String,
    pub branch_name: Option<String>,
    pub timeout_config_secs: Option<i32>,
}

// =============================================================================
// Execution CRUD
// =============================================================================

/// Insert a new execution row in `running` status and return it.
pub async fn create_execution(
    pool: &PgPool,
    params: CreateExecutionParams,
) -> anyhow::Result<Execution> {
    let timeout = params.timeout_config_secs.unwrap_or(7200);
    let row = sqlx::query_as::<_, Execution>(
        r#"
        INSERT INTO executions (workspace_id, prompt, branch_name, timeout_config_secs)
        VALUES ($1, $2, $3, $4)
        RETURNING id, workspace_id, prompt, branch_name, session_id, container_id,
                  volume_name, opencode_endpoint, workspace_path,
                  status, agent_phase, started_at, completed_at, diff_summary, error,
                  timeout_config_secs, container_expires_at
        "#,
    )
    .bind(params.workspace_id)
    .bind(params.prompt)
    .bind(params.branch_name)
    .bind(timeout)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Fetch a single execution by ID.
pub async fn get_execution(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<Execution>> {
    let row = sqlx::query_as::<_, Execution>(
        r#"
        SELECT id, workspace_id, prompt, branch_name, session_id, container_id,
               volume_name, opencode_endpoint, workspace_path,
               status, agent_phase, started_at, completed_at, diff_summary, error,
               timeout_config_secs, container_expires_at
        FROM executions
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// List all executions for a workspace, newest first.
pub async fn list_executions(pool: &PgPool, workspace_id: Uuid) -> anyhow::Result<Vec<Execution>> {
    let rows = sqlx::query_as::<_, Execution>(
        r#"
        SELECT id, workspace_id, prompt, branch_name, session_id, container_id,
               volume_name, opencode_endpoint, workspace_path,
               status, agent_phase, started_at, completed_at, diff_summary, error,
               timeout_config_secs, container_expires_at
        FROM executions
        WHERE workspace_id = $1
        ORDER BY started_at DESC
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Store the Docker container_id for a running execution.
pub async fn set_container_id(
    pool: &PgPool,
    execution_id: Uuid,
    container_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE executions SET container_id = $2 WHERE id = $1")
        .bind(execution_id)
        .bind(container_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store the OpenCode session_id for a running execution.
pub async fn set_session_id(
    pool: &PgPool,
    execution_id: Uuid,
    session_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE executions SET session_id = $2 WHERE id = $1")
        .bind(execution_id)
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store runtime info (volume, endpoint, workspace path) for a running execution.
pub async fn set_runtime_info(
    pool: &PgPool,
    execution_id: Uuid,
    volume_name: &str,
    opencode_endpoint: &str,
    workspace_path: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET volume_name = $2,
            opencode_endpoint = $3,
            workspace_path = $4
        WHERE id = $1
        "#,
    )
    .bind(execution_id)
    .bind(volume_name)
    .bind(opencode_endpoint)
    .bind(workspace_path)
    .execute(pool)
    .await?;
    Ok(())
}

/// Advance the execution to a new agent phase.
pub async fn set_agent_phase(pool: &PgPool, execution_id: Uuid, phase: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE executions SET agent_phase = $2 WHERE id = $1")
        .bind(execution_id)
        .bind(phase)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark an execution as `completed`, recording the diff summary.
pub async fn complete_execution(
    pool: &PgPool,
    execution_id: Uuid,
    diff_summary: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET status = 'completed',
            completed_at = NOW(),
            diff_summary = $2
        WHERE id = $1
        "#,
    )
    .bind(execution_id)
    .bind(diff_summary)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an execution as `failed` with an error message.
pub async fn fail_execution(pool: &PgPool, execution_id: Uuid, error: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET status = 'failed',
            completed_at = NOW(),
            error = $2
        WHERE id = $1
        "#,
    )
    .bind(execution_id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark an execution as `timeout`.
pub async fn timeout_execution(pool: &PgPool, execution_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET status = 'timeout',
            completed_at = NOW()
        WHERE id = $1 AND status = 'running'
        "#,
    )
    .bind(execution_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// List all running executions that have exceeded their timeout.
///
/// Returns `(execution_id, container_id)` pairs for the sweeper to kill.
pub async fn list_timed_out_executions(
    pool: &PgPool,
) -> anyhow::Result<Vec<(Uuid, Option<String>)>> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, container_id
        FROM executions
        WHERE status = 'running'
          AND started_at < NOW() - (timeout_config_secs || ' seconds')::INTERVAL
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Set the keep-alive expiry timestamp for an execution container.
///
/// Called by the runner when `keep_alive_secs > 0` on the success path.
pub async fn set_container_expires_at(
    pool: &PgPool,
    execution_id: Uuid,
    expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET container_expires_at = $2
        WHERE id = $1
        "#,
    )
    .bind(execution_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// List executions whose keep-alive has expired but whose containers are still running.
///
/// Returns `(execution_id, container_id)` pairs for the sweeper to clean up.
pub async fn list_expired_keepalive_executions(
    pool: &PgPool,
) -> anyhow::Result<Vec<(Uuid, Option<String>)>> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, container_id
        FROM executions
        WHERE container_expires_at IS NOT NULL
          AND container_expires_at <= NOW()
          AND container_id IS NOT NULL
          AND status IN ('completed', 'running')
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Clear the container_id and container_expires_at fields for an execution.
///
/// Called by the sweeper after removing an expired keep-alive container.
pub async fn clear_container_id(pool: &PgPool, execution_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        UPDATE executions
        SET container_id = NULL,
            container_expires_at = NULL
        WHERE id = $1
        "#,
    )
    .bind(execution_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// List all running executions for a workspace with their container IDs.
///
/// Used by workspace teardown to stop active containers before cleanup.
pub async fn list_running_executions_for_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> anyhow::Result<Vec<(Uuid, Option<String>)>> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, container_id
        FROM executions
        WHERE workspace_id = $1 AND status = 'running'
        "#,
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Abort all running executions for a workspace.
///
/// Sets status to 'aborted' and records a cleanup message.
pub async fn abort_executions_for_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> anyhow::Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE executions
        SET status = 'aborted',
            completed_at = NOW(),
            error = 'Workspace archived'
        WHERE workspace_id = $1 AND status = 'running'
        "#,
    )
    .bind(workspace_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

// =============================================================================
// AgentEvent CRUD
// =============================================================================

/// Insert a single agent event with monotonic seq assignment and content dedup.
///
/// Seq is computed as `COALESCE(MAX(seq), 0) + 1` within the same execution,
/// guaranteeing monotonically increasing per-execution sequence numbers.
/// The `UNIQUE (execution_id, seq)` constraint provides a safety net against
/// races in concurrent-write scenarios.
///
/// Content dedup: SHA-256 of `(execution_id, event_type, content)` is computed
/// and stored in `content_hash`. A `UNIQUE (execution_id, content_hash)`
/// constraint prevents duplicate events from runner retries. On conflict the
/// existing row is returned via a fallback SELECT.
pub async fn insert_agent_event(
    pool: &PgPool,
    execution_id: Uuid,
    event_type: &str,
    content: serde_json::Value,
) -> anyhow::Result<AgentEvent> {
    let content_hash = compute_content_hash(execution_id, event_type, &content);

    let row = sqlx::query_as::<_, AgentEvent>(
        r#"
        INSERT INTO agent_events (execution_id, event_type, content, seq, content_hash)
        VALUES ($1, $2, $3,
            (SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_events WHERE execution_id = $1),
            $4)
        ON CONFLICT (execution_id, content_hash) DO NOTHING
        RETURNING id, execution_id, timestamp, event_type, content, seq
        "#,
    )
    .bind(execution_id)
    .bind(event_type)
    .bind(content)
    .bind(content_hash.as_slice())
    .fetch_optional(pool)
    .await?;

    if let Some(event) = row {
        return Ok(event);
    }

    // ON CONFLICT — duplicate detected, fetch existing row
    let existing = sqlx::query_as::<_, AgentEvent>(
        r#"
        SELECT id, execution_id, timestamp, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1 AND content_hash = $2
        "#,
    )
    .bind(execution_id)
    .bind(content_hash.as_slice())
    .fetch_one(pool)
    .await?;

    Ok(existing)
}

/// List all agent events for an execution, ordered by seq (chronological).
pub async fn list_agent_events(
    pool: &PgPool,
    execution_id: Uuid,
) -> anyhow::Result<Vec<AgentEvent>> {
    let rows = sqlx::query_as::<_, AgentEvent>(
        r#"
        SELECT id, execution_id, timestamp, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1
        ORDER BY seq ASC
        "#,
    )
    .bind(execution_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fetch agent events after a given event ID (for incremental SSE polling).
pub async fn list_agent_events_after(
    pool: &PgPool,
    execution_id: Uuid,
    after_id: i64,
) -> anyhow::Result<Vec<AgentEvent>> {
    let rows = sqlx::query_as::<_, AgentEvent>(
        r#"
        SELECT id, execution_id, timestamp, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1 AND id > $2
        ORDER BY seq ASC
        "#,
    )
    .bind(execution_id)
    .bind(after_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Fetch agent events after a given seq number (cursor-based, for SSE backfill).
///
/// Returns all events for the given execution with `seq > after_seq`, ordered
/// by seq ascending. This is the primary cursor-based read path (FR-18).
pub async fn list_agent_events_after_seq(
    pool: &PgPool,
    execution_id: Uuid,
    after_seq: i64,
) -> anyhow::Result<Vec<AgentEvent>> {
    let rows = sqlx::query_as::<_, AgentEvent>(
        r#"
        SELECT id, execution_id, timestamp, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1 AND seq > $2
        ORDER BY seq ASC
        "#,
    )
    .bind(execution_id)
    .bind(after_seq)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn compute_content_hash(
    execution_id: Uuid,
    event_type: &str,
    content: &serde_json::Value,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(execution_id.as_bytes());
    hasher.update(event_type.as_bytes());
    hasher.update(content.to_string().as_bytes());
    hasher.finalize().to_vec()
}

/// Fetch a single agent event by execution_id and seq number.
pub async fn get_agent_event_by_seq(
    pool: &PgPool,
    execution_id: Uuid,
    seq: i64,
) -> anyhow::Result<Option<AgentEvent>> {
    let row = sqlx::query_as::<_, AgentEvent>(
        r#"
        SELECT id, execution_id, timestamp, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1 AND seq = $2
        "#,
    )
    .bind(execution_id)
    .bind(seq)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_serializes() {
        let e = Execution {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            prompt: "fix the bug".into(),
            branch_name: Some("fix/branch".into()),
            session_id: Some("ses_abc".into()),
            container_id: Some("abc123".into()),
            volume_name: Some("rustbrain-ws-abc12345".into()),
            opencode_endpoint: Some("http://rustbrain-exec-abc:4096".into()),
            workspace_path: Some("/workspace".into()),
            status: "running".into(),
            agent_phase: Some("researching".into()),
            started_at: Utc::now(),
            completed_at: None,
            diff_summary: None,
            error: None,
            timeout_config_secs: 7200,
            container_expires_at: None,
        };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["status"], "running");
        assert_eq!(json["agent_phase"], "researching");
        assert_eq!(json["container_id"], "abc123");
        assert_eq!(json["volume_name"], "rustbrain-ws-abc12345");
        assert_eq!(json["opencode_endpoint"], "http://rustbrain-exec-abc:4096");
        assert_eq!(json["workspace_path"], "/workspace");
    }

    #[test]
    fn agent_event_serializes() {
        let ev = AgentEvent {
            id: 1,
            execution_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: "phase_change".into(),
            content: serde_json::json!({"phase": "researching"}),
            seq: 1,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["event_type"], "phase_change");
        assert_eq!(json["content"]["phase"], "researching");
        assert_eq!(json["seq"], 1);
    }

    #[test]
    fn create_execution_params_defaults() {
        let p = CreateExecutionParams {
            workspace_id: Uuid::new_v4(),
            prompt: "do something".into(),
            branch_name: None,
            timeout_config_secs: None,
        };
        // Default timeout should be 7200 when None passed to create_execution
        let timeout = p.timeout_config_secs.unwrap_or(7200);
        assert_eq!(timeout, 7200);
    }

    #[test]
    fn create_execution_params_with_branch() {
        let p = CreateExecutionParams {
            workspace_id: Uuid::new_v4(),
            prompt: "fix the bug".into(),
            branch_name: Some("fix/issue-123".into()),
            timeout_config_secs: Some(3600),
        };
        assert_eq!(p.branch_name.as_deref(), Some("fix/issue-123"));
        assert_eq!(p.timeout_config_secs, Some(3600));
    }

    #[test]
    fn agent_event_seq_in_json_output() {
        let ev = AgentEvent {
            id: 99,
            execution_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: "tool_call".into(),
            content: serde_json::json!({"tool": "read_file"}),
            seq: 42,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["seq"], 42);
        assert_eq!(json["id"], 99);
    }

    #[test]
    fn agent_event_seq_monotonic_ordering() {
        let exec_id = Uuid::new_v4();
        let events: Vec<AgentEvent> = (1..=5)
            .map(|seq| AgentEvent {
                id: seq * 10,
                execution_id: exec_id,
                timestamp: Utc::now(),
                event_type: "reasoning".into(),
                content: serde_json::json!({"seq": seq}),
                seq,
            })
            .collect();
        let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5]);
        for i in 1..events.len() {
            assert!(events[i].seq > events[i - 1].seq);
        }
    }

    #[test]
    fn content_hash_deterministic() {
        let id = Uuid::new_v4();
        let content = serde_json::json!({"text": "hello"});
        let h1 = compute_content_hash(id, "reasoning", &content);
        let h2 = compute_content_hash(id, "reasoning", &content);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 32);
    }

    #[test]
    fn content_hash_differs_on_event_type() {
        let id = Uuid::new_v4();
        let content = serde_json::json!({"text": "hello"});
        let h1 = compute_content_hash(id, "reasoning", &content);
        let h2 = compute_content_hash(id, "tool_call", &content);
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_differs_on_execution_id() {
        let content = serde_json::json!({"text": "hello"});
        let h1 = compute_content_hash(Uuid::new_v4(), "reasoning", &content);
        let h2 = compute_content_hash(Uuid::new_v4(), "reasoning", &content);
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_hash_differs_on_content() {
        let id = Uuid::new_v4();
        let h1 = compute_content_hash(id, "reasoning", &serde_json::json!({"text": "a"}));
        let h2 = compute_content_hash(id, "reasoning", &serde_json::json!({"text": "b"}));
        assert_ne!(h1, h2);
    }
}
