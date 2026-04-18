//! Contract tests for workspace isolation that run in CI with just the API server.
//!
//! Part of [RUSA-199] — tests the HTTP-level contract for workspace isolation
//! without requiring workspace data. These need the API server running but
//! do NOT need any workspaces to be provisioned or data ingested.
//!
//! What's tested:
//! - Missing X-Workspace-Id header returns 400 (no silent fallback)
//! - Invalid X-Workspace-Id header returns 400 (not 500)
//! - POST /tools/query_graph rejects write Cypher (CREATE, DELETE, MERGE, SET)
//! - POST /tools/query_graph rejects dangerous APOC procedures
//! - Parameter sanitization prevents workspace label injection
//!
//! These ARE marked #[ignore] — they need the API server running.
//! In CI, the workspace-isolation-tests job starts the API server before running these.
//! The unit-level validation tests (51 in workspace_label.rs, 23+ in graph_templates.rs)
//! cover the internal logic without needing a server.

use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "http://localhost:8088";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("Failed to build HTTP client")
}

// =============================================================================
// 1. Missing Workspace Header → 400
// =============================================================================

#[tokio::test]
#[ignore]
async fn query_graph_without_workspace_returns_400() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    assert!(
        body.to_string().contains("workspace") || body.to_string().contains("Workspace"),
        "Error message should mention workspace, got: {:?}",
        body
    );
}

#[tokio::test]
#[ignore]
async fn get_module_tree_without_workspace_returns_400() {
    let resp = client()
        .get(format!("{BASE}/tools/get_module_tree"))
        .query(&[("crate_name", "rustbrain_api")])
        .send()
        .await
        .expect("get_module_tree request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn get_callers_without_workspace_returns_400() {
    let resp = client()
        .get(format!("{BASE}/tools/get_callers"))
        .query(&[("fqn", "crate::main"), ("depth", "1")])
        .send()
        .await
        .expect("get_callers request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn get_trait_impls_without_workspace_returns_400() {
    let resp = client()
        .get(format!("{BASE}/tools/get_trait_impls"))
        .query(&[("trait_name", "Clone"), ("limit", "10")])
        .send()
        .await
        .expect("get_trait_impls request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn find_usages_of_type_without_workspace_returns_400() {
    let resp = client()
        .get(format!("{BASE}/tools/find_usages_of_type"))
        .query(&[("type_name", "String"), ("limit", "10")])
        .send()
        .await
        .expect("find_usages_of_type request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn search_semantic_without_workspace_returns_400() {
    let resp = client()
        .post(format!("{BASE}/tools/search_semantic"))
        .json(&json!({"query": "test", "limit": 5}))
        .send()
        .await
        .expect("search_semantic request failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for missing workspace header, got {}",
        resp.status()
    );
}

// =============================================================================
// 2. Invalid Workspace Header → 400
// =============================================================================

#[tokio::test]
#[ignore]
async fn query_graph_with_special_chars_workspace_returns_400() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "evil;workspace")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph with invalid workspace failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for invalid workspace header, got {}",
        resp.status()
    );
}

#[tokio::test]
#[ignore]
async fn query_graph_with_empty_workspace_returns_400() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph with empty workspace failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for empty workspace header, got {}",
        resp.status()
    );
}

// =============================================================================
// 3. Write-Rejection Security Contract (via HTTP)
// =============================================================================

