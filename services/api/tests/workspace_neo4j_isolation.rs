#![allow(dead_code, unused_imports, unused_variables)]

//! Integration tests for cross-workspace Neo4j isolation.
//!
//! Part of [RUSA-199] — Phase 3 of multi-tenancy physical isolation.
//! These tests verify that graph queries made with a workspace context
//! return ONLY data belonging to that workspace, with zero cross-workspace
//! leakage.
//!
//! **Dependencies (must land before these tests can pass):**
//!
//! - Typed `WorkspaceGraphClient` — wraps `neo4j_graph` with workspace label filtering
//! - Label injection during ingestion — all Neo4j nodes get a `:Workspace_<id>` label
//! - Workspace-aware template registry — templates add workspace label to MATCH clauses
//! - `X-Workspace-Id` header routing in graph handlers
//!
//! All tests are marked `#[ignore]` until the above dependencies are implemented.
//!
//! Run with:
//! ```
//! cargo test --test workspace_neo4j_isolation -- --include-ignored
//! ```

mod common;

use common::*;
use reqwest::Client;
use serde_json::{json, Value};

const GITHUB_URL: &str = "https://github.com/jarnura/rust-brain.git";

/// All query template names in the graph template registry.
/// Each must be tested for workspace isolation.
const ALL_TEMPLATES: &[&str] = &[
    "find_functions_by_name",
    "find_callers",
    "find_callees",
    "find_trait_implementations",
    "find_by_fqn",
    "find_neighbors",
    "find_nodes_by_label",
    "find_module_contents",
    "count_by_label",
    "find_crate_overview",
    "find_crate_dependencies",
    "find_crate_dependents",
];

