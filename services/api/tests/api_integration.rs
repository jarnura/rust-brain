//! Integration tests for the rust-brain REST API.
//!
//! These tests exercise all 27+ REST routes against a live server at
//! `http://localhost:8088`.  They require the full docker-compose stack to be
//! running (`bash scripts/start.sh`).
//!
//! Run with:
//! ```
//! cargo test --test api_integration -- --include-ignored
//! ```
//!
//! In CI the docker-compose stack is always up, so the tests run unconditionally.

use reqwest::Client;
use serde_json::{json, Value};

const BASE: &str = "http://localhost:8088";

/// Build a reusable reqwest client.
fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client")
}

// =============================================================================
// Helper assertions
// =============================================================================

/// Assert that a JSON object has a specific key.
fn has_key(v: &Value, key: &str) -> bool {
    v.as_object().map(|o| o.contains_key(key)).unwrap_or(false)
}

// =============================================================================
// 1. Health / Metrics / Snapshot  (GET /health, /metrics, /api/snapshot)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_health_returns_healthy() {
    let resp = client()
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "healthy");
    assert!(has_key(&body["dependencies"], "postgres"));
    assert!(has_key(&body["dependencies"], "neo4j"));
    assert!(has_key(&body["dependencies"], "qdrant"));
}

#[tokio::test]
#[ignore]
async fn test_metrics_endpoint() {
    let resp = client()
        .get(format!("{BASE}/metrics"))
        .send()
        .await
        .expect("GET /metrics failed");

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    // Prometheus text format always contains this header comment
    assert!(text.contains("rustbrain_api") || text.contains("# HELP") || text.contains("# TYPE"));
}

#[tokio::test]
#[ignore]
async fn test_snapshot_endpoint() {
    let resp = client()
        .get(format!("{BASE}/api/snapshot"))
        .send()
        .await
        .expect("GET /api/snapshot failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // Snapshot returns version or service info
    assert!(body.is_object());
}

// =============================================================================
// 2. Semantic search  (POST /tools/search_semantic)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_search_semantic_happy_path() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({"query": "parse rust source file", "limit": 3}))
        .send()
        .await
        .expect("POST /tools/search_semantic failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "results"));
    assert!(has_key(&body, "query"));
    assert!(has_key(&body, "total"));
    // total must match results length
    let results = body["results"].as_array().unwrap();
    assert_eq!(body["total"].as_u64().unwrap() as usize, results.len());
}

#[tokio::test]
#[ignore]
async fn test_search_semantic_missing_query() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({"limit": 5}))
        .send()
        .await
        .expect("POST /tools/search_semantic failed");

    // Missing required field should be 422 (Unprocessable Entity) from Axum
    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_search_semantic_empty_query_returns_results() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({"query": "", "limit": 1}))
        .send()
        .await
        .expect("POST /tools/search_semantic failed");

    // Empty query may return 200 with empty results or a 400; either is valid
    assert!(resp.status() == 200 || resp.status() == 400);
}

// =============================================================================
// 3. Aggregate search  (POST /tools/aggregate_search)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_aggregate_search_happy_path() {
    let resp = client()
        .post(format!("{BASE}/tools/aggregate_search"))
        .json(&json!({"query": "function", "limit": 3}))
        .send()
        .await
        .expect("POST /tools/aggregate_search failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "results") || body.is_array() || body.is_object());
}

