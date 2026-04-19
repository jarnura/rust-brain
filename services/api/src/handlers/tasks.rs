//! Task lifecycle handlers.
//!
//! Provides endpoints for managing orchestrator tasks:
//! - `POST /api/tasks` — create
//! - `GET /api/tasks/:id` — get by ID
//! - `GET /api/tasks?status=X&agent=Y&phase=Z&class=W` — list with filters
//! - `PUT /api/tasks/:id` — update with state transition validation

use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::errors::AppError;
use crate::extractors::OptionalWorkspaceId;
use crate::state::AppState;
use crate::workspace::acquire_conn;

// =============================================================================
// Types
// =============================================================================

/// Full task record.
#[derive(Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub parent_id: Option<String>,
    pub phase: String,
    pub class: String,
    pub agent: String,
    pub status: String,
    pub inputs: serde_json::Value,
    pub constraints: serde_json::Value,
    pub acceptance: Option<String>,
    pub retry_count: i32,
    pub error: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Request body for creating a task.
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub id: String,
    pub parent_id: Option<String>,
    pub phase: String,
    pub class: String,
    pub agent: String,
    #[serde(default = "default_pending")]
    pub status: String,
    #[serde(default = "default_inputs")]
    pub inputs: serde_json::Value,
    #[serde(default = "default_constraints")]
    pub constraints: serde_json::Value,
    pub acceptance: Option<String>,
}

fn default_pending() -> String {
    "pending".to_string()
}
fn default_inputs() -> serde_json::Value {
    serde_json::json!([])
}
fn default_constraints() -> serde_json::Value {
    serde_json::json!({})
}

/// Request body for updating a task.
#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    pub status: Option<String>,
    pub retry_count: Option<i32>,
    pub error: Option<String>,
}

/// Query params for listing tasks.
#[derive(Debug, Deserialize)]
pub struct ListTasksQuery {
    pub status: Option<String>,
    pub agent: Option<String>,
    pub phase: Option<String>,
    pub class: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

// =============================================================================
// State transition validation
// =============================================================================

/// Validate state transitions. Any state can transition to "escalated" as an
/// escape hatch. On invalid transition, returns the list of allowed targets.
fn validate_transition(current: &str, next: &str) -> Result<(), AppError> {
    // Any state can transition to escalated (escape hatch)
    if next == "escalated" {
        return Ok(());
    }

    let allowed: &[&str] = match current {
        "pending" => &["dispatched"],
        "dispatched" => &["in_progress", "blocked"],
        "in_progress" => &["review", "blocked"],
        "review" => &["completed", "rejected"],
        "rejected" => &["dispatched"],
        "blocked" => &["dispatched"],
        _ => &[],
    };

    if allowed.contains(&next) {
        Ok(())
    } else {
        let mut all_allowed = allowed.to_vec();
        all_allowed.push("escalated");
        Err(AppError::BadRequest(format!(
            "Invalid state transition: '{}' \u{2192} '{}'. Allowed transitions from '{}': {:?}",
            current, next, current, all_allowed
        )))
    }
}

// =============================================================================
// Row mapping
// =============================================================================

type TaskRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    Option<String>,
    i32,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

fn row_to_task(row: TaskRow) -> Task {
    Task {
        id: row.0,
        parent_id: row.1,
        phase: row.2,
        class: row.3,
        agent: row.4,
        status: row.5,
        inputs: row.6,
        constraints: row.7,
        acceptance: row.8,
        retry_count: row.9,
        error: row.10,
        created_at: row.11.map(|t| t.to_rfc3339()),
        updated_at: row.12.map(|t| t.to_rfc3339()),
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// Create a new task.
pub async fn create_task(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    debug!("Creating task: {}", request.id);

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        INSERT INTO tasks (id, parent_id, phase, class, agent, status, inputs, constraints, acceptance)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, parent_id, phase, class, agent, status, inputs, constraints, acceptance, retry_count, error, created_at, updated_at
        "#,
    )
    .bind(&request.id)
    .bind(&request.parent_id)
    .bind(&request.phase)
    .bind(&request.class)
    .bind(&request.agent)
    .bind(&request.status)
    .bind(&request.inputs)
    .bind(&request.constraints)
    .bind(&request.acceptance)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(ref db_err) = e {
            if db_err.code().as_deref() == Some("23505") {
                return AppError::Conflict(format!(
                    "Task with ID '{}' already exists",
                    request.id
                ));
            }
        }
        AppError::Database(format!("Failed to create task: {}", e))
    })?;