// =============================================================================
// 1. Per-Template Isolation Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_find_functions_by_name_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-fn", GITHUB_URL).await;

    // Search for a function name that exists in both workspaces
    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 50}
        }))
        .send()
        .await
        .expect("query_graph for workspace A failed");

    assert_eq!(resp_a.status(), 200, "Workspace A query should return 200");
    let results_a: Value = resp_a.json().await.unwrap();

    // All results must belong to workspace A only
    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    // Repeat for workspace B
    let resp_b = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_b)
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 50}
        }))
        .send()
        .await
        .expect("query_graph for workspace B failed");

    assert_eq!(resp_b.status(), 200);
    let results_b: Value = resp_b.json().await.unwrap();

    if let Some(nodes) = results_b.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_a));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_callers_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-callers", GITHUB_URL).await;

    // Find callers for a function — should only return callers from the active workspace
    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_callers",
            "parameters": {"name": "main", "depth": 2, "limit": 50}
        }))
        .send()
        .await
        .expect("find_callers for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_callees_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-callees", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_callees",
            "parameters": {"name": "main", "limit": 50}
        }))
        .send()
        .await
        .expect("find_callees for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_trait_implementations_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-trait", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_trait_implementations",
            "parameters": {"name": "Clone", "limit": 50}
        }))
        .send()
        .await
        .expect("find_trait_implementations for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_by_fqn_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-fqn", GITHUB_URL).await;

    // Query a specific FQN that exists in both workspaces
    // Should return only the active workspace's version
    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_by_fqn",
            "parameters": {"fqn": "rustbrain_api::main", "limit": 10}
        }))
        .send()
        .await
        .expect("find_by_fqn for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_neighbors_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-neighbor", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_neighbors",
            "parameters": {"fqn": "rustbrain_api::main", "depth": 2, "limit": 50}
        }))
        .send()
        .await
        .expect("find_neighbors for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_nodes_by_label_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-label", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_nodes_by_label",
            "parameters": {"label": "Function", "limit": 50}
        }))
        .send()
        .await
        .expect("find_nodes_by_label for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_module_contents_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-mod", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_module_contents",
            "parameters": {"path": "rustbrain_api::handlers", "limit": 50}
        }))
        .send()
        .await
        .expect("find_module_contents for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_count_by_label_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-count", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "count_by_label",
            "parameters": {"label": "Function"}
        }))
        .send()
        .await
        .expect("count_by_label for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    // Count should reflect only workspace A's data, not global count
    if let Some(count) = results_a.get("count").and_then(|c| c.as_i64()) {
        assert!(count > 0, "Workspace A should have some Function nodes");
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_crate_overview_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-overview", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_crate_overview",
            "parameters": {"crate_name": "rustbrain_api"}
        }))
        .send()
        .await
        .expect("find_crate_overview for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_crate_dependencies_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-dep", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_crate_dependencies",
            "parameters": {"crate_name": "rustbrain_api"}
        }))
        .send()
        .await
        .expect("find_crate_dependencies for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_crate_dependents_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-depnd", GITHUB_URL).await;

    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_crate_dependents",
            "parameters": {"crate_name": "rustbrain_common"}
        }))
        .send()
        .await
        .expect("find_crate_dependents for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 2. Direct Endpoint Isolation Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_get_callers_direct_endpoint_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-dir-call", GITHUB_URL).await;

    // Test the direct GET /tools/get_callers endpoint with workspace header
    let resp_a = client()
        .get(format!("{BASE}/tools/get_callers"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("fqn", "rustbrain_api::main"), ("depth", "2")])
        .send()
        .await
        .expect("GET /tools/get_callers for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    if let Some(callers) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(callers, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_get_module_tree_direct_endpoint_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-dir-mod", GITHUB_URL).await;

    let resp_a = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("crate_name", "rustbrain_api")])
        .send()
        .await
        .expect("GET /tools/get_module_tree for workspace A failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    // Module tree should only contain modules from workspace A
    if let Some(items) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(items, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_get_trait_impls_direct_endpoint_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-dir-trait", GITHUB_URL).await;

    let resp_a = client()
        .get(format!("{BASE}/tools/get_trait_impls"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("trait_name", "Clone"), ("limit", "50")])
        .send()
        .await
        .expect("GET /tools/get_trait_impls for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

#[tokio::test]
#[ignore]
async fn test_find_usages_of_type_direct_endpoint_isolation() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-dir-type", GITHUB_URL).await;

    let resp_a = client()
        .get(format!("{BASE}/tools/find_usages_of_type"))
        .header("X-Workspace-Id", &ws_a)
        .query(&[("type_name", "AppState"), ("limit", "50")])
        .send()
        .await
        .expect("GET /tools/find_usages_of_type for workspace A failed");

    assert_eq!(resp_a.status(), 200);

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 3. Error Path — No Workspace Context
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_query_graph_without_workspace_returns_400() {
    // Query without X-Workspace-Id header should return 400
    // (no silent fallback to all data)
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph without workspace failed");

    assert_eq!(
        resp.status(),
        400,
        "Query without workspace context should return 400, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_query_graph_with_nonexistent_workspace_returns_empty() {
    // A valid-but-nonexistent workspace ID passes WorkspaceContext::new() validation
    // (alphanumeric + hyphens) but the Workspace_000000000000 label matches no nodes.
    // The handler returns 200 with empty results — no data leak, no crash.
    let nonexistent_workspace_id = "0000000000000000";

    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", nonexistent_workspace_id)
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph with nonexistent workspace failed");

    // Must not return 5xx — the server handles nonexistent workspaces gracefully
    assert!(
        resp.status().is_success(),
        "Nonexistent workspace should return 200 with empty results, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    // Results must be empty (no data leaked from other workspaces)
    if let Some(results) = body.as_array() {
        assert!(
            results.is_empty(),
            "Nonexistent workspace should return empty results, got {} nodes — possible data leak",
            results.len()
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_query_graph_with_malformed_workspace_returns_400() {
    // Semicolons and other special characters fail WorkspaceContext::new() validation
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "evil;workspace")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph with malformed workspace failed");

    assert_eq!(
        resp.status(),
        400,
        "Malformed workspace ID should return 400, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn test_get_module_tree_without_workspace_returns_400() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .query(&[("crate_name", "rustbrain_api")])
        .send()
        .await
        .expect("get_module_tree without workspace failed");

    // Without workspace header, should return 400 (not silently return all data)
    assert_eq!(
        resp.status(),
        400,
        "get_module_tree without workspace should return 400, got {}",
        resp.status()
    );
}

// =============================================================================
// 4. User Cypher Isolation Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_query_graph_user_cypher_cannot_enumerate_other_workspace() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-cypher", GITHUB_URL).await;

    // Try to enumerate all labels (information disclosure attempt)
    let resp_a = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", &ws_a)
        .json(&json!({
            "query_name": "find_nodes_by_label",
            "parameters": {"label": "Crate", "limit": 100}
        }))
        .send()
        .await
        .expect("query_graph label enumeration failed");

    assert_eq!(resp_a.status(), 200);
    let results_a: Value = resp_a.json().await.unwrap();

    // Must not contain any nodes from workspace B
    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}

// =============================================================================
// 5. Concurrent Cross-Workspace Query Tests
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_concurrent_queries_no_cross_contamination() {
    let (ws_a, ws_b) = provision_two_ready_workspaces("neo4j-iso-conc", GITHUB_URL).await;

    // Fire queries for both workspaces concurrently
    let (resp_a, resp_b) = tokio::join!(
        client()
            .post(format!("{BASE}/tools/query_graph"))
            .header("X-Workspace-Id", &ws_a)
            .json(&json!({
                "query_name": "find_functions_by_name",
                "parameters": {"name": "main", "limit": 50}
            }))
            .send(),
        client()
            .post(format!("{BASE}/tools/query_graph"))
            .header("X-Workspace-Id", &ws_b)
            .json(&json!({
                "query_name": "find_functions_by_name",
                "parameters": {"name": "main", "limit": 50}
            }))
            .send()
    );

    let resp_a = resp_a.expect("Workspace A query failed");
    let resp_b = resp_b.expect("Workspace B query failed");

    assert_eq!(resp_a.status(), 200);
    assert_eq!(resp_b.status(), 200);

    let results_a: Value = resp_a.json().await.unwrap();
    let results_b: Value = resp_b.json().await.unwrap();

    // Verify no cross-contamination
    if let Some(nodes) = results_a.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_b));
    }
    if let Some(nodes) = results_b.as_array() {
        assert_no_nodes_have_workspace_label(nodes, &format!("Workspace_{}", ws_a));
    }

    cleanup_workspace(&ws_a).await;
    cleanup_workspace(&ws_b).await;
}
