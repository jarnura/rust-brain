//! Integration tests for seq-based cursor event storage (RUSA-251 Phase 1D).
//!
//! These tests require a running Postgres instance (Docker Compose stack).
//! Marked with `#[ignore]` for CI — run with `cargo test -- --include-ignored`.
//!
//! Acceptance criteria from RUSA-251:
//! - seq is monotonically increasing per execution
//! - Cursor-based read returns exactly events with seq > cursor
//! - Append-only: no UPDATE/DELETE on stored events
//! - Storage failure → structured error event emitted (not silent)
//! - Integration test: write N events, read from seq M, get exactly N-M events in order

use sqlx::PgPool;
use uuid::Uuid;

async fn test_pool() -> PgPool {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://rustbrain:rustbrain@localhost:5432/rustbrain".into());
    PgPool::connect(&db_url)
        .await
        .expect("Failed to connect to Postgres — is Docker Compose running?")
}

/// Helper: create a minimal execution row for testing event inserts.
async fn create_test_execution(pool: &PgPool) -> Uuid {
    let workspace_id: Uuid = sqlx::query_scalar(
        "INSERT INTO workspaces (github_url, name, status) VALUES ($1, $2, 'ready') RETURNING id",
    )
    .bind("https://github.com/test/seq-test")
    .bind(format!("seq-test-{}", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .expect("Failed to create test workspace");

    let exec_id: Uuid = sqlx::query_scalar(
        "INSERT INTO executions (workspace_id, prompt, status) VALUES ($1, $2, 'running') RETURNING id",
    )
    .bind(workspace_id)
    .bind("test prompt for seq")
    .fetch_one(pool)
    .await
    .expect("Failed to create test execution");

    exec_id
}

/// Row shape matching agent_events columns needed by these tests.
#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
struct EventRow {
    id: i64,
    execution_id: Uuid,
    event_type: String,
    content: serde_json::Value,
    seq: i64,
}

/// Insert a single agent event using the same seq subquery as the app code.
async fn insert_event(
    pool: &PgPool,
    execution_id: Uuid,
    event_type: &str,
    content: serde_json::Value,
) -> EventRow {
    sqlx::query_as::<_, EventRow>(
        r#"
        INSERT INTO agent_events (execution_id, event_type, content, seq)
        VALUES ($1, $2, $3,
            (SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_events WHERE execution_id = $1))
        RETURNING id, execution_id, event_type, content, seq
        "#,
    )
    .bind(execution_id)
    .bind(event_type)
    .bind(content)
    .fetch_one(pool)
    .await
    .expect("insert failed")
}

/// List all events for an execution ordered by seq.
async fn list_all_events(pool: &PgPool, execution_id: Uuid) -> Vec<EventRow> {
    sqlx::query_as::<_, EventRow>(
        r#"
        SELECT id, execution_id, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1
        ORDER BY seq ASC
        "#,
    )
    .bind(execution_id)
    .fetch_all(pool)
    .await
    .expect("list failed")
}

/// List events after a given seq (cursor-based read).
async fn list_events_after_seq(pool: &PgPool, execution_id: Uuid, after_seq: i64) -> Vec<EventRow> {
    sqlx::query_as::<_, EventRow>(
        r#"
        SELECT id, execution_id, event_type, content, seq
        FROM agent_events
        WHERE execution_id = $1 AND seq > $2
        ORDER BY seq ASC
        "#,
    )
    .bind(execution_id)
    .bind(after_seq)
    .fetch_all(pool)
    .await
    .expect("cursor read failed")
}

#[tokio::test]
#[ignore]
async fn seq_monotonically_increasing_per_execution() {
    let pool = test_pool().await;
    let exec_id = create_test_execution(&pool).await;

    let n = 100;
    let mut seqs = Vec::with_capacity(n);
    for i in 0..n {
        let event = insert_event(
            &pool,
            exec_id,
            "reasoning",
            serde_json::json!({ "index": i }),
        )
        .await;
        seqs.push(event.seq);
    }

    for i in 1..seqs.len() {
        assert!(
            seqs[i] > seqs[i - 1],
            "seq not monotonic at index {}: {} <= {}",
            i,
            seqs[i],
            seqs[i - 1]
        );
    }
    assert_eq!(seqs[0], 1, "seq should start at 1");
}

#[tokio::test]
#[ignore]
async fn cursor_read_returns_exact_events_after_cursor() {
    let pool = test_pool().await;
    let exec_id = create_test_execution(&pool).await;

    let n = 1000;
    for i in 0..n {
        insert_event(
            &pool,
            exec_id,
            "reasoning",
            serde_json::json!({ "index": i }),
        )
        .await;
    }

    let after_seq = 500;
    let events = list_events_after_seq(&pool, exec_id, after_seq).await;

    assert_eq!(
        events.len(),
        500,
        "expected 500 events after seq 500, got {}",
        events.len()
    );

    for ev in &events {
        assert!(
            ev.seq > after_seq,
            "event seq {} should be > cursor {}",
            ev.seq,
            after_seq
        );
    }

    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    for i in 1..seqs.len() {
        assert!(
            seqs[i] > seqs[i - 1],
            "events not in seq order at index {}: {} <= {}",
            i,
            seqs[i],
            seqs[i - 1]
        );
    }
}

#[tokio::test]
#[ignore]
async fn no_gaps_in_seq_numbers() {
    let pool = test_pool().await;
    let exec_id = create_test_execution(&pool).await;

    let n = 100;
    for i in 0..n {
        insert_event(
            &pool,
            exec_id,
            "reasoning",
            serde_json::json!({ "index": i }),
        )
        .await;
    }

    let all_events = list_all_events(&pool, exec_id).await;

    let seqs: Vec<i64> = all_events.iter().map(|e| e.seq).collect();
    for (i, &seq) in seqs.iter().enumerate() {
        let expected = (i + 1) as i64;
        assert_eq!(
            seq, expected,
            "gap in seq at position {}: expected {}, got {}",
            i, expected, seq
        );
    }
}

#[tokio::test]
#[ignore]
async fn seq_is_per_execution_not_global() {
    let pool = test_pool().await;
    let exec_a = create_test_execution(&pool).await;
    let exec_b = create_test_execution(&pool).await;

    let event_a1 = insert_event(
        &pool,
        exec_a,
        "reasoning",
        serde_json::json!({ "stream": "a" }),
    )
    .await;

    let event_b1 = insert_event(
        &pool,
        exec_b,
        "reasoning",
        serde_json::json!({ "stream": "b" }),
    )
    .await;

    assert_eq!(event_a1.seq, 1, "execution A first event seq should be 1");
    assert_eq!(event_b1.seq, 1, "execution B first event seq should be 1");
}

#[tokio::test]
#[ignore]
async fn cursor_read_from_zero_returns_all_events() {
    let pool = test_pool().await;
    let exec_id = create_test_execution(&pool).await;

    for i in 0..5 {
        insert_event(&pool, exec_id, "reasoning", serde_json::json!({ "i": i })).await;
    }

    let events = list_events_after_seq(&pool, exec_id, 0).await;
    assert_eq!(events.len(), 5);
}

#[tokio::test]
#[ignore]
async fn cursor_read_past_last_returns_empty() {
    let pool = test_pool().await;
    let exec_id = create_test_execution(&pool).await;

    for i in 0..5 {
        insert_event(&pool, exec_id, "reasoning", serde_json::json!({ "i": i })).await;
    }

    let events = list_events_after_seq(&pool, exec_id, 9999).await;
    assert!(events.is_empty());
}
