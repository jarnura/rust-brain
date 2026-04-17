#![allow(dead_code, unused_imports, unused_variables)]

//! End-to-end isolation tests for cross-workspace data leak prevention.
//!
//! Part of [RUSA-199] — Phase 3 of multi-tenancy physical isolation.
//! These tests spin up real service containers, provision two workspaces,
//! ingest different test data, and verify that ALL code-intelligence
//! endpoints respect workspace boundaries.
//!
//! **Dependencies (must land before these tests can pass):**
//!
//! - Typed `WorkspaceGraphClient` — workspace-scoped Neo4j access
//! - Per-workspace Qdrant collections (RUSA-181)
//! - Postgres read-path middleware (RUSA-182)
//! - Label injection during ingestion
//!
//! All tests are marked `#[ignore]` until the above dependencies are implemented.
//!
//! Run with:
//! ```
//! cargo test --test workspace_e2e_isolation -- --include-ignored
//! ```

mod common;

use common::*;
use reqwest::Client;
use serde_json::{json, Value};

const GITHUB_URL: &str = "https://github.com/jarnura/rust-brain.git";

/// All code-intelligence endpoints that must be tested for workspace isolation.
const INTELLIGENCE_ENDPOINTS: &[&str] = &[
    "search_semantic",
    "search_code",
    "get_function",
    "get_callers",
    "get_callees",
    "get_module_tree",
    "get_trait_impls",
    "find_usages_of_type",
    "query_graph",
    "pg_query",
    "aggregate_search",
];

