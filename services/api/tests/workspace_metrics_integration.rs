//! Integration tests for workspace metrics endpoints.
//!
//! Tests cover:
//! - Health endpoint store counts (Postgres, Neo4j, Qdrant)
//! - Prometheus metrics scraping
//! - Cross-store consistency reports
//! - Ingestion pipeline progress
//! - Workspace index progress and status
//! - Named query templates for workspace statistics
//!
//! Run with:
//! ```
//! cargo test --test workspace_metrics_integration -- --include-ignored
//! ```

use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "http://localhost:8088";

/// Build a reusable reqwest client with extended timeout for metrics operations.
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
// Section 1: Health Metrics (GET /health)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_health_returns_store_counts() {
    let resp = client()
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Verify dependencies object exists with required stores
    assert!(
        has_key(&body, "dependencies"),
        "Response must have dependencies"
    );
    let deps = &body["dependencies"];

    // Verify Postgres count
    assert!(has_key(deps, "postgres"), "dependencies must have postgres");
    let pg_items = deps["postgres"]["items_count"].as_i64();
    assert!(
        pg_items.map(|n| n >= 0).unwrap_or(false),
        "postgres items_count must be non-negative"
    );

    // Verify Neo4j counts
    assert!(has_key(deps, "neo4j"), "dependencies must have neo4j");
    let neo4j_nodes = deps["neo4j"]["nodes_count"].as_i64();
    let neo4j_edges = deps["neo4j"]["edges_count"].as_i64();
    assert!(
        neo4j_nodes.map(|n| n >= 0).unwrap_or(false),
        "neo4j nodes_count must be non-negative"
    );
    assert!(
        neo4j_edges.map(|n| n >= 0).unwrap_or(false),
        "neo4j edges_count must be non-negative"
    );

    // Verify Qdrant count
    assert!(has_key(deps, "qdrant"), "dependencies must have qdrant");
    let qdrant_points = deps["qdrant"]["points_count"].as_i64();
    assert!(
        qdrant_points.map(|n| n >= 0).unwrap_or(false),
        "qdrant points_count must be non-negative"
    );
}

#[tokio::test]
#[ignore]
async fn test_health_uptime_increases() {
    let client = client();

    // First call
    let resp1 = client
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health first call failed");
    assert_eq!(resp1.status(), 200);
    let body1: Value = resp1.json().await.unwrap();
    let uptime1 = body1["uptime_secs"]
        .as_f64()
        .expect("uptime_secs must be a number");

    // Small delay
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Second call
    let resp2 = client
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health second call failed");
    assert_eq!(resp2.status(), 200);
    let body2: Value = resp2.json().await.unwrap();
    let uptime2 = body2["uptime_secs"]
        .as_f64()
        .expect("uptime_secs must be a number");

    // Uptime should have increased (or at least stayed the same, accounting for timing)
    assert!(
        uptime2 >= uptime1,
        "uptime_secs should increase or stay same: {} vs {}",
        uptime1,
        uptime2
    );
}