    Ok(Json(row_to_task(row)))
}

/// Get a task by ID.
pub async fn get_task(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Path(id): Path<String>,
) -> Result<Json<Task>, AppError> {
    debug!("Getting task: {}", id);

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let row = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT id, parent_id, phase, class, agent, status, inputs, constraints, acceptance, retry_count, error, created_at, updated_at
        FROM tasks WHERE id = $1
        "#,
    )
    .bind(&id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to get task: {}", e)))?
    .ok_or_else(|| AppError::NotFound(format!("Task not found: {}", id)))?;

    Ok(Json(row_to_task(row)))
}

/// List tasks with optional filters (dynamic WHERE clause).
pub async fn list_tasks(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Query(query): Query<ListTasksQuery>,
) -> Result<Json<Vec<Task>>, AppError> {
    debug!(
        "Listing tasks: status={:?}, agent={:?}, phase={:?}, class={:?}",
        query.status, query.agent, query.phase, query.class
    );

    let limit = query.limit.min(100);
    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    let rows = sqlx::query_as::<_, TaskRow>(
        r#"
        SELECT id, parent_id, phase, class, agent, status, inputs, constraints, acceptance, retry_count, error, created_at, updated_at
        FROM tasks
        WHERE ($1::TEXT IS NULL OR status = $1)
          AND ($2::TEXT IS NULL OR agent = $2)
          AND ($3::TEXT IS NULL OR phase = $3)
          AND ($4::TEXT IS NULL OR class = $4)
        ORDER BY updated_at DESC
        LIMIT $5
        "#,
    )
    .bind(&query.status)
    .bind(&query.agent)
    .bind(&query.phase)
    .bind(&query.class)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list tasks: {}", e)))?;

    let tasks: Vec<Task> = rows.into_iter().map(row_to_task).collect();
    Ok(Json(tasks))
}

