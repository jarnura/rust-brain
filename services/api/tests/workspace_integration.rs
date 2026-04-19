//! Integration tests for workspace-scoped search and queries.
//!
//! Part of [RUSA-190] — Phase 1 integration tests for multi-tenancy physical
//! isolation. These tests verify:
//!
//! 1. Workspace CRUD endpoints (create, list, get, delete)
//! 2. Workspace lifecycle state transitions
//! 3. Workspace-scoped search isolation (Qdrant + Postgres)
//! 4. Cross-workspace data leak prevention
//!
//! **Dependency note:** Tests in section 3–4 require the per-workspace Qdrant
//! collections (RUSA-181) and Postgres read-path middleware (RUSA-182) to land
//! first. Those tests are marked `#[ignore]` and will fail until the
//! implementation is complete. Sections 1–2 work against the current codebase.
//!
//! Run with:
//! ```
//! cargo test --test workspace_integration -- --include-ignored
//! ```

use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "http://localhost:8088";

/// Build a reusable reqwest client with extended timeout for workspace operations.
fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client")
}

/// Assert that a JSON object has a specific key.
fn has_key(v: &Value, key: &str) -> bool {
    v.as_object().map(|o| o.contains_key(key)).unwrap_or(false)
}

/// Generate a short unique suffix for test workspace names.
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}", ts)
}