#[tokio::test]
#[ignore]
async fn test_health_status_healthy_or_degraded() {
    let resp = client()
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let status = body["status"].as_str().expect("status must be a string");
    assert!(
        status == "healthy" || status == "degraded",
        "status must be 'healthy' or 'degraded', got: {}",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_health_version_present() {
    let resp = client()
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(
        has_key(&body, "version"),
        "Response must have version field"
    );
    let version = body["version"].as_str().expect("version must be a string");
    assert!(!version.is_empty(), "version must be non-empty");
}

// =============================================================================
// Section 2: Prometheus Metrics (GET /metrics)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_metrics_prometheus_format() {
    let resp = client()
        .get(format!("{BASE}/metrics"))
        .send()
        .await
        .expect("GET /metrics failed");

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();

    // Prometheus format should contain HELP and TYPE comments
    assert!(
        text.contains("# HELP") || text.contains("# TYPE"),
        "Prometheus metrics should contain # HELP or # TYPE comments"
    );

    // Should contain rustbrain_api metric prefix
    assert!(
        text.contains("rustbrain_api"),
        "Metrics should contain rustbrain_api prefix"
    );
}

#[tokio::test]
#[ignore]
async fn test_metrics_content_type() {
    let resp = client()
        .get(format!("{BASE}/metrics"))
        .send()
        .await
        .expect("GET /metrics failed");

    assert_eq!(resp.status(), 200);

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    assert!(
        content_type.contains("text/plain"),
        "Content-Type should be text/plain, got: {}",
        content_type
    );
}

#[tokio::test]
#[ignore]
async fn test_metrics_records_requests() {
    let client = client();

    // First, make a request to /health to generate metrics
    let _ = client
        .get(format!("{BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");

    // Now check metrics
    let resp = client
        .get(format!("{BASE}/metrics"))
        .send()
        .await
        .expect("GET /metrics failed");

    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();

    // Should contain request counter with count > 0
    assert!(
        text.contains("rustbrain_api_requests_total"),
        "Metrics should contain rustbrain_api_requests_total"
    );
}

// =============================================================================
// Section 3: Cross-Store Consistency Metrics (GET /api/consistency)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_consistency_summary_report() {
    let resp = client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Required fields per ConsistencyReport
    assert!(has_key(&body, "crate_name"), "missing crate_name");
    assert!(has_key(&body, "timestamp"), "missing timestamp");
    assert!(has_key(&body, "store_counts"), "missing store_counts");
    assert!(has_key(&body, "status"), "missing status");
    assert!(has_key(&body, "recommendation"), "missing recommendation");

    // store_counts should have all three stores
    let counts = &body["store_counts"];
    assert!(
        counts["postgres"].is_number(),
        "store_counts.postgres must be number"
    );
    assert!(
        counts["neo4j"].is_number(),
        "store_counts.neo4j must be number"
    );
    assert!(
        counts["qdrant"].is_number(),
        "store_counts.qdrant must be number"
    );

    // status should be consistent or inconsistent
    let status = body["status"].as_str().expect("status must be string");
    assert!(
        status == "consistent" || status == "inconsistent",
        "unexpected status: {}",
        status
    );
}

#[tokio::test]
#[ignore]
async fn test_consistency_full_detail() {
    let resp = client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // detail=full should include discrepancies
    assert!(
        has_key(&body, "discrepancies"),
        "detail=full must include discrepancies"
    );
    let disc = &body["discrepancies"];
    assert!(disc.is_object(), "discrepancies must be an object");
}

#[tokio::test]
#[ignore]
async fn test_consistency_with_crate_filter() {
    let resp = client()
        .get(format!("{BASE}/api/consistency?crate=rustbrain_common"))
        .send()
        .await
        .expect("GET /api/consistency?crate=... failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert_eq!(
        body["crate_name"], "rustbrain_common",
        "crate_name should match filter"
    );
}

#[tokio::test]
#[ignore]
async fn test_consistency_invalid_detail_param() {
    let resp = client()
        .get(format!("{BASE}/api/consistency?detail=invalid"))
        .send()
        .await
        .expect("GET /api/consistency?detail=invalid failed");

    // Should still return 200 (graceful handling)
    assert_eq!(
        resp.status(),
        200,
        "Invalid detail param should be handled gracefully"
    );
}

// =============================================================================
// Section 4: Health Consistency (GET /health/consistency)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_health_consistency_returns_status() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    // Should return 200 (all consistent) or 503 (some inconsistent)
    assert!(
        resp.status() == 200 || resp.status() == 503,
        "expected 200 or 503, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();

    // Required fields
    assert!(has_key(&body, "status"), "missing status");
    assert!(has_key(&body, "total_crates"), "missing total_crates");
    assert!(
        has_key(&body, "inconsistent_crates"),
        "missing inconsistent_crates"
    );
    assert!(has_key(&body, "crates"), "missing crates array");

    // crates should be an array
    assert!(body["crates"].is_array(), "crates must be an array");
}

#[tokio::test]
#[ignore]
async fn test_health_consistency_crate_summaries() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let body: Value = resp.json().await.unwrap();

    if let Some(crates) = body["crates"].as_array() {
        for crate_summary in crates {
            assert!(
                crate_summary["crate_name"].is_string(),
                "crate_summary missing crate_name"
            );
            assert!(
                crate_summary["consistent"].is_boolean(),
                "crate_summary missing consistent boolean"
            );
            assert!(
                has_key(crate_summary, "counts"),
                "crate_summary missing counts"
            );

            let counts = &crate_summary["counts"];
            assert!(
                counts["postgres"].is_number(),
                "counts.postgres must be number"
            );
            assert!(counts["neo4j"].is_number(), "counts.neo4j must be number");
            assert!(counts["qdrant"].is_number(), "counts.qdrant must be number");
        }
    }
}

// =============================================================================
// Section 5: Ingestion Progress (GET /api/ingestion/progress)
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_ingestion_progress_structure() {
    let resp = client()
        .get(format!("{BASE}/api/ingestion/progress"))
        .send()
        .await
        .expect("GET /api/ingestion/progress failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Required fields
    assert!(has_key(&body, "status"), "missing status");
    assert!(has_key(&body, "started_at"), "missing started_at");
    assert!(
        has_key(&body, "crates_processed"),
        "missing crates_processed"
    );
    assert!(has_key(&body, "items_extracted"), "missing items_extracted");
    assert!(has_key(&body, "stages"), "missing stages");

    // stages should be an array
    assert!(body["stages"].is_array(), "stages must be an array");
}

#[tokio::test]
#[ignore]
async fn test_ingestion_progress_stages() {
    let resp = client()
        .get(format!("{BASE}/api/ingestion/progress"))
        .send()
        .await
        .expect("GET /api/ingestion/progress failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    if let Some(stages) = body["stages"].as_array() {
        if !stages.is_empty() {
            for stage in stages {
                assert!(has_key(stage, "name"), "stage missing name");
                assert!(has_key(stage, "status"), "stage missing status");
                assert!(
                    has_key(stage, "items_processed"),
                    "stage missing items_processed"
                );
            }
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_ingestion_progress_items_non_negative() {
    let resp = client()
        .get(format!("{BASE}/api/ingestion/progress"))
        .send()
        .await
        .expect("GET /api/ingestion/progress failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let crates_processed = body["crates_processed"].as_i64().unwrap_or(-1);
    let items_extracted = body["items_extracted"].as_i64().unwrap_or(-1);

    assert!(
        crates_processed >= 0,
        "crates_processed must be non-negative, got {}",
        crates_processed
    );
    assert!(
        items_extracted >= 0,
        "items_extracted must be non-negative, got {}",
        items_extracted
    );
}

// =============================================================================
// Section 6: Workspace Metrics via query_graph Templates
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_count_by_label_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "count_by_label",
            "parameters": {
                "label": "Function"
            }
        }))
        .send()
        .await
        .expect("POST /tools/query_graph with count_by_label failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Response should contain count data
    assert!(
        body.is_array() || body.is_object(),
        "Response should contain data"
    );
}

#[tokio::test]
#[ignore]
async fn test_count_by_label_invalid_label() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "count_by_label",
            "parameters": {
                "label": "InvalidLabelXYZ123"
            }
        }))
        .send()
        .await
        .expect("POST /tools/query_graph with invalid label failed");

    // Should return 400 for invalid label
    assert_eq!(resp.status(), 400, "Invalid label should return 400");
}

