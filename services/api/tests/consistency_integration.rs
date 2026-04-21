//! Integration tests for cross-store consistency validation.
//!
//! Tests the consistency checker endpoints (`GET /api/consistency`,
//! `GET /health/consistency`) and cross-store data validation per
//! ADR-004 (`docs/adr/ADR-004-cross-store-consistency.md`).
//!
//! These tests exercise real HTTP against a live server at
//! `http://localhost:8088`.  They require the full docker-compose stack
//! to be running (`bash scripts/start.sh`).
//!
//! Run with:
//! ```
//! cargo test --test consistency_integration -- --include-ignored
//! ```

use reqwest::header::{HeaderMap, AUTHORIZATION};
use reqwest::Client;
use serde_json::Value;

const BASE: &str = "http://localhost:8088";

/// Build a reusable reqwest client with a longer timeout for consistency
/// queries (they hit three stores and may take a few seconds).
fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client")
}

fn default_workspace_id() -> String {
    std::env::var("RUSTBRAIN_TEST_WORKSPACE_ID")
        .unwrap_or_else(|_| "4e863a9c-b3fe-49a0-ace7-255440922c31".to_string())
}

fn authenticated_client() -> Client {
    let builder = Client::builder().timeout(std::time::Duration::from_secs(30));

    let mut headers = reqwest::header::HeaderMap::new();

    if let Ok(key) = std::env::var("RUSTBRAIN_TEST_API_KEY") {
        if !key.is_empty() {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {key}")
                    .parse()
                    .expect("Invalid API key header value"),
            );
        }
    }

    headers.insert(
        "X-Workspace-Id",
        default_workspace_id()
            .parse()
            .expect("Invalid workspace ID header value"),
    );

    builder
        .default_headers(headers)
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
// 1. GET /api/consistency — Contract Tests
// =============================================================================

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_endpoint_returns_200() {
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_response_schema() {
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // Required fields per ConsistencyReport in handlers/consistency.rs
    assert!(body.is_object());
    assert!(has_key(&body, "crate_name"), "missing crate_name field");
    assert!(has_key(&body, "timestamp"), "missing timestamp field");
    assert!(has_key(&body, "store_counts"), "missing store_counts field");
    assert!(has_key(&body, "status"), "missing status field");
    assert!(
        has_key(&body, "recommendation"),
        "missing recommendation field"
    );

    // store_counts must have all three stores
    let counts = &body["store_counts"];
    assert!(
        counts["postgres"].is_number(),
        "store_counts.postgres must be a number"
    );
    assert!(
        counts["neo4j"].is_number(),
        "store_counts.neo4j must be a number"
    );
    assert!(
        counts["qdrant"].is_number(),
        "store_counts.qdrant must be a number"
    );
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_with_crate_filter() {
    let resp = client()
        .get(format!("{BASE}/api/consistency?crate=rustbrain_common"))
        .send()
        .await
        .expect("GET /api/consistency?crate=rustbrain_common failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["crate_name"], "rustbrain_common");
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detail_full_includes_discrepancies() {
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // detail=full should include discrepancies object
    assert!(
        body["discrepancies"].is_object(),
        "detail=full must include discrepancies"
    );
    let disc = &body["discrepancies"];
    assert!(disc["in_postgres_not_neo4j"].is_array());
    assert!(disc["in_postgres_not_qdrant"].is_array());
    assert!(disc["in_neo4j_not_postgres"].is_array());
    assert!(disc["in_qdrant_not_postgres"].is_array());
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detail_summary_omits_discrepancies() {
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=summary"))
        .send()
        .await
        .expect("GET /api/consistency?detail=summary failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // detail=summary should NOT include discrepancies (skip_serializing_if = None)
    assert!(
        body.get("discrepancies").is_none() || body["discrepancies"].is_null(),
        "detail=summary should omit discrepancies"
    );
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_unknown_crate_returns_zero_counts() {
    let resp = authenticated_client()
        .get(format!(
            "{BASE}/api/consistency?crate=nonexistent_crate_xyz_12345"
        ))
        .send()
        .await
        .expect("GET /api/consistency?crate=nonexistent failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["store_counts"]["postgres"], 0);
    assert_eq!(body["store_counts"]["neo4j"], 0);
    assert_eq!(body["store_counts"]["qdrant"], 0);
    assert_eq!(body["status"], "consistent");
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_status_values() {
    // Verify status field only returns "consistent" or "inconsistent"
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let status = body["status"].as_str().unwrap();
    assert!(
        status == "consistent" || status == "inconsistent",
        "unexpected status: {}",
        status
    );
}

// =============================================================================
// 2. GET /health/consistency — Health Check Tests
// =============================================================================

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_returns_valid_status() {
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
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_response_schema() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let body: Value = resp.json().await.unwrap();

    // Required fields per ConsistencyHealthResponse
    assert!(body["status"].is_string());
    assert!(body["total_crates"].is_number());
    assert!(body["inconsistent_crates"].is_number());
    assert!(body["crates"].is_array());
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_200_when_all_consistent() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    // If status is "healthy", response code must be 200
    if body["status"] == "healthy" {
        assert_eq!(status, 200);
        assert_eq!(body["inconsistent_crates"], 0);
    }
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_503_when_inconsistent() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    // If status is "unhealthy", response code must be 503
    if body["status"] == "unhealthy" {
        assert_eq!(status, 503);
        assert!(
            body["inconsistent_crates"].as_u64().unwrap() > 0,
            "unhealthy status requires inconsistent_crates > 0"
        );

        // Each inconsistent crate must be marked as such
        let crates = body["crates"].as_array().unwrap();
        let inconsistent_count = crates.iter().filter(|c| c["consistent"] == false).count();
        assert!(
            inconsistent_count > 0,
            "unhealthy but no crates marked inconsistent"
        );
    }
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_per_crate_schema() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let body: Value = resp.json().await.unwrap();
    let crates = body["crates"].as_array().unwrap();

    for crate_summary in crates {
        assert!(
            crate_summary["crate_name"].is_string(),
            "missing crate_name in crate summary"
        );
        assert!(
            crate_summary["consistent"].is_boolean(),
            "missing consistent boolean in crate summary"
        );
        assert!(
            crate_summary["counts"]["postgres"].is_number(),
            "missing counts.postgres in crate summary"
        );
        assert!(
            crate_summary["counts"]["neo4j"].is_number(),
            "missing counts.neo4j in crate summary"
        );
        assert!(
            crate_summary["counts"]["qdrant"].is_number(),
            "missing counts.qdrant in crate summary"
        );
    }
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_health_consistency_empty_when_no_data() {
    let resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let status = resp.status();
    let body: Value = resp.json().await.unwrap();

    // When no crates are ingested, health should return 200 with empty stats
    if body["total_crates"] == 0 {
        assert_eq!(status, 200);
        assert_eq!(body["status"], "healthy");
        assert!(body["crates"].as_array().unwrap().is_empty());
    }
}

// =============================================================================
// 3. Cross-Store Count Validation (after full ingestion)
// =============================================================================

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_cross_store_counts_match_after_ingestion() {
    // Verify that after a full ingestion run, all three stores have equal
    // counts per ADR-004's consistency definition.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let pg = body["store_counts"]["postgres"].as_u64().unwrap();
    let neo4j = body["store_counts"]["neo4j"].as_u64().unwrap();
    let qdrant = body["store_counts"]["qdrant"].as_u64().unwrap();

    assert_eq!(
        pg, neo4j,
        "Postgres ({}) and Neo4j ({}) counts must match after full ingestion",
        pg, neo4j
    );
    assert_eq!(
        pg, qdrant,
        "Postgres ({}) and Qdrant ({}) counts must match after full ingestion",
        pg, qdrant
    );

    assert_eq!(body["status"], "consistent");
}

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_fqn_sets_match_across_stores() {
    // Using detail=full, verify that FQN sets are identical across all stores.
    // This is the core cross-store consistency guarantee from ADR-004.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    let disc = &body["discrepancies"];
    let in_pg_not_neo4j = disc["in_postgres_not_neo4j"].as_array().unwrap();
    let in_pg_not_qdrant = disc["in_postgres_not_qdrant"].as_array().unwrap();
    let in_neo4j_not_pg = disc["in_neo4j_not_postgres"].as_array().unwrap();
    let in_qdrant_not_pg = disc["in_qdrant_not_postgres"].as_array().unwrap();

    assert!(
        in_pg_not_neo4j.is_empty(),
        "FQNs in Postgres but not Neo4j (Graph stage incomplete): {:?}",
        in_pg_not_neo4j
    );
    assert!(
        in_pg_not_qdrant.is_empty(),
        "FQNs in Postgres but not Qdrant (Embed stage incomplete): {:?}",
        in_pg_not_qdrant
    );
    assert!(
        in_neo4j_not_pg.is_empty(),
        "Orphaned Neo4j nodes (not in Postgres): {:?}",
        in_neo4j_not_pg
    );
    assert!(
        in_qdrant_not_pg.is_empty(),
        "Orphaned Qdrant points (not in Postgres): {:?}",
        in_qdrant_not_pg
    );
}

// =============================================================================
// 4. Per-Crate Consistency Validation
// =============================================================================

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_per_crate_consistency_check() {
    // Test that per-crate consistency checks return correct crate-scoped data.
    // First, get list of crates from health endpoint.
    let health_resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let health: Value = health_resp.json().await.unwrap();
    let crates = health["crates"].as_array();

    if let Some(crate_list) = crates {
        if !crate_list.is_empty() {
            let first_crate = crate_list[0]["crate_name"]
                .as_str()
                .expect("crate_name should be a string");

            let resp = authenticated_client()
                .get(format!(
                    "{BASE}/api/consistency?crate={first_crate}&detail=full"
                ))
                .send()
                .await
                .expect("GET /api/consistency?crate=... failed");

            assert_eq!(resp.status(), 200);
            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["crate_name"], first_crate);
            assert!(
                body["discrepancies"].is_object(),
                "detail=full must include discrepancies for per-crate query"
            );
        }
    }
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_recommendation_is_actionable() {
    // When stores are inconsistent, the recommendation must mention which
    // stage to re-run, per ADR-004's output format.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let recommendation = body["recommendation"].as_str().unwrap();

    if body["status"] == "inconsistent" {
        // Recommendation should mention a stage name or remediation action
        let mentions_action = recommendation.contains("Graph")
            || recommendation.contains("Embed")
            || recommendation.contains("re-run")
            || recommendation.contains("cleanup")
            || recommendation.contains("No data found")
            || recommendation.contains("incomplete");
        assert!(
            mentions_action,
            "inconsistent stores should have actionable recommendation, got: '{}'",
            recommendation
        );
    } else {
        // When consistent, recommendation should confirm no action needed
        assert!(
            recommendation.contains("consistent") || recommendation.contains("No action"),
            "consistent stores should confirm no action needed, got: '{}'",
            recommendation
        );
    }
}

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_per_crate_counts_sum_to_total() {
    // Verify that per-crate counts from /health/consistency sum up to the
    // total counts from /api/consistency?crate=all.
    let health_resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let health: Value = health_resp.json().await.unwrap();
    let empty_crates: Vec<Value> = vec![];
    let crates = health["crates"].as_array().unwrap_or(&empty_crates);

    if !crates.is_empty() {
        let total_resp = client()
            .get(format!("{BASE}/api/consistency"))
            .send()
            .await
            .expect("GET /api/consistency failed");

        let total_body: Value = total_resp.json().await.unwrap();
        let total_pg = total_body["store_counts"]["postgres"].as_u64().unwrap();

        let crate_pg_sum: u64 = crates
            .iter()
            .map(|c| c["counts"]["postgres"].as_u64().unwrap_or(0))
            .sum();

        assert_eq!(
            total_pg, crate_pg_sum,
            "Sum of per-crate Postgres counts ({}) should equal total ({})",
            crate_pg_sum, total_pg
        );
    }
}

// =============================================================================
// 5. Idempotency Verification
// =============================================================================

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_idempotency_consistency_after_reingestion() {
    // After running ingestion with idempotent writes (ON CONFLICT DO UPDATE,
    // MERGE, upsert), the consistency check should still show consistent.
    // This test validates the current state — the actual double-ingestion
    // test is in tests/integration/test_consistency.sh.
    //
    // Per ADR-004: "Re-ingestion must be safe. Idempotent writes already
    // satisfy this."
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    // If data exists, stores should be consistent after idempotent re-ingestion
    if body["store_counts"]["postgres"].as_u64().unwrap() > 0 {
        let pg = body["store_counts"]["postgres"].as_u64().unwrap();
        let neo4j = body["store_counts"]["neo4j"].as_u64().unwrap();
        let qdrant = body["store_counts"]["qdrant"].as_u64().unwrap();

        // All counts must be positive and matching
        assert!(pg > 0, "Postgres should have data after ingestion");
        assert_eq!(
            pg, neo4j,
            "Idempotent re-ingestion should not change Postgres/Neo4j count parity"
        );
        assert_eq!(
            pg, qdrant,
            "Idempotent re-ingestion should not change Postgres/Qdrant count parity"
        );
    }
}

#[tokio::test]
#[ignore = "requires fully populated Qdrant vector store from a complete ingestion run; pass with live stack + snapshot data"]
async fn test_idempotency_no_orphaned_data() {
    // After idempotent writes, there should be no orphaned data in Neo4j
    // or Qdrant (items not in Postgres). This validates that idempotent
    // re-ingestion doesn't create ghost entries.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();

    if let Some(disc) = body.get("discrepancies") {
        let empty_fqns: Vec<Value> = vec![];
        let in_neo4j_not_pg = disc["in_neo4j_not_postgres"]
            .as_array()
            .unwrap_or(&empty_fqns);
        let in_qdrant_not_pg = disc["in_qdrant_not_postgres"]
            .as_array()
            .unwrap_or(&empty_fqns);

        assert!(
            in_neo4j_not_pg.is_empty(),
            "Idempotent re-ingestion should not orphan Neo4j nodes: {:?}",
            in_neo4j_not_pg
        );
        assert!(
            in_qdrant_not_pg.is_empty(),
            "Idempotent re-ingestion should not orphan Qdrant points: {:?}",
            in_qdrant_not_pg
        );
    }
}

// =============================================================================
// 6. Consistency Detection — Missing Store Data
// =============================================================================

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detects_zero_neo4j_as_inconsistent() {
    // If Neo4j count is 0 but Postgres has data, the report should flag
    // "Graph stage has not been run" in the recommendation.
    // This tests the detection logic — actual Neo4j unavailability is
    // tested in tests/integration/test_consistency.sh.
    let health_resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let health: Value = health_resp.json().await.unwrap();
    let empty_crates: Vec<Value> = vec![];
    let crates = health["crates"].as_array().unwrap_or(&empty_crates);

    // Find a crate where Neo4j count is 0 but Postgres has data
    for crate_summary in crates {
        let pg = crate_summary["counts"]["postgres"].as_u64().unwrap_or(0);
        let neo4j = crate_summary["counts"]["neo4j"].as_u64().unwrap_or(0);

        if pg > 0 && neo4j == 0 {
            let name = crate_summary["crate_name"].as_str().unwrap();
            let resp = client()
                .get(format!("{BASE}/api/consistency?crate={name}"))
                .send()
                .await
                .expect("GET /api/consistency?crate=... failed");

            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["status"], "inconsistent");
            assert!(
                body["recommendation"]
                    .as_str()
                    .unwrap()
                    .contains("Graph stage"),
                "Missing Neo4j data should recommend re-running Graph stage"
            );
            // Stop after first match
            return;
        }
    }
    // If no crate has this pattern, the test passes vacuously
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detects_zero_qdrant_as_inconsistent() {
    // If Qdrant count is 0 but Postgres has data, the report should flag
    // "Embed stage has not been run" in the recommendation.
    let health_resp = client()
        .get(format!("{BASE}/health/consistency"))
        .send()
        .await
        .expect("GET /health/consistency failed");

    let health: Value = health_resp.json().await.unwrap();
    let empty_crates: Vec<Value> = vec![];
    let crates = health["crates"].as_array().unwrap_or(&empty_crates);

    // Find a crate where Qdrant count is 0 but Postgres has data
    for crate_summary in crates {
        let pg = crate_summary["counts"]["postgres"].as_u64().unwrap_or(0);
        let qdrant = crate_summary["counts"]["qdrant"].as_u64().unwrap_or(0);

        if pg > 0 && qdrant == 0 {
            let name = crate_summary["crate_name"].as_str().unwrap();
            let resp = client()
                .get(format!("{BASE}/api/consistency?crate={name}"))
                .send()
                .await
                .expect("GET /api/consistency?crate=... failed");

            let body: Value = resp.json().await.unwrap();
            assert_eq!(body["status"], "inconsistent");
            assert!(
                body["recommendation"]
                    .as_str()
                    .unwrap()
                    .contains("Embed stage"),
                "Missing Qdrant data should recommend re-running Embed stage"
            );
            return;
        }
    }
    // If no crate has this pattern, the test passes vacuously
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detects_partial_neo4j_data() {
    // If Neo4j has fewer items than Postgres, the detail=full response
    // should list the missing FQNs in in_postgres_not_neo4j.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    let body: Value = resp.json().await.unwrap();

    if body["status"] == "inconsistent" {
        let disc = &body["discrepancies"];
        let pg_count = body["store_counts"]["postgres"].as_u64().unwrap();
        let neo4j_count = body["store_counts"]["neo4j"].as_u64().unwrap();

        if pg_count > neo4j_count {
            let missing = disc["in_postgres_not_neo4j"].as_array().unwrap();
            assert!(
                !missing.is_empty(),
                "Neo4j count < Postgres count but in_postgres_not_neo4j is empty"
            );
            // Each entry should be a valid FQN string
            for fqn in missing {
                assert!(
                    fqn.as_str().unwrap().contains("::"),
                    "FQN should contain '::', got: {}",
                    fqn
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_detects_partial_qdrant_data() {
    // If Qdrant has fewer items than Postgres, the detail=full response
    // should list the missing FQNs in in_postgres_not_qdrant.
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency?detail=full"))
        .send()
        .await
        .expect("GET /api/consistency?detail=full failed");

    let body: Value = resp.json().await.unwrap();

    if body["status"] == "inconsistent" {
        let disc = &body["discrepancies"];
        let pg_count = body["store_counts"]["postgres"].as_u64().unwrap();
        let qdrant_count = body["store_counts"]["qdrant"].as_u64().unwrap();

        if pg_count > qdrant_count {
            let missing = disc["in_postgres_not_qdrant"].as_array().unwrap();
            assert!(
                !missing.is_empty(),
                "Qdrant count < Postgres count but in_postgres_not_qdrant is empty"
            );
            for fqn in missing {
                assert!(
                    fqn.as_str().unwrap().contains("::"),
                    "FQN should contain '::', got: {}",
                    fqn
                );
            }
        }
    }
}

// =============================================================================
// 7. Timestamp and Metadata Validation
// =============================================================================

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_timestamp_is_iso8601() {
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    let body: Value = resp.json().await.unwrap();
    let timestamp = body["timestamp"].as_str().unwrap();

    // ISO 8601 timestamps should contain 'T' separator
    assert!(
        timestamp.contains('T'),
        "timestamp should be ISO 8601 format, got: {}",
        timestamp
    );
    // Should also contain timezone info (Z or +HH:MM)
    assert!(
        timestamp.contains('Z') || timestamp.contains('+'),
        "timestamp should include timezone, got: {}",
        timestamp
    );
}

#[tokio::test]
#[ignore = "integration test — needs live docker-compose stack; run with: cargo test --test consistency_integration -- --include-ignored"]
async fn test_consistency_all_crates_scope() {
    // When no crate filter is provided, crate_name should be "all"
    let resp = authenticated_client()
        .get(format!("{BASE}/api/consistency"))
        .send()
        .await
        .expect("GET /api/consistency failed");

    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["crate_name"], "all");
}
