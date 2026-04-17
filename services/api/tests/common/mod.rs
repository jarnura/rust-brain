//! Shared test utilities for workspace integration tests.
//!
//! Provides reusable fixtures for:
//! - HTTP client construction
//! - Workspace CRUD operations (create, wait, delete)
//! - Isolation test helpers (dual-workspace provisioning)
//!
//! Used by:
//! - `workspace_integration.rs` (Phase 1 — RUSA-190)
//! - `workspace_neo4j_isolation.rs` (Phase 3 — RUSA-199)
//! - `workspace_e2e_isolation.rs` (Phase 3 — RUSA-199)

#![allow(dead_code, unused_imports, unused_variables)]

use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub const BASE: &str = "http://localhost:8088";

/// Build a reusable reqwest client with extended timeout for workspace operations.
pub fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client")
}

pub fn workspace_client(_workspace_id: &str) -> Client {
    client()
}

/// Generate a short unique suffix for test workspace names.
pub fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{:08x}", ts)
}

/// Assert that a JSON object has a specific key.
pub fn has_key(v: &Value, key: &str) -> bool {
    v.as_object().map(|o| o.contains_key(key)).unwrap_or(false)
}

/// Create a workspace via POST /workspaces and return the workspace ID.
///
/// # Panics
///
/// Panics if the creation request fails or returns a non-success status.
pub async fn create_workspace(name: &str, github_url: &str) -> String {
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": github_url,
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert!(
        resp.status().is_success() || resp.status() == 202,
        "Workspace creation failed: {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("Response missing 'id': {:?}", body))
        .to_string()
}

/// Wait for a workspace to reach the given status, polling every 2 seconds.
///
/// # Panics
///
/// Panics if the workspace reaches "error" status or the timeout expires.
pub async fn wait_for_workspace_status(workspace_id: &str, target_status: &str, max_polls: usize) {
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

/// Delete (archive) a workspace. Best-effort — does not panic on failure.
pub async fn cleanup_workspace(workspace_id: &str) {
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;
}

/// Create two workspaces with distinct names for isolation testing.
///
/// Returns `(workspace_a_id, workspace_b_id)` or panics if either fails.
pub async fn create_two_workspaces(prefix: &str, github_url: &str) -> (String, String) {
    let name_a = format!("{}-a-{}", prefix, uuid_v4());
    let name_b = format!("{}-b-{}", prefix, uuid_v4());

    let id_a = create_workspace(&name_a, github_url).await;
    let id_b = create_workspace(&name_b, github_url).await;

    (id_a, id_b)
}

/// Provision two workspaces, wait for both to be ready, and return their IDs.
///
/// Callers should use `cleanup_workspace` in their test cleanup.
pub async fn provision_two_ready_workspaces(prefix: &str, github_url: &str) -> (String, String) {
    let (ws_a, ws_b) = create_two_workspaces(prefix, github_url).await;
    wait_for_workspace_status(&ws_a, "ready", 90).await;
    wait_for_workspace_status(&ws_b, "ready", 90).await;
    (ws_a, ws_b)
}

/// Assert that all nodes in a JSON result array have the expected workspace label.
///
/// Each node is expected to have a `_labels` array containing the workspace label.
/// Returns the count of checked nodes.
pub fn assert_all_nodes_have_workspace_label(results: &[Value], expected_label: &str) -> usize {
    let mut count = 0;
    for node in results {
        if let Some(labels) = node.get("_labels").and_then(|l| l.as_array()) {
            let has_label = labels.iter().any(|l| {
                l.as_str()
                    .is_some_and(|s| s == expected_label || s.contains(expected_label))
            });
            assert!(
                has_label,
                "Node {:?} missing workspace label '{}', has labels: {:?}",
                node.get("fqn").unwrap_or(&Value::Null),
                expected_label,
                labels
            );
            count += 1;
        }
    }
    count
}

/// Assert that NO nodes in a JSON result array have the forbidden workspace label.
pub fn assert_no_nodes_have_workspace_label(results: &[Value], forbidden_label: &str) {
    for node in results {
        if let Some(labels) = node.get("_labels").and_then(|l| l.as_array()) {
            let has_forbidden = labels.iter().any(|l| {
                l.as_str()
                    .is_some_and(|s| s == forbidden_label || s.contains(forbidden_label))
            });
            assert!(
                !has_forbidden,
                "Node {:?} has forbidden workspace label '{}', labels: {:?} — CROSS-WORKSPACE LEAK DETECTED",
                node.get("fqn").unwrap_or(&Value::Null),
                forbidden_label,
                labels
            );
        }
    }
}