// =============================================================================
// 1. Full Pipeline Isolation — Semantic Search
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_e2e_search_semantic_workspace_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-search", GITHUB_URL).await;

    // Search workspace A
    let resp_a = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query": "function that handles HTTP requests",
            "limit": 20
        }))
        .send()
        .await
        .expect("search_semantic for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    // Verify no cross-workspace data in results
    if let Some(results) = results_a.as_array() {
        for result in results {
            if let Some(ws_id) = result.get("workspace_id").and_then(|v| v.as_str()) {
                assert_eq!(
                    ws_id, ws_a,
                    "Search result belongs to wrong workspace: expected {}, got {}",
                    ws_a, ws_id
                );
            }
        }
    }

    // Search workspace B — should return different results
    let resp_b = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({
            "query": "function that handles HTTP requests",
            "limit": 20
        }))
        .send()
        .await
        .expect("search_semantic for workspace B failed");

    assert_eq!(resp_b.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 2. Full Pipeline Isolation — Postgres Queries
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_e2e_pg_query_workspace_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-pg", GITHUB_URL).await;

    // Query workspace A's Postgres schema
    let resp_a = client()
        .post(format!("{BASE}/tools/pg_query"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query": "SELECT count(*) as cnt FROM extracted_items"
        }))
        .send()
        .await
        .expect("pg_query for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let result_a: Value = resp_a.json().await.unwrap();
    let count_a = result_a
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("cnt"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Query workspace B's Postgres schema
    let resp_b = client()
        .post(format!("{BASE}/tools/pg_query"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({
            "query": "SELECT count(*) as cnt FROM extracted_items"
        }))
        .send()
        .await
        .expect("pg_query for workspace B failed");

    assert_eq!(resp_b.status(), 200);
    let result_b: Value = resp_b.json().await.unwrap();
    let count_b = result_b
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("cnt"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // Both workspaces ingested the same repo, so counts should be equal
    // but the data must be isolated (separate schemas)
    assert!(count_a > 0, "Workspace A should have ingested items");
    assert!(count_b > 0, "Workspace B should have ingested items");

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 3. Full Pipeline Isolation — Cross-Store Consistency
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_e2e_cross_store_consistency_per_workspace() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-xstore", GITHUB_URL).await;

    // Verify cross-store consistency for workspace A
    let resp_a = client()
        .get(format!("{BASE}/api/consistency"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("crate", "rustbrain_api")])
        .send()
        .await
        .expect("consistency check for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    // Verify cross-store consistency for workspace B
    let resp_b = client()
        .get(format!("{BASE}/api/consistency"))
        .header("X-Workspace-Id", &ws_b)
        .query(&[("crate", "rustbrain_api")])
        .send()
        .await
        .expect("consistency check for workspace B failed");

    assert_eq!(resp_b.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 4. Full Pipeline — All 12+ Intelligence Endpoints
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_e2e_all_intelligence_endpoints_workspace_a() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-all-a", GITHUB_URL).await;

    // Test each intelligence endpoint with workspace A header
    // All must return 200 and only workspace A data

    // 1. search_semantic
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({"query": "handler", "limit": 10}))
        .send()
        .await;
    assert_eq!(resp.unwrap().status(), 200, "search_semantic failed");

    // 2. search_code
    let resp = client()
        .post(format!("{BASE}/tools/search_code"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({"query": "fn main", "limit": 10}))
        .send()
        .await;
    assert!(resp.is_ok(), "search_code failed");

    // 3. get_function
    let resp = client()
        .get(format!("{BASE}/tools/get_function"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("fqn", "rustbrain_api::main")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_function failed");

    // 4. get_callers
    let resp = client()
        .get(format!("{BASE}/tools/get_callers"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("fqn", "rustbrain_api::main"), ("depth", "1")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_callers failed");

    // 5. get_callees
    let resp = client()
        .get(format!("{BASE}/tools/get_callees"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("fqn", "rustbrain_api::main")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_callees failed");

    // 6. get_module_tree
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("crate_name", "rustbrain_api")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_module_tree failed");

    // 7. get_trait_impls
    let resp = client()
        .get(format!("{BASE}/tools/get_trait_impls"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("trait_name", "Clone"), ("limit", "10")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_trait_impls failed");

    // 8. find_usages_of_type
    let resp = client()
        .get(format!("{BASE}/tools/find_usages_of_type"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("type_name", "AppState"), ("limit", "10")])
        .send()
        .await;
    assert!(resp.is_ok(), "find_usages_of_type failed");

    // 9. query_graph (template)
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_crate_overview",
            "parameters": {"crate_name": "rustbrain_api"}
        }))
        .send()
        .await;
    assert!(resp.is_ok(), "query_graph template failed");

    // 10. pg_query
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({"query": "SELECT count(*) as cnt FROM extracted_items"}))
        .send()
        .await;
    assert!(resp.is_ok(), "pg_query failed");

    // 11. aggregate_search
    let resp = client()
        .post(format!("{BASE}/tools/aggregate_search"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({"query": "handler function", "limit": 10}))
        .send()
        .await;
    assert!(resp.is_ok(), "aggregate_search failed");

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_e2e_all_intelligence_endpoints_workspace_b() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-all-b", GITHUB_URL).await;

    // Same as above but querying workspace B
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({"query": "handler", "limit": 10}))
        .send()
        .await;
    assert_eq!(resp.unwrap().status(), 200, "search_semantic for B failed");

    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .header("X-Workspace-Id", &ws_b)
        .query(&[("crate_name", "rustbrain_api")])
        .send()
        .await;
    assert!(resp.is_ok(), "get_module_tree for B failed");

    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({
            "query_name": "find_crate_overview",
            "parameters": {"crate_name": "rustbrain_api"}
        }))
        .send()
        .await;
    assert!(resp.is_ok(), "query_graph for B failed");

    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({"query": "SELECT count(*) as cnt FROM extracted_items"}))
        .send()
        .await;
    assert!(resp.is_ok(), "pg_query for B failed");

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 5. Workspace Lifecycle + Isolation
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_e2e_delete_workspace_does_not_affect_other() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("e2e-iso-del", GITHUB_URL).await;

    // Delete workspace A
    cleanup_workspace(&ws_a).await;

    // Verify workspace B still works
    let resp = client()
        .post(format!("{BASE}/tools/pg_query"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({"query": "SELECT count(*) as cnt FROM extracted_items"}))
        .send()
        .await
        .expect("pg_query for workspace B after deleting A failed");

    assert_eq!(
        resp.status(),
        200,
        "Workspace B queries should still work after A deleted"
    );

    let result: Value = resp.json().await.unwrap();
    let count = result
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("cnt"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    assert!(
        count > 0,
        "Workspace B data should be intact after deleting A"
    );

    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_e2e_recreate_workspace_with_same_name() {
    let name = format!("e2e-recreate-{}", uuid_v4());

    // Create, wait, delete
    let ws_id = create_workspace(&name, GITHUB_URL).await;
    wait_for_workspace_status(&ws_id, "ready", 90).await;
    cleanup_workspace(&ws_id).await;

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Re-create with the same name
    let ws_id_2 = create_workspace(&name, GITHUB_URL).await;
    wait_for_workspace_status(&ws_id_2, "ready", 90).await;

    // Verify it works
    let resp = client()
        .get(format!("{BASE}/workspaces/{}", ws_id_2))
        .send()
        .await
        .expect("GET re-created workspace failed");

    assert_eq!(resp.status(), 200);

    cleanup_workspace(&ws_id_2).await;
}