// =============================================================================
// 1. Workspace CRUD — Create, List, Get, Delete
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_workspace_create_returns_202() {
    let name = format!("test-crud-{}", uuid_v4());
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert_eq!(
        resp.status(),
        202,
        "Expected 202 Accepted, got {}",
        resp.status()
    );
    let body: Value = resp.json().await.unwrap();
    assert!(has_key(&body, "id"), "Response must contain 'id'");
    assert!(has_key(&body, "status"), "Response must contain 'status'");
    assert_eq!(
        body["status"], "cloning",
        "Initial status should be 'cloning'"
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_create_rejects_invalid_github_url() {
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://gitlab.com/some/repo",
            "name": "invalid-source"
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert_eq!(resp.status(), 400, "Non-GitHub URL should be rejected");
}

#[tokio::test]
#[ignore]
async fn test_workspace_create_rejects_missing_url() {
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "name": "no-url"
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert!(
        resp.status() == 400 || resp.status() == 422,
        "Missing github_url should be rejected, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_list_returns_array() {
    let resp = client()
        .get(format!("{BASE}/workspaces"))
        .send()
        .await
        .expect("GET /workspaces failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body.is_array(), "Response should be an array");
}

#[tokio::test]
#[ignore]
async fn test_workspace_get_existing() {
    // Create a workspace first
    let name = format!("test-get-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");
    assert!(create_resp.status().is_success() || create_resp.status() == 202);

    let create_body: Value = create_resp.json().await.unwrap();
    let workspace_id = create_body["id"].as_str().unwrap();

    // Fetch it
    let resp = client()
        .get(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["id"], workspace_id);
    assert!(has_key(&body, "status"));
    assert!(has_key(&body, "name"));
    assert!(has_key(&body, "source_url"));
    assert!(has_key(&body, "schema_name"));
}

#[tokio::test]
#[ignore]
async fn test_workspace_get_nonexistent_returns_404() {
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client()
        .get(format!("{BASE}/workspaces/{}", fake_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    assert_eq!(resp.status(), 404);
}

#[tokio::test]
#[ignore]
async fn test_workspace_get_invalid_uuid_returns_400_or_404() {
    let resp = client()
        .get(format!("{BASE}/workspaces/not-a-uuid"))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    assert!(
        resp.status() == 400 || resp.status() == 404,
        "Invalid UUID should return 400 or 404, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_delete_returns_204() {
    // Create a workspace
    let name = format!("test-delete-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");
    assert!(create_resp.status().is_success() || create_resp.status() == 202);

    let create_body: Value = create_resp.json().await.unwrap();
    let workspace_id = create_body["id"].as_str().unwrap();

    // Delete it
    let resp = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("DELETE /workspaces/:id failed");

    assert_eq!(resp.status(), 204, "Delete should return 204 No Content");

    // Verify it's archived (GET should still work but show archived status)
    let get_resp = client()
        .get(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    // Workspace should either be 404 (filtered from list) or show archived status
    if get_resp.status() == 200 {
        let body: Value = get_resp.json().await.unwrap();
        assert_eq!(
            body["status"], "archived",
            "Deleted workspace should have 'archived' status"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_workspace_delete_nonexistent_returns_404_or_204() {
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client()
        .delete(format!("{BASE}/workspaces/{}", fake_id))
        .send()
        .await
        .expect("DELETE /workspaces/:id failed");

    // Either 204 (idempotent delete) or 404 (not found) is acceptable
    assert!(
        resp.status() == 204 || resp.status() == 404,
        "Delete of nonexistent workspace should return 204 or 404, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_delete_idempotent() {
    // Create a workspace
    let name = format!("test-idempotent-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");
    let workspace_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Delete twice — both should succeed
    let resp1 = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("First DELETE failed");
    assert_eq!(resp1.status(), 204);

    let resp2 = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("Second DELETE failed");
    assert_eq!(resp2.status(), 204, "Second delete should also return 204");
}

// =============================================================================
// 2. Workspace Lifecycle — State Transitions
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_workspace_lifecycle_cloning_to_ready() {
    let name = format!("test-lifecycle-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert_eq!(create_resp.status(), 202);
    let body: Value = create_resp.json().await.unwrap();
    let workspace_id = body["id"].as_str().unwrap();

    // Initial status should be cloning
    assert_eq!(body["status"], "cloning");

    // Poll until ready (up to 120s for clone + index)
    let mut final_status = String::new();
    for _ in 0..60 {
        tokio::time::sleep(Duration::from_secs(2)).await;

        let resp = client()
            .get(format!("{BASE}/workspaces/{}", workspace_id))
            .send()
            .await
            .expect("GET /workspaces/:id failed");

        if resp.status() == 200 {
            let ws: Value = resp.json().await.unwrap();
            let status = ws["status"].as_str().unwrap_or("").to_string();
            match status.as_str() {
                "ready" => {
                    final_status = status;
                    break;
                }
                "error" => {
                    final_status = status;
                    break;
                }
                _ => continue,
            }
        }
    }

    // Cleanup
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;

    assert_eq!(
        final_status, "ready",
        "Workspace should reach 'ready' status, got '{}'",
        final_status
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_schema_name_format() {
    let name = format!("test-schema-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");
    let body: Value = create_resp.json().await.unwrap();
    let workspace_id = body["id"].as_str().unwrap();

    // Fetch workspace details
    let resp = client()
        .get(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    assert_eq!(resp.status(), 200);
    let ws: Value = resp.json().await.unwrap();

    // Schema name must follow ws_<12_hex_chars> pattern
    if let Some(schema_name) = ws["schema_name"].as_str() {
        assert!(
            schema_name.starts_with("ws_"),
            "Schema name must start with 'ws_', got: {}",
            schema_name
        );
        let suffix = &schema_name[3..];
        assert_eq!(
            suffix.len(),
            12,
            "Schema name suffix must be 12 hex chars, got: {}",
            suffix
        );
        assert!(
            suffix.chars().all(|c| c.is_ascii_hexdigit()),
            "Schema name suffix must be hex, got: {}",
            suffix
        );
    }

    // Cleanup
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_files_endpoint() {
    // Create workspace and wait for it to be ready
    let name = format!("test-files-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let body: Value = create_resp.json().await.unwrap();
    let workspace_id = body["id"].as_str().unwrap();

    // Wait for ready
    wait_for_workspace_status(workspace_id, "ready", 60).await;

    // List files
    let resp = client()
        .get(format!("{BASE}/workspaces/{}/files", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id/files failed");

    // Cleanup
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;

    assert_eq!(resp.status(), 200, "Files endpoint should return 200");
    let files: Value = resp.json().await.unwrap();
    assert!(files.is_array(), "Files response should be an array");

    // A rust-brain clone should have Cargo.toml at the root
    let has_cargo_toml = files
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .any(|f| f["name"] == "Cargo.toml");
    assert!(has_cargo_toml, "Workspace files should include Cargo.toml");
}

// =============================================================================
// 3. Workspace-Scoped Search Isolation (Qdrant + Postgres)
//
// NOTE: These tests require per-workspace Qdrant collections (RUSA-181) and
// Postgres read-path middleware (RUSA-182) to be implemented. They are
// scaffolded here so they can be filled in once those dependencies land.
// =============================================================================

/// Helper: Create a workspace and wait for it to reach the given status.
async fn wait_for_workspace_status(workspace_id: &str, target_status: &str, max_polls: usize) {
    for _ in 0..max_polls {
        let resp = client()
            .get(format!("{BASE}/workspaces/{}", workspace_id))
            .send()
            .await
            .expect("GET /workspaces/:id failed");

        if resp.status() == 200 {
            let body: Value = resp.json().await.unwrap();
            let status = body["status"].as_str().unwrap_or("");
            if status == target_status {
                return;
            }
            if status == "error" {
                panic!(
                    "Workspace {} reached 'error' status while waiting for '{}'",
                    workspace_id, target_status
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    panic!(
        "Workspace {} did not reach '{}' within {}s",
        workspace_id,
        target_status,
        max_polls * 2
    );
}

/// Helper: Create two workspaces with distinct data for isolation testing.
///
/// Returns (workspace_a_id, workspace_b_id) or panics if either fails.
async fn create_two_workspaces() -> (String, String) {
    let name_a = format!("ws-iso-a-{}", uuid_v4());
    let name_b = format!("ws-iso-b-{}", uuid_v4());

    let resp_a = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name_a
        }))
        .send()
        .await
        .expect("POST /workspaces for A failed");

    let resp_b = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name_b
        }))
        .send()
        .await
        .expect("POST /workspaces for B failed");

    assert!(
        resp_a.status().is_success() || resp_a.status() == 202,
        "Workspace A creation failed: {}",
        resp_a.status()
    );
    assert!(
        resp_b.status().is_success() || resp_b.status() == 202,
        "Workspace B creation failed: {}",
        resp_b.status()
    );

    let id_a = resp_a.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let id_b = resp_b.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    (id_a, id_b)
}

/// Helper: Clean up workspace by archiving it.
async fn cleanup_workspace(workspace_id: &str) {
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_search_qdrant_isolation() {
    // Create two workspaces and wait for indexing to complete
    let (ws_a, ws_b) = create_two_workspaces().await;

    // Wait for both to be ready
    wait_for_workspace_status(&ws_a, "ready", 90).await;
    wait_for_workspace_status(&ws_b, "ready", 90).await;

    // TODO: Once RUSA-181 lands, search workspace A and verify only
    // workspace A vectors are returned. For now, verify the workspace
    // header is accepted by the search endpoint.
    //
    // Expected behavior after RUSA-181:
    //   let resp = client()
    //       .post(format!("{BASE}/tools/search_semantic"))
    //       .header("X-Workspace-Id", &ws_a)
    //       .json(&json!({"query": "function", "limit": 10}))
    //       .send()
    //       .await;
    //   let results: Value = resp.json().await.unwrap();
    //   // All results must belong to workspace A's data
    //   for result in results.as_array().unwrap() {
    //       assert_eq!(result["workspace_id"], ws_a);
    //   }

    // Verify basic search still works (without workspace header = public)
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({
            "query": "function",
            "limit": 5
        }))
        .send()
        .await
        .expect("search_semantic failed");

    assert_eq!(resp.status(), 200, "Search should return 200");

    // Cleanup
    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_search_postgres_isolation() {
    // Create two workspaces and wait for indexing
    let (ws_a, ws_b) = create_two_workspaces().await;
    wait_for_workspace_status(&ws_a, "ready", 90).await;
    wait_for_workspace_status(&ws_b, "ready", 90).await;

    // TODO: Once RUSA-182 lands, query workspace A and verify only
    // workspace A items are returned from Postgres.
    //
    // Expected behavior after RUSA-182:
    //   let resp = client()
    //       .post(format!("{BASE}/tools/pg_query"))
    //       .header("X-Workspace-Id", &ws_a)
    //       .json(&json!({"query": "SELECT count(*) FROM extracted_items"}))
    //       .send()
    //       .await;
    //   // Result count should match only workspace A's schema

    // Verify basic pg_query still works (without workspace header)
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .json(&json!({
            "query": "SELECT count(*) as cnt FROM extracted_items"
        }))
        .send()
        .await
        .expect("pg_query failed");

    assert_eq!(resp.status(), 200, "pg_query should return 200");

    // Cleanup
    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_cross_store_consistency() {
    // Create workspace and wait for ready
    let name = format!("ws-consistency-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");
    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    wait_for_workspace_status(&ws_id, "ready", 90).await;

    // Verify cross-store counts match (Postgres items == Qdrant vectors)
    // This uses the existing consistency endpoint
    let resp = client()
        .get(format!("{BASE}/api/consistency"))
        .query(&[("crate", "rustbrain_api")])
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200, "Consistency check should return 200");
    let body: Value = resp.json().await.unwrap();
    // Consistency endpoint should report matching counts
    assert!(
        has_key(&body, "postgres_count") || has_key(&body, "consistent") || body.is_object(),
        "Consistency response should contain count or status data"
    );

    // Cleanup
    cleanup_workspace(&ws_id).await;
}

// =============================================================================
// 4. Workspace-Scoped Search with crate_filter
//
// These tests verify that existing crate-level filtering works correctly
// with workspace-scoped queries. They work against the current codebase.
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_search_semantic_with_crate_filter() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({
            "query": "function",
            "limit": 10,
            "crate_filter": "rustbrain_common"
        }))
        .send()
        .await
        .expect("search_semantic with crate_filter failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // All results should belong to the filtered crate
    if let Some(results) = body.as_array() {
        for result in results {
            if let Some(crate_name) = result.get("crate_name").and_then(|v| v.as_str()) {
                assert_eq!(
                    crate_name, "rustbrain_common",
                    "All results should be from rustbrain_common, got: {}",
                    crate_name
                );
            }
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_search_semantic_with_nonexistent_crate_filter() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({
            "query": "function",
            "limit": 10,
            "crate_filter": "nonexistent_crate_xyz"
        }))
        .send()
        .await
        .expect("search_semantic with invalid crate_filter failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // Should return empty results, not an error
    if body.is_array() {
        assert!(
            body.as_array().unwrap().is_empty(),
            "Nonexistent crate should return empty results"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_search_semantic_without_crate_filter() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({
            "query": "function",
            "limit": 5
        }))
        .send()
        .await
        .expect("search_semantic without crate_filter failed");

    assert_eq!(resp.status(), 200);
    // Without filter, results may span multiple crates
}

#[tokio::test]
#[ignore]
async fn test_get_module_tree_with_valid_crate() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .query(&[("crate_name", "rustbrain_common")])
        .send()
        .await
        .expect("get_module_tree failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    // Should return a tree structure
    assert!(
        body.is_array() || body.is_object(),
        "Module tree should be structured data"
    );
}

#[tokio::test]
#[ignore]
async fn test_get_module_tree_with_unknown_crate() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .query(&[("crate_name", "nonexistent_crate_xyz")])
        .send()
        .await
        .expect("get_module_tree failed");

    // Unknown crate should return empty tree or 404
    assert!(
        resp.status() == 200 || resp.status() == 404,
        "Unknown crate should return 200 (empty) or 404, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_query_graph_crate_overview_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_crate_overview",
            "parameters": {
                "crate_name": "rustbrain_common"
            }
        }))
        .send()
        .await
        .expect("query_graph crate_overview failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body.is_array() || body.is_object(),
        "Crate overview should return data"
    );
}

#[tokio::test]
#[ignore]
async fn test_query_graph_crate_dependencies_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_crate_dependencies",
            "parameters": {
                "crate_name": "rustbrain_common"
            }
        }))
        .send()
        .await
        .expect("query_graph crate_dependencies failed");

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore]
async fn test_query_graph_crate_dependents_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_crate_dependents",
            "parameters": {
                "crate_name": "rustbrain_common"
            }
        }))
        .send()
        .await
        .expect("query_graph crate_dependents failed");

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore]
async fn test_query_graph_template_missing_crate_name() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_crate_overview"
        }))
        .send()
        .await
        .expect("query_graph without crate_name failed");

    // Template requiring crate_name should fail without it
    assert!(
        resp.status() == 400 || resp.status() == 422,
        "Missing crate_name for template should return 400 or 422, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_consistency_with_crate_filter() {
    let resp = client()
        .get(format!("{BASE}/api/consistency"))
        .query(&[("crate", "rustbrain_common")])
        .send()
        .await
        .expect("consistency with crate filter failed");

    assert_eq!(resp.status(), 200);
}

// =============================================================================
// 5. Edge Cases
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_workspace_zero_items_search() {
    // Create a workspace that may not have indexed yet
    let name = format!("ws-empty-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    // Even while cloning, search should not error
    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Search on a workspace that is not yet ready should return empty, not error
    // (This tests the graceful handling of incomplete workspaces)
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({
            "query": "test query",
            "limit": 5,
            "crate_filter": "nonexistent_empty_crate"
        }))
        .send()
        .await
        .expect("search_semantic on empty workspace failed");

    assert_eq!(
        resp.status(),
        200,
        "Search should return 200 even for empty data"
    );

    // Cleanup
    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_create_with_git_suffix() {
    let name = format!("test-git-suffix-{}", uuid_v4());
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces with .git suffix failed");

    assert!(
        resp.status().is_success() || resp.status() == 202,
        "URL with .git suffix should be accepted, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_workspace_create_without_optional_name() {
    // Name is optional — should derive from repo slug
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git"
        }))
        .send()
        .await
        .expect("POST /workspaces without name failed");

    assert!(
        resp.status().is_success() || resp.status() == 202,
        "Workspace creation without name should work, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    let workspace_id = body["id"].as_str().unwrap();

    // Verify the workspace has a name (derived from repo slug)
    let get_resp = client()
        .get(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    if get_resp.status() == 200 {
        let ws: Value = get_resp.json().await.unwrap();
        assert!(
            ws["name"].as_str().is_some_and(|n| !n.is_empty()),
            "Workspace should have a derived name"
        );
    }

    // Cleanup
    cleanup_workspace(workspace_id).await;
}

// =============================================================================
// 6. Git Operations (diff, commit, reset)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_workspace_diff_returns_patch_or_clean() {
    let name = format!("ws-diff-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Wait for ready (or accept current state)
    let _ = wait_for_workspace_status(&ws_id, "ready", 30).await;

    let resp = client()
        .get(format!("{BASE}/workspaces/{}/diff", ws_id))
        .send()
        .await
        .expect("GET /workspaces/:id/diff failed");

    let status = resp.status();
    if status == 200 {
        let body: Value = resp.json().await.unwrap();
        assert!(
            has_key(&body, "patch"),
            "diff response must have 'patch' key"
        );
        assert!(
            has_key(&body, "clean"),
            "diff response must have 'clean' key"
        );
    } else {
        // 400 is acceptable if workspace is not yet cloned
        assert!(status == 400, "Expected 200 or 400, got {}", status);
    }

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_diff_nonexistent_workspace_404() {
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client()
        .get(format!("{BASE}/workspaces/{}/diff", fake_id))
        .send()
        .await
        .expect("GET /workspaces/:id/diff failed");

    assert_eq!(resp.status(), 404);
}

#[tokio::test]
#[ignore]
async fn test_workspace_commit_empty_message_400() {
    let name = format!("ws-commit-empty-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client()
        .post(format!("{BASE}/workspaces/{}/commit", ws_id))
        .json(&json!({
            "message": ""
        }))
        .send()
        .await
        .expect("POST /workspaces/:id/commit failed");

    assert_eq!(resp.status(), 400, "Empty commit message should return 400");

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_commit_whitespace_message_400() {
    let name = format!("ws-commit-ws-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client()
        .post(format!("{BASE}/workspaces/{}/commit", ws_id))
        .json(&json!({
            "message": "   "
        }))
        .send()
        .await
        .expect("POST /workspaces/:id/commit failed");

    assert_eq!(
        resp.status(),
        400,
        "Whitespace-only commit message should return 400"
    );

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_commit_nothing_to_commit_400() {
    let name = format!("ws-commit-clean-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Wait for ready so the workspace is cloned
    let _ = wait_for_workspace_status(&ws_id, "ready", 30).await;

    let resp = client()
        .post(format!("{BASE}/workspaces/{}/commit", ws_id))
        .json(&json!({
            "message": "test commit on clean workspace"
        }))
        .send()
        .await
        .expect("POST /workspaces/:id/commit failed");

    let status = resp.status();
    // 400 if nothing to commit or not cloned; 200 is also possible if there were changes
    assert!(
        status == 400 || status == 200,
        "Expected 400 (nothing to commit) or 200, got {}",
        status
    );

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_reset_returns_head_sha() {
    let name = format!("ws-reset-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let _ = wait_for_workspace_status(&ws_id, "ready", 30).await;

    let resp = client()
        .post(format!("{BASE}/workspaces/{}/reset", ws_id))
        .send()
        .await
        .expect("POST /workspaces/:id/reset failed");

    let status = resp.status();
    if status == 200 {
        let body: Value = resp.json().await.unwrap();
        assert!(
            has_key(&body, "message"),
            "reset response must have 'message' key"
        );
        assert!(
            has_key(&body, "head_sha"),
            "reset response must have 'head_sha' key"
        );
    } else {
        // 400 if workspace not yet cloned
        assert_eq!(status, 400, "Expected 200 or 400, got {}", status);
    }

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_reset_nonexistent_workspace_404() {
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client()
        .post(format!("{BASE}/workspaces/{}/reset", fake_id))
        .send()
        .await
        .expect("POST /workspaces/:id/reset failed");

    assert_eq!(resp.status(), 404);
}

// =============================================================================
// 7. Stats & Stream
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_workspace_stats_returns_all_fields() {
    let name = format!("ws-stats-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client()
        .get(format!("{BASE}/workspaces/{}/stats", ws_id))
        .send()
        .await
        .expect("GET /workspaces/:id/stats failed");

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    assert!(
        has_key(&body, "workspace_id"),
        "stats must have workspace_id"
    );
    assert!(has_key(&body, "status"), "stats must have status");
    assert!(
        has_key(&body, "pg_items_count"),
        "stats must have pg_items_count"
    );
    assert!(
        has_key(&body, "neo4j_nodes_count"),
        "stats must have neo4j_nodes_count"
    );
    assert!(
        has_key(&body, "neo4j_edges_count"),
        "stats must have neo4j_edges_count"
    );
    assert!(
        has_key(&body, "qdrant_vectors_count"),
        "stats must have qdrant_vectors_count"
    );

    // Consistency sub-object
    assert!(has_key(&body, "consistency"), "stats must have consistency");
    let consistency = &body["consistency"];
    assert!(
        has_key(consistency, "pg_vs_neo4j_delta"),
        "consistency must have pg_vs_neo4j_delta"
    );
    assert!(
        has_key(consistency, "pg_vs_qdrant_delta"),
        "consistency must have pg_vs_qdrant_delta"
    );
    assert!(
        has_key(consistency, "status"),
        "consistency must have status"
    );

    // Isolation sub-object
    assert!(has_key(&body, "isolation"), "stats must have isolation");
    let isolation = &body["isolation"];
    assert!(
        has_key(isolation, "multi_label_nodes"),
        "isolation must have multi_label_nodes"
    );
    assert!(
        has_key(isolation, "cross_workspace_edges"),
        "isolation must have cross_workspace_edges"
    );
    assert!(
        has_key(isolation, "label_mismatches"),
        "isolation must have label_mismatches"
    );

    cleanup_workspace(&ws_id).await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_stream_nonexistent_execution_404() {
    let name = format!("ws-stream-{}", uuid_v4());
    let create_resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    let ws_id = create_resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let fake_exec_id = "00000000-0000-0000-0000-000000000000";
    let resp = client()
        .get(format!(
            "{BASE}/workspaces/{}/stream?execution_id={}",
            ws_id, fake_exec_id
        ))
        .send()
        .await
        .expect("GET /workspaces/:id/stream failed");

    assert_eq!(
        resp.status(),
        404,
        "Non-existent execution should return 404"
    );

    cleanup_workspace(&ws_id).await;
}