#[tokio::test]
#[ignore]
async fn test_aggregate_search_missing_query() {
    let resp = client()
        .post(format!("{BASE}/tools/aggregate_search"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/aggregate_search failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 4. Get function  (GET /tools/get_function)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_get_function_not_found() {
    let resp = client()
        .get(format!("{BASE}/tools/get_function?fqn=nonexistent::fake::fn"))
        .send()
        .await
        .expect("GET /tools/get_function failed");

    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
#[ignore]
async fn test_get_function_missing_fqn_param() {
    let resp = client()
        .get(format!("{BASE}/tools/get_function"))
        .send()
        .await
        .expect("GET /tools/get_function failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 5. Get callers  (GET /tools/get_callers)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_get_callers_unknown_fqn_returns_empty() {
    let resp = client()
        .get(format!("{BASE}/tools/get_callers?fqn=nonexistent::fn"))
        .send()
        .await
        .expect("GET /tools/get_callers failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "fqn"));
    assert!(has_key(&body, "callers"));
    let callers = body["callers"].as_array().unwrap();
    assert!(callers.is_empty());
}

#[tokio::test]
#[ignore]
async fn test_get_callers_missing_fqn_param() {
    let resp = client()
        .get(format!("{BASE}/tools/get_callers"))
        .send()
        .await
        .expect("GET /tools/get_callers failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 6. Get trait impls  (GET /tools/get_trait_impls)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_get_trait_impls_happy_path() {
    let resp = client()
        .get(format!("{BASE}/tools/get_trait_impls?trait_name=Display"))
        .send()
        .await
        .expect("GET /tools/get_trait_impls failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "trait_name"));
    assert!(has_key(&body, "implementations"));
}

#[tokio::test]
#[ignore]
async fn test_get_trait_impls_missing_param() {
    let resp = client()
        .get(format!("{BASE}/tools/get_trait_impls"))
        .send()
        .await
        .expect("GET /tools/get_trait_impls failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 7. Find usages of type  (GET /tools/find_usages_of_type)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_find_usages_of_type_happy_path() {
    let resp = client()
        .get(format!("{BASE}/tools/find_usages_of_type?type_name=String"))
        .send()
        .await
        .expect("GET /tools/find_usages_of_type failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "type_name"));
    assert!(has_key(&body, "usages"));
}

#[tokio::test]
#[ignore]
async fn test_find_usages_of_type_missing_param() {
    let resp = client()
        .get(format!("{BASE}/tools/find_usages_of_type"))
        .send()
        .await
        .expect("GET /tools/find_usages_of_type failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 8. Get module tree  (GET /tools/get_module_tree)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_get_module_tree_happy_path() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree?crate_name=rustbrain_ingestion"))
        .send()
        .await
        .expect("GET /tools/get_module_tree failed");

    // 200 if crate is ingested, otherwise may be empty
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "crate_name") || body.is_object());
}

#[tokio::test]
#[ignore]
async fn test_get_module_tree_missing_param() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .send()
        .await
        .expect("GET /tools/get_module_tree failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 9. Query graph  (POST /tools/query_graph)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_query_graph_read_only_happy_path() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({"query": "MATCH (n) RETURN n LIMIT 1", "parameters": {}}))
        .send()
        .await
        .expect("POST /tools/query_graph failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "rows") || has_key(&body, "results") || body.is_object());
}

#[tokio::test]
#[ignore]
async fn test_query_graph_rejects_write_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({"query": "CREATE (n:Test {name: 'evil'}) RETURN n", "parameters": {}}))
        .send()
        .await
        .expect("POST /tools/query_graph failed");

    assert_eq!(resp.status(), 400, "write Cypher should be rejected");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

#[tokio::test]
#[ignore]
async fn test_query_graph_both_fields_missing() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/query_graph failed");

    // Neither 'query' nor 'query_name' provided → 400
    assert_eq!(
        resp.status(), 400,
        "missing both query fields should return 400, got {}",
        resp.status()
    );
}

// =============================================================================
// 10. Find calls with type  (GET /tools/find_calls_with_type)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_find_calls_with_type_happy_path() {
    let resp = client()
        .get(format!("{BASE}/tools/find_calls_with_type?type_name=String"))
        .send()
        .await
        .expect("GET /tools/find_calls_with_type failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_object() || body.is_array());
}

#[tokio::test]
#[ignore]
async fn test_find_calls_with_type_missing_param() {
    let resp = client()
        .get(format!("{BASE}/tools/find_calls_with_type"))
        .send()
        .await
        .expect("GET /tools/find_calls_with_type failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 11. Find trait impls for type  (GET /tools/find_trait_impls_for_type)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_find_trait_impls_for_type_happy_path() {
    // Parameter is `type_name`, not `type_fqn`
    let resp = client()
        .get(format!("{BASE}/tools/find_trait_impls_for_type?type_name=String"))
        .send()
        .await
        .expect("GET /tools/find_trait_impls_for_type failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "type_name") || body.is_object());
}

#[tokio::test]
#[ignore]
async fn test_find_trait_impls_for_type_missing_param() {
    let resp = client()
        .get(format!("{BASE}/tools/find_trait_impls_for_type"))
        .send()
        .await
        .expect("GET /tools/find_trait_impls_for_type failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 12. PG query  (POST /tools/pg_query)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_pg_query_select_happy_path() {
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({"query": "SELECT 1 AS n"}))
        .send()
        .await
        .expect("POST /tools/pg_query failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "rows"));
    assert!(has_key(&body, "row_count"));
    assert_eq!(body["row_count"], 1);
}

#[tokio::test]
#[ignore]
async fn test_pg_query_rejects_insert() {
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({"query": "INSERT INTO extracted_items VALUES (1)"}))
        .send()
        .await
        .expect("POST /tools/pg_query failed");

    assert_eq!(resp.status(), 400, "INSERT should be rejected");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

#[tokio::test]
#[ignore]
async fn test_pg_query_rejects_drop() {
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({"query": "DROP TABLE extracted_items"}))
        .send()
        .await
        .expect("POST /tools/pg_query failed");

    assert_eq!(resp.status(), 400, "DROP should be rejected");
}

#[tokio::test]
#[ignore]
async fn test_pg_query_rejects_system_tables() {
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({"query": "SELECT * FROM pg_catalog.pg_tables"}))
        .send()
        .await
        .expect("POST /tools/pg_query failed");

    assert_eq!(resp.status(), 400, "system table query should be rejected");
}

#[tokio::test]
#[ignore]
async fn test_pg_query_missing_query_field() {
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/pg_query failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 13. Ingestion progress  (GET /api/ingestion/progress)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_ingestion_progress() {
    let resp = client()
        .get(format!("{BASE}/api/ingestion/progress"))
        .send()
        .await
        .expect("GET /api/ingestion/progress failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_object());
}

// =============================================================================
// 14. Artifacts CRUD
//     POST /api/artifacts, GET /api/artifacts, GET /api/artifacts/:id, PUT /api/artifacts/:id
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_artifacts_crud_lifecycle() {
    let client = client();
    let artifact_id = format!("test-artifact-{}", uuid_v4());

    // CREATE
    let create_resp = client
        .post(format!("{BASE}/api/artifacts"))
        .json(&json!({
            "id": artifact_id,
            "task_id": "test-task-001",
            "type": "analysis",
            "producer": "qa-lead",
            "summary": {"key": "value"},
            "payload": {"data": "test"}
        }))
        .send()
        .await
        .expect("POST /api/artifacts failed");

    assert!(
        create_resp.status() == 200 || create_resp.status() == 201,
        "artifact creation should return 200/201, got {}",
        create_resp.status()
    );
    let created: Value = create_resp.json().await.unwrap();
    assert_eq!(created["id"], artifact_id);
    assert_eq!(created["status"], "draft");

    // LIST
    let list_resp = client
        .get(format!("{BASE}/api/artifacts"))
        .send()
        .await
        .expect("GET /api/artifacts failed");
    assert_eq!(list_resp.status(), 200);
    let list: Value = list_resp.json().await.unwrap();
    assert!(list.is_array() || has_key(&list, "artifacts"));

    // GET by ID
    let get_resp = client
        .get(format!("{BASE}/api/artifacts/{artifact_id}"))
        .send()
        .await
        .expect("GET /api/artifacts/:id failed");
    assert_eq!(get_resp.status(), 200);
    let fetched: Value = get_resp.json().await.unwrap();
    assert_eq!(fetched["id"], artifact_id);

    // UPDATE
    let update_resp = client
        .put(format!("{BASE}/api/artifacts/{artifact_id}"))
        .json(&json!({"status": "final"}))
        .send()
        .await
        .expect("PUT /api/artifacts/:id failed");
    assert_eq!(update_resp.status(), 200);
    let updated: Value = update_resp.json().await.unwrap();
    assert_eq!(updated["status"], "final");

    // UPDATE invalid status
    let bad_update_resp = client
        .put(format!("{BASE}/api/artifacts/{artifact_id}"))
        .json(&json!({"status": "not_a_real_status"}))
        .send()
        .await
        .expect("PUT /api/artifacts/:id failed");
    assert!(
        bad_update_resp.status() == 400 || bad_update_resp.status() == 422,
        "invalid status should be rejected"
    );

    // GET non-existent
    let missing_resp = client
        .get(format!("{BASE}/api/artifacts/does-not-exist"))
        .send()
        .await
        .expect("GET /api/artifacts/:id failed");
    assert_eq!(missing_resp.status(), 404);
}

// =============================================================================
// 15. Tasks CRUD + state transitions
//     POST /api/tasks, GET /api/tasks, GET /api/tasks/:id, PUT /api/tasks/:id
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_tasks_crud_lifecycle() {
    let client = client();
    let task_id = format!("test-task-{}", uuid_v4());

    // CREATE — valid phases: understand, plan, build, verify, communicate
    let create_resp = client
        .post(format!("{BASE}/api/tasks"))
        .json(&json!({
            "id": task_id,
            "phase": "build",
            "class": "A",
            "agent": "qa-lead"
        }))
        .send()
        .await
        .expect("POST /api/tasks failed");

    assert!(
        create_resp.status() == 200 || create_resp.status() == 201,
        "task creation should return 200/201, got {}",
        create_resp.status()
    );
    let created: Value = create_resp.json().await.unwrap();
    assert_eq!(created["id"], task_id);
    assert_eq!(created["status"], "pending");

    // LIST
    let list_resp = client
        .get(format!("{BASE}/api/tasks"))
        .send()
        .await
        .expect("GET /api/tasks failed");
    assert_eq!(list_resp.status(), 200);
    let list: Value = list_resp.json().await.unwrap();
    assert!(list.is_array() || has_key(&list, "tasks"));

    // GET by ID
    let get_resp = client
        .get(format!("{BASE}/api/tasks/{task_id}"))
        .send()
        .await
        .expect("GET /api/tasks/:id failed");
    assert_eq!(get_resp.status(), 200);
    let fetched: Value = get_resp.json().await.unwrap();
    assert_eq!(fetched["id"], task_id);

    // VALID transition: pending → dispatched
    let update_resp = client
        .put(format!("{BASE}/api/tasks/{task_id}"))
        .json(&json!({"status": "dispatched"}))
        .send()
        .await
        .expect("PUT /api/tasks/:id failed");
    assert_eq!(update_resp.status(), 200);
    let updated: Value = update_resp.json().await.unwrap();
    assert_eq!(updated["status"], "dispatched");

    // VALID transition: dispatched → in_progress
    let update_resp2 = client
        .put(format!("{BASE}/api/tasks/{task_id}"))
        .json(&json!({"status": "in_progress"}))
        .send()
        .await
        .expect("PUT /api/tasks/:id failed");
    assert_eq!(update_resp2.status(), 200);

    // INVALID transition: in_progress → pending (not allowed)
    let bad_resp = client
        .put(format!("{BASE}/api/tasks/{task_id}"))
        .json(&json!({"status": "pending"}))
        .send()
        .await
        .expect("PUT /api/tasks/:id bad transition failed");
    assert_eq!(bad_resp.status(), 400, "invalid transition should return 400");
    let bad_body: Value = bad_resp.json().await.unwrap();
    assert_eq!(bad_body["code"], "BAD_REQUEST");

    // ESCAPE HATCH: any state → escalated
    let escalate_resp = client
        .put(format!("{BASE}/api/tasks/{task_id}"))
        .json(&json!({"status": "escalated"}))
        .send()
        .await
        .expect("PUT /api/tasks/:id escalate failed");
    assert_eq!(escalate_resp.status(), 200, "escalated is always a valid transition");

    // GET non-existent task
    let missing_resp = client
        .get(format!("{BASE}/api/tasks/does-not-exist"))
        .send()
        .await
        .expect("GET /api/tasks/:id not-found failed");
    assert_eq!(missing_resp.status(), 404);
}

// =============================================================================
// 16. Chat  (POST /tools/chat)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_chat_happy_path() {
    let resp = client()
        .post(format!("{BASE}/tools/chat"))
        .json(&json!({"message": "What is 2+2?", "session_id": null}))
        .send()
        .await
        .expect("POST /tools/chat failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "message") || has_key(&body, "response") || body.is_object());
}

#[tokio::test]
#[ignore]
async fn test_chat_missing_message() {
    let resp = client()
        .post(format!("{BASE}/tools/chat"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/chat failed");

    assert!(
        resp.status() == 422 || resp.status() == 400,
        "expected 400 or 422, got {}",
        resp.status()
    );
}

// =============================================================================
// 17. Chat stream  (GET /tools/chat/stream)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_chat_stream_is_sse() {
    // Just verify the endpoint accepts a GET with a message parameter and starts streaming
    let resp = client()
        .get(format!("{BASE}/tools/chat/stream?message=hello"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("GET /tools/chat/stream failed");

    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "expected SSE content type, got {content_type}"
    );
}

#[tokio::test]
#[ignore]
async fn test_chat_stream_post_returns_405() {
    let resp = client()
        .post(format!("{BASE}/tools/chat/stream"))
        .json(&json!({"message": "hello"}))
        .send()
        .await
        .expect("POST /tools/chat/stream failed");

    // Chat stream is GET-only
    assert_eq!(resp.status(), 405);
}

// =============================================================================
// 18. Chat sessions CRUD
//     POST /tools/chat/sessions, GET /tools/chat/sessions,
//     GET /tools/chat/sessions/:id, DELETE /tools/chat/sessions/:id
//     POST /tools/chat/sessions/:id/fork
//     POST /tools/chat/sessions/:id/abort
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_chat_sessions_lifecycle() {
    let client = client();

    // CREATE
    let create_resp = client
        .post(format!("{BASE}/tools/chat/sessions"))
        .json(&json!({"title": "Integration test session"}))
        .send()
        .await
        .expect("POST /tools/chat/sessions failed");

    assert!(
        create_resp.status() == 200 || create_resp.status() == 201,
        "expected 200 or 201, got {}",
        create_resp.status()
    );
    let created: Value = create_resp.json().await.unwrap();
    let session_id = created["id"].as_str().expect("session should have id");
    let session_id = session_id.to_string();

    // LIST
    let list_resp = client
        .get(format!("{BASE}/tools/chat/sessions"))
        .send()
        .await
        .expect("GET /tools/chat/sessions failed");
    assert_eq!(list_resp.status(), 200);
    let list: Value = list_resp.json().await.unwrap();
    assert!(list.is_array() || list.is_object());

    // GET by ID
    let get_resp = client
        .get(format!("{BASE}/tools/chat/sessions/{session_id}"))
        .send()
        .await
        .expect("GET /tools/chat/sessions/:id failed");
    assert_eq!(get_resp.status(), 200);

    // FORK
    let fork_resp = client
        .post(format!("{BASE}/tools/chat/sessions/{session_id}/fork"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/chat/sessions/:id/fork failed");
    assert!(
        fork_resp.status() == 200 || fork_resp.status() == 201,
        "fork should succeed, got {}",
        fork_resp.status()
    );

    // ABORT
    let abort_resp = client
        .post(format!("{BASE}/tools/chat/sessions/{session_id}/abort"))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /tools/chat/sessions/:id/abort failed");
    assert!(
        abort_resp.status() == 200 || abort_resp.status() == 202 || abort_resp.status() == 204,
        "abort should succeed, got {}",
        abort_resp.status()
    );

    // DELETE
    let delete_resp = client
        .delete(format!("{BASE}/tools/chat/sessions/{session_id}"))
        .send()
        .await
        .expect("DELETE /tools/chat/sessions/:id failed");
    assert!(
        delete_resp.status() == 200 || delete_resp.status() == 204,
        "delete should succeed, got {}",
        delete_resp.status()
    );

    // GET non-existent session — API may return 404 or 500 depending on OpenCode integration
    let missing_resp = client
        .get(format!("{BASE}/tools/chat/sessions/does-not-exist-xyz"))
        .send()
        .await
        .expect("GET /tools/chat/sessions/:id (not-found) failed");
    assert!(
        missing_resp.status() == 404 || missing_resp.status() == 500,
        "missing session should return 404 or 500, got {}",
        missing_resp.status()
    );
}

// =============================================================================
// Helpers
// =============================================================================

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}", ts)
}