#[tokio::test]
#[ignore]
async fn test_find_crate_overview_template() {
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
        .expect("POST /tools/query_graph with find_crate_overview failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Should return item_type/cnt data
    assert!(
        body.is_array() || body.is_object(),
        "Response should contain overview data"
    );
}

#[tokio::test]
#[ignore]
async fn test_count_total_nodes_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "count_total_nodes"
        }))
        .send()
        .await
        .expect("POST /tools/query_graph with count_total_nodes failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Should contain a count
    assert!(
        body.is_array() || body.is_object(),
        "Response should contain count data"
    );
}

#[tokio::test]
#[ignore]
async fn test_count_total_relationships_template() {
    let resp = client()
        .post(format!("{BASE}/tools/query_graph"))
        .json(&json!({
            "query_name": "count_total_relationships"
        }))
        .send()
        .await
        .expect("POST /tools/query_graph with count_total_relationships failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Should contain a count
    assert!(
        body.is_array() || body.is_object(),
        "Response should contain count data"
    );
}

// =============================================================================
// Section 7: Workspace Index Metrics
// =============================================================================

/// Helper: Create a test workspace and return its ID.
async fn create_test_workspace() -> String {
    let name = format!("test-metrics-ws-{}", uuid_v4());
    let resp = client()
        .post(format!("{BASE}/workspaces"))
        .json(&json!({
            "github_url": "https://github.com/jarnura/rust-brain.git",
            "name": name
        }))
        .send()
        .await
        .expect("POST /workspaces failed");

    assert!(
        resp.status() == 200 || resp.status() == 201 || resp.status() == 202,
        "create workspace should succeed, got {}",
        resp.status()
    );

    let body: Value = resp.json().await.unwrap();
    body["id"].as_str().unwrap().to_string()
}

/// Helper: Wait for workspace to reach a specific status.
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

/// Helper: Clean up workspace by archiving it.
async fn cleanup_workspace(workspace_id: &str) {
    let _ = client()
        .delete(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await;
}

#[tokio::test]
#[ignore]
async fn test_workspace_index_progress_field() {
    // Create workspace
    let workspace_id = create_test_workspace().await;

    // Wait for it to be ready
    wait_for_workspace_status(&workspace_id, "ready", 60).await;

    // Get workspace details
    let resp = client()
        .get(format!("{BASE}/workspaces/{}", workspace_id))
        .send()
        .await
        .expect("GET /workspaces/:id failed");

    // Cleanup
    cleanup_workspace(&workspace_id).await;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // When ready, index fields should be present
    assert!(
        has_key(&body, "index_started_at"),
        "missing index_started_at"
    );
    assert!(
        has_key(&body, "index_completed_at"),
        "missing index_completed_at"
    );
    assert!(has_key(&body, "index_progress"), "missing index_progress");

    // Verify timestamps are set (not null) when ready
    if body["status"] == "ready" {
        assert!(
            !body["index_started_at"].is_null(),
            "index_started_at should be set"
        );
        assert!(
            !body["index_completed_at"].is_null(),
            "index_completed_at should be set"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_workspace_list_includes_status() {
    // Create a workspace to ensure there's at least one in the list
    let workspace_id = create_test_workspace().await;

    // Wait a bit for workspace to be created
    tokio::time::sleep(Duration::from_secs(2)).await;

    let resp = client()
        .get(format!("{BASE}/workspaces"))
        .send()
        .await
        .expect("GET /workspaces failed");

    // Cleanup
    cleanup_workspace(&workspace_id).await;

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    assert!(body.is_array(), "Response should be an array");

    if let Some(workspaces) = body.as_array() {
        for ws in workspaces {
            assert!(has_key(ws, "id"), "workspace missing id");
            assert!(has_key(ws, "name"), "workspace missing name");
            assert!(has_key(ws, "status"), "workspace missing status");
        }
    }
}