/// Update a task's status with state transition validation.
///
/// On status change to "rejected", auto-increments `retry_count`.
/// Auto-sets `updated_at = NOW()` on every update.
pub async fn update_task(
    State(state): State<AppState>,
    OptionalWorkspaceId(ws): OptionalWorkspaceId,
    Path(id): Path<String>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<Task>, AppError> {
    debug!("Updating task: {}", id);

    let mut conn = acquire_conn(&state.pg_pool, ws.as_ref()).await?;

    if let Some(ref new_status) = request.status {
        // Fetch current status for transition validation
        let current_row = sqlx::query_as::<_, (String,)>("SELECT status FROM tasks WHERE id = $1")
            .bind(&id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| AppError::Database(format!("Failed to get task: {}", e)))?
            .ok_or_else(|| AppError::NotFound(format!("Task not found: {}", id)))?;

        let current_status = &current_row.0;
        validate_transition(current_status, new_status)?;

        // Auto-increment retry_count when transitioning TO "rejected"
        let retry_increment = if new_status == "rejected" { 1 } else { 0 };

        let row = sqlx::query_as::<_, TaskRow>(
            r#"
            UPDATE tasks
            SET status = $2,
                retry_count = retry_count + $3,
                error = COALESCE($4, error),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, parent_id, phase, class, agent, status, inputs, constraints, acceptance, retry_count, error, created_at, updated_at
            "#,
        )
        .bind(&id)
        .bind(new_status)
        .bind(retry_increment)
        .bind(&request.error)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update task: {}", e)))?;

        Ok(Json(row_to_task(row)))
    } else {
        // No status change — update error/retry_count only
        let row = sqlx::query_as::<_, TaskRow>(
            r#"
            UPDATE tasks
            SET retry_count = COALESCE($2, retry_count),
                error = COALESCE($3, error),
                updated_at = NOW()
            WHERE id = $1
            RETURNING id, parent_id, phase, class, agent, status, inputs, constraints, acceptance, retry_count, error, created_at, updated_at
            "#,
        )
        .bind(&id)
        .bind(request.retry_count)
        .bind(&request.error)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update task: {}", e)))?
        .ok_or_else(|| AppError::NotFound(format!("Task not found: {}", id)))?;

        Ok(Json(row_to_task(row)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_task_request_deserialization() {
        let json = r#"{
            "id": "task-001",
            "phase": "build",
            "class": "B",
            "agent": "developer"
        }"#;
        let req: CreateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.id, "task-001");
        assert_eq!(req.phase, "build");
        assert_eq!(req.class, "B");
        assert_eq!(req.agent, "developer");
        assert_eq!(req.status, "pending");
        assert_eq!(req.inputs, serde_json::json!([]));
        assert_eq!(req.constraints, serde_json::json!({}));
    }

    #[test]
    fn test_valid_state_transitions() {
        // All valid explicit transitions
        assert!(validate_transition("pending", "dispatched").is_ok());
        assert!(validate_transition("dispatched", "in_progress").is_ok());
        assert!(validate_transition("dispatched", "blocked").is_ok());
        assert!(validate_transition("in_progress", "review").is_ok());
        assert!(validate_transition("in_progress", "blocked").is_ok());
        assert!(validate_transition("review", "completed").is_ok());
        assert!(validate_transition("review", "rejected").is_ok());
        assert!(validate_transition("rejected", "dispatched").is_ok());
        assert!(validate_transition("blocked", "dispatched").is_ok());

        // Any → escalated escape hatch
        assert!(validate_transition("pending", "escalated").is_ok());
        assert!(validate_transition("dispatched", "escalated").is_ok());
        assert!(validate_transition("in_progress", "escalated").is_ok());
        assert!(validate_transition("review", "escalated").is_ok());
        assert!(validate_transition("blocked", "escalated").is_ok());
        assert!(validate_transition("completed", "escalated").is_ok());
        assert!(validate_transition("rejected", "escalated").is_ok());
    }

    #[test]
    fn test_invalid_state_transitions() {
        assert!(validate_transition("pending", "completed").is_err());
        assert!(validate_transition("pending", "in_progress").is_err());
        assert!(validate_transition("dispatched", "completed").is_err());
        assert!(validate_transition("dispatched", "rejected").is_err());
        assert!(validate_transition("review", "pending").is_err());
        assert!(validate_transition("completed", "pending").is_err());
        assert!(validate_transition("completed", "dispatched").is_err());
    }

    #[test]
    fn test_invalid_transition_error_message() {
        let err = validate_transition("pending", "completed").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("pending"));
        assert!(msg.contains("completed"));
        assert!(msg.contains("dispatched"));
        assert!(msg.contains("escalated"));
    }

    #[test]
    fn test_rejected_auto_increments_retry() {
        // Verify that transitioning TO "rejected" is valid (the handler
        // auto-increments retry_count, but that requires a DB so we just
        // verify the transition is allowed here)
        assert!(validate_transition("review", "rejected").is_ok());
    }

    #[test]
    fn test_update_task_request_deserialization() {
        let json = r#"{"status": "in_progress"}"#;
        let req: UpdateTaskRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.status, Some("in_progress".to_string()));
        assert_eq!(req.retry_count, None);
        assert_eq!(req.error, None);
    }

    #[test]
    fn test_list_tasks_query_defaults() {
        let json = r#"{}"#;
        let query: ListTasksQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 20);
        assert!(query.status.is_none());
        assert!(query.agent.is_none());
        assert!(query.phase.is_none());
        assert!(query.class.is_none());
    }

    #[test]
    fn test_task_serialization() {
        let task = Task {
            id: "task-001".to_string(),
            parent_id: None,
            phase: "build".to_string(),
            class: "B".to_string(),
            agent: "developer".to_string(),
            status: "pending".to_string(),
            inputs: serde_json::json!([]),
            constraints: serde_json::json!({}),
            acceptance: None,
            retry_count: 0,
            error: None,
            created_at: None,
            updated_at: None,
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["id"], "task-001");
        assert_eq!(json["status"], "pending");
    }
}