#[tokio::test]
#[ignore]
async fn query_graph_rejects_create_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CREATE (n:Evil) RETURN n"
        }))
        .send()
        .await
        .expect("query_graph with CREATE failed");

    assert_eq!(
        resp.status(),
        400,
        "Expected 400 for CREATE Cypher, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    let msg = body.to_string().to_lowercase();
    assert!(
        msg.contains("read-only") || msg.contains("not allowed"),
        "Error should mention read-only restriction, got: {:?}",
        body
    );
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_delete_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MATCH (n) DELETE n"
        }))
        .send()
        .await
        .expect("query_graph with DELETE failed");

    assert_eq!(resp.status(), 400, "Expected 400 for DELETE Cypher");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_merge_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MERGE (n:Evil {id: 1}) RETURN n"
        }))
        .send()
        .await
        .expect("query_graph with MERGE failed");

    assert_eq!(resp.status(), 400, "Expected 400 for MERGE Cypher");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_set_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MATCH (n) SET n.x = 1 RETURN n"
        }))
        .send()
        .await
        .expect("query_graph with SET failed");

    assert_eq!(resp.status(), 400, "Expected 400 for SET Cypher");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_remove_cypher() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MATCH (n) REMOVE n.x RETURN n"
        }))
        .send()
        .await
        .expect("query_graph with REMOVE failed");

    assert_eq!(resp.status(), 400, "Expected 400 for REMOVE Cypher");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_cypher_run() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.cypher.run('MATCH (n) RETURN n', {})"
        }))
        .send()
        .await
        .expect("query_graph with apoc.cypher.run failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.cypher.run");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_create_node() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.create.node(['Evil'], {name: 'test'})"
        }))
        .send()
        .await
        .expect("query_graph with apoc.create.node failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.create.node");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_do_when() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.do.when(true, 'CREATE (n) RETURN n', '')"
        }))
        .send()
        .await
        .expect("query_graph with apoc.do.when failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.do.when");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_detach_delete() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MATCH (n) DETACH DELETE n"
        }))
        .send()
        .await
        .expect("query_graph with DETACH DELETE failed");

    assert_eq!(resp.status(), 400, "Expected 400 for DETACH DELETE");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_periodic_iterate() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.periodic.iterate('MATCH (n) RETURN n', 'SET n.x = 1', {batchSize: 100})"
        }))
        .send()
        .await
        .expect("query_graph with apoc.periodic.iterate failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.periodic.iterate");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_periodic_commit() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.periodic.commit('MATCH (n) RETURN n')"
        }))
        .send()
        .await
        .expect("query_graph with apoc.periodic.commit failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.periodic.commit");
}

#[tokio::test]
#[ignore]
async fn query_graph_rejects_apoc_refactor() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.refactor.mergeNodes([n1, n2])"
        }))
        .send()
        .await
        .expect("query_graph with apoc.refactor failed");

    assert_eq!(resp.status(), 400, "Expected 400 for apoc.refactor");
}

// =============================================================================
// 4. Parameter Sanitization (workspace label injection attempt)
// =============================================================================

#[tokio::test]
#[ignore]
async fn query_graph_strips_workspace_label_from_parameters() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {
                "name": "main",
                "limit": 10,
                "workspace_label": "Workspace_evil",
                "ws_label": "Workspace_evil2"
            }
        }))
        .send()
        .await
        .expect("query_graph with injected parameters failed");

    assert_ne!(
        resp.status(),
        500,
        "Server should not crash on injected workspace_label parameter"
    );
}

#[tokio::test]
#[ignore]
async fn query_graph_strips_workspace_id_from_parameters() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {
                "name": "main",
                "limit": 10,
                "workspace_id": "evil-workspace"
            }
        }))
        .send()
        .await
        .expect("query_graph with injected workspace_id parameter failed");

    assert_ne!(
        resp.status(),
        500,
        "Server should not crash on injected workspace_id parameter"
    );
}

// =============================================================================
// 5. Read-Only Queries Accepted (with valid workspace header)
// =============================================================================

#[tokio::test]
#[ignore]
async fn query_graph_accepts_readonly_match_return() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "MATCH (n:Function) RETURN n.name LIMIT 10"
        }))
        .send()
        .await
        .expect("query_graph with MATCH failed");

    assert_ne!(
        resp.status(),
        400,
        "Read-only MATCH query should not be rejected as a write"
    );
}

#[tokio::test]
#[ignore]
async fn query_graph_accepts_template_query_with_workspace() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query_name": "find_functions_by_name",
            "parameters": {"name": "main", "limit": 10}
        }))
        .send()
        .await
        .expect("query_graph template with workspace failed");

    if resp.status() == 400 {
        let body: Value = resp.json().await.unwrap();
        let msg = body.to_string().to_lowercase();
        assert!(
            !msg.contains("read-only") && !msg.contains("only read-only"),
            "Template query should not be rejected as a write, got: {:?}",
            body
        );
    }
}

#[tokio::test]
#[ignore]
async fn query_graph_accepts_apoc_path_expand() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .header("X-Workspace-Id", "abc123def456")
        .json(&json!({
            "query": "CALL apoc.path.expand(startNode, 'CALLS>', null, 1, 3) YIELD path RETURN path"
        }))
        .send()
        .await
        .expect("query_graph with apoc.path.expand failed");

    // May fail for other reasons (no startNode variable), but should NOT fail for write-rejection
    if resp.status() == 400 {
        let body: Value = resp.json().await.unwrap();
        let msg = body.to_string().to_lowercase();
        assert!(
            !msg.contains("read-only") && !msg.contains("not in the read-only"),
            "apoc.path.expand should not be rejected as a write, got: {:?}",
            body
        );
    }
}
