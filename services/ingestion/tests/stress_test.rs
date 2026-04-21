//! Integration test: 500K+ LOC ingestion stress test
//!
//! Validates that the ingestion pipeline can handle production-scale Rust
//! codebases without OOM, crashes, or data corruption.
//!
//! Prerequisites:
//!   - Docker stack running (Postgres, Neo4j, Qdrant, Ollama)
//!   - Stress workspace generated via `tools/stress-gen/`
//!   - Ingestion completed: `./scripts/stress-test-ingestion.sh`
//!
//! Run with:
//!   cargo test --test stress_test -- --include-ignored --nocapture
//!
//! Environment variables:
//!   STRESS_WORKSPACE_PATH  - Path to the 500K+ LOC workspace (default: ./target/stress-workspace)
//!   DATABASE_URL           - Postgres connection URL
//!   NEO4J_HTTP_URL         - Neo4j HTTP URL (default: http://localhost:7474)
//!   NEO4J_PASSWORD         - Neo4j password

use reqwest::header::{HeaderMap, AUTHORIZATION};
use reqwest::Client;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn stress_workspace_path() -> PathBuf {
    std::env::var("STRESS_WORKSPACE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./target/stress-workspace"))
}

fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://rustbrain:rustbrain@localhost:5432/rustbrain".to_string())
}

fn neo4j_http_url() -> String {
    std::env::var("NEO4J_HTTP_URL").unwrap_or_else(|_| "http://localhost:7474".to_string())
}

fn neo4j_password() -> String {
    std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "rustbrain_dev_2024".to_string())
}

fn api_url() -> String {
    std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:8088".to_string())
}

/// Build a reqwest client with `Authorization: Bearer <key>` default header.
///
/// Reads the API key from `RUSTBRAIN_TEST_API_KEY` env var.  If not set,
/// returns a plain client (works when `RUSTBRAIN_AUTH_DISABLED=true`).
fn authenticated_client() -> Client {
    let builder = Client::builder().timeout(std::time::Duration::from_secs(30));

    match std::env::var("RUSTBRAIN_TEST_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {key}")
                    .parse()
                    .expect("Invalid API key header value"),
            );
            builder
                .default_headers(headers)
                .build()
                .expect("Failed to build HTTP client")
        }
        _ => builder.build().expect("Failed to build HTTP client"),
    }
}

fn qdrant_url() -> String {
    std::env::var("QDRANT_HOST").unwrap_or_else(|_| "http://localhost:6333".to_string())
}

/// Count lines of Rust source in a directory tree.
fn count_rust_loc(dir: &std::path::Path) -> usize {
    let mut total = 0usize;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                total += count_rust_loc(&path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    total += content.lines().count();
                }
            }
        }
    }
    total
}

/// Query Postgres for extracted item counts by type.
async fn get_postgres_counts(db_url: &str) -> HashMap<String, usize> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(db_url)
        .await
        .expect("Failed to connect to Postgres");

    let mut counts = HashMap::new();

    // Total items
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM extracted_items")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    counts.insert("total".to_string(), total as usize);

    // Per-type counts
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT item_type, COUNT(*) as cnt FROM extracted_items GROUP BY item_type ORDER BY cnt DESC",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    for (item_type, count) in rows {
        counts.insert(item_type, count as usize);
    }

    // Source files count
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM source_files")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    counts.insert("source_files".to_string(), files as usize);

    pool.close().await;
    counts
}

/// Execute a Cypher query via Neo4j HTTP API and return the raw JSON response.
async fn neo4j_query(cypher: &str) -> serde_json::Value {
    let url = format!("{}/db/neo4j/tx/commit", neo4j_http_url());
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "statements": [{"statement": cypher}]
    });

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .basic_auth("neo4j", Some(&neo4j_password()))
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(response) if response.status().is_success() => response
            .json::<serde_json::Value>()
            .await
            .unwrap_or(serde_json::json!({})),
        Ok(response) => {
            eprintln!(
                "Neo4j HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            );
            serde_json::json!({})
        }
        Err(e) => {
            eprintln!("Neo4j HTTP error: {}", e);
            serde_json::json!({})
        }
    }
}

/// Extract a single integer value from a Neo4j transactional HTTP response.
fn extract_neo4j_count(json: &serde_json::Value, index: usize) -> i64 {
    json.pointer(&format!("/results/{}/data/0/row/0", index))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// Query Qdrant for vector counts.
async fn get_qdrant_counts(qdrant_url: &str) -> HashMap<String, usize> {
    let client = reqwest::Client::new();
    let mut counts = HashMap::new();

    for collection in &["code_embeddings", "doc_embeddings"] {
        let url = format!("{}/collections/{}", qdrant_url, collection);
        if let Ok(resp) = client.get(&url).send().await {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                let points = json
                    .pointer("/result/points_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                counts.insert(collection.to_string(), points);
            }
        }
    }

    counts
}

// ---------------------------------------------------------------------------
// Stress tests
// ---------------------------------------------------------------------------

#[test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
fn stress_workspace_exists_and_has_500k_loc() {
    let path = stress_workspace_path();
    assert!(
        path.exists(),
        "Stress workspace not found at {:?}. Generate it with: cd tools/stress-gen && cargo run -- --loc 500000",
        path
    );

    assert!(
        path.join("Cargo.toml").exists(),
        "Workspace Cargo.toml missing at {:?}",
        path
    );

    let loc = count_rust_loc(&path);
    println!("Stress workspace LOC: {}", loc);
    assert!(
        loc >= 500_000,
        "Stress workspace has only {} LOC, need >= 500000",
        loc
    );
}

#[test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
fn stress_workspace_compiles() {
    let path = stress_workspace_path();
    let status = std::process::Command::new("cargo")
        .args(["check", "--workspace"])
        .current_dir(&path)
        .env("RUSTFLAGS", "-Awarnings")
        .status()
        .expect("Failed to run cargo check");

    assert!(
        status.success(),
        "Stress workspace failed to compile with cargo check"
    );
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_ingestion_produces_items_in_postgres() {
    let db_url = database_url();
    let counts = get_postgres_counts(&db_url).await;

    let total = counts.get("total").copied().unwrap_or(0);
    println!("Postgres extracted_items: {:?}", counts);

    // A 500K+ LOC workspace should produce at least 10K extracted items
    // (conservative estimate: ~50 LOC per item on average)
    assert!(
        total >= 10_000,
        "Expected >= 10000 extracted items in Postgres, got {}. Counts: {:?}",
        total,
        counts
    );

    // Must have at least some of each major item type
    let required_types = ["function", "struct", "enum", "trait", "impl"];
    for item_type in &required_types {
        let count = counts.get(*item_type).copied().unwrap_or(0);
        assert!(
            count > 0,
            "No {} items found in Postgres. Item is required for pipeline validation",
            item_type
        );
    }
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_ingestion_builds_neo4j_graph() {
    let json = neo4j_query("MATCH (n) RETURN count(n) as nodes_total").await;

    let total_nodes = extract_neo4j_count(&json, 0);
    println!("Neo4j total nodes: {}", total_nodes);

    assert!(
        total_nodes >= 5_000,
        "Expected >= 5000 Neo4j nodes, got {}",
        total_nodes
    );

    // Count edges
    let json = neo4j_query("MATCH ()-[r]->() RETURN count(r) as edges_total").await;
    let total_edges = extract_neo4j_count(&json, 0);
    println!("Neo4j total edges: {}", total_edges);

    assert!(
        total_edges >= 1_000,
        "Expected >= 1000 Neo4j edges, got {}",
        total_edges
    );
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_ingestion_creates_qdrant_embeddings() {
    let counts = get_qdrant_counts(&qdrant_url()).await;
    println!("Qdrant counts: {:?}", counts);

    let code_embeddings = counts.get("code_embeddings").copied().unwrap_or(0);

    assert!(
        code_embeddings >= 5_000,
        "Expected >= 5000 code embeddings in Qdrant, got {}",
        code_embeddings
    );
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_cross_store_consistency() {
    // Get Postgres counts
    let pg_counts = get_postgres_counts(&database_url()).await;
    let pg_total = pg_counts.get("total").copied().unwrap_or(0);

    // Get Neo4j node count
    let neo4j_json = neo4j_query("MATCH (n) RETURN count(n)").await;
    let neo4j_nodes = extract_neo4j_count(&neo4j_json, 0) as usize;

    // Get Qdrant embeddings count
    let qdrant_counts = get_qdrant_counts(&qdrant_url()).await;
    let code_embeddings = qdrant_counts.get("code_embeddings").copied().unwrap_or(0);

    println!("Cross-store consistency check:");
    println!("  Postgres items:  {}", pg_total);
    println!("  Neo4j nodes:     {}", neo4j_nodes);
    println!("  Qdrant vectors:  {}", code_embeddings);

    if pg_total > 0 {
        // Neo4j should have 30-300% of Postgres items
        let ratio = (neo4j_nodes as f64 / pg_total as f64) * 100.0;
        assert!(
            (30.0..=300.0).contains(&ratio),
            "Neo4j/Postgres ratio out of range: {:.1}% (nodes={}, items={}). Possible data inconsistency.",
            ratio, neo4j_nodes, pg_total
        );

        // Qdrant code_embeddings should be within 40-200% of Postgres items
        let embed_ratio = (code_embeddings as f64 / pg_total as f64) * 100.0;
        assert!(
            (40.0..=200.0).contains(&embed_ratio),
            "Qdrant/Postgres ratio out of range: {:.1}% (vectors={}, items={}). Possible embedding failure.",
            embed_ratio, code_embeddings, pg_total
        );
    }
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_fqn_uniqueness_in_postgres() {
    let db_url = database_url();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_url)
        .await
        .expect("Failed to connect to Postgres");

    // Check for duplicate FQNs (should be zero or very few due to upsert)
    let duplicate_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
            SELECT fqn, COUNT(*) as cnt 
            FROM extracted_items 
            GROUP BY fqn 
            HAVING COUNT(*) > 1
        ) dupes",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or(0);

    let total_items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM extracted_items")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    println!(
        "FQN uniqueness: {} total items, {} duplicate FQNs",
        total_items, duplicate_count
    );

    // Allow up to 1% duplicates (some macro-generated items may overlap)
    let max_dupes = (total_items as f64 * 0.01).ceil() as i64;
    assert!(
        duplicate_count <= max_dupes,
        "Too many duplicate FQNs: {} (max allowed: {} for {} items). Possible ingestion deduplication bug.",
        duplicate_count, max_dupes, total_items
    );

    pool.close().await;
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_neo4j_edge_types_exist() {
    // Batch query: get counts for CALLS, IMPLEMENTS, CONTAINS edges
    let json = neo4j_query(
        "MATCH ()-[c:CALLS]->() RETURN count(c) \
         UNION ALL \
         MATCH ()-[i:IMPLEMENTS]->() RETURN count(i) \
         UNION ALL \
         MATCH ()-[co:CONTAINS]->() RETURN count(co)",
    )
    .await;

    let calls_count = extract_neo4j_count(&json, 0);
    let implements_count = extract_neo4j_count(&json, 1);
    let contains_count = extract_neo4j_count(&json, 2);

    println!("Neo4j edge types:");
    println!("  CALLS:       {}", calls_count);
    println!("  IMPLEMENTS:  {}", implements_count);
    println!("  CONTAINS:    {}", contains_count);

    assert!(
        calls_count > 0,
        "No CALLS edges found in Neo4j. The graph stage may not be building call relationships correctly."
    );

    assert!(
        implements_count > 0,
        "No IMPLEMENTS edges found in Neo4j. The graph stage may not be building trait implementation relationships correctly."
    );

    assert!(
        contains_count > 0,
        "No CONTAINS edges found in Neo4j. The graph stage may not be building module hierarchy correctly."
    );
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_semantic_search_returns_results() {
    let url = api_url();
    let client = authenticated_client();

    let queries = vec![
        "process request",
        "handle error",
        "create new instance",
        "validate input",
        "transform data",
    ];

    let mut success_count = 0;
    let mut total_results = 0;

    for query in &queries {
        let resp = client
            .post(format!("{}/tools/search_semantic", url))
            .json(&serde_json::json!({
                "query": query,
                "limit": 5
            }))
            .send()
            .await;

        match resp {
            Ok(response) if response.status().is_success() => {
                if let Ok(body) = response.json::<serde_json::Value>().await {
                    if let Some(results) = body.get("results").and_then(|r| r.as_array()) {
                        let count = results.len();
                        total_results += count;
                        if count > 0 {
                            success_count += 1;
                        }
                        println!("  search_semantic(\"{}\") → {} results", query, count);
                    }
                }
            }
            Ok(response) => {
                println!(
                    "  search_semantic(\"{}\") → HTTP {}",
                    query,
                    response.status()
                );
            }
            Err(e) => {
                println!("  search_semantic(\"{}\") → error: {}", query, e);
            }
        }
    }

    println!(
        "Semantic search: {}/{} queries returned results (total: {} results)",
        success_count,
        queries.len(),
        total_results
    );

    let min_success = (queries.len() as f64 * 0.6).ceil() as usize;
    assert!(
        success_count >= min_success,
        "Only {}/{} semantic search queries returned results (need >= {}). Embedding quality may be poor.",
        success_count,
        queries.len(),
        min_success
    );
}

#[tokio::test]
#[ignore = "stress test — requires full docker-compose stack with 32GB RAM; run manually with: cargo test --test stress_test -- --include-ignored"]
async fn stress_query_graph_security_contract() {
    let url = api_url();
    let client = authenticated_client();

    // Verify query_graph rejects write operations
    let write_queries = vec![
        "CREATE (n:Test {name: 'stress'})",
        "MATCH (n) DETACH DELETE n",
        "MERGE (n:Stress {val: 1})",
    ];

    for cypher in &write_queries {
        let resp = client
            .post(format!("{}/tools/query_graph", url))
            .json(&serde_json::json!({
                "query": cypher
            }))
            .send()
            .await;

        match resp {
            Ok(response) => {
                let status = response.status();
                assert!(
                    status.as_u16() >= 400,
                    "query_graph accepted write query '{}'. Got HTTP {}. Security contract violated.",
                    cypher, status
                );
                println!("  query_graph rejected '{}': HTTP {}", cypher, status);
            }
            Err(e) => {
                println!("  query_graph error for '{}': {}", cypher, e);
            }
        }
    }

    // Verify read queries are accepted
    let read_resp = client
        .post(format!("{}/tools/query_graph", url))
        .json(&serde_json::json!({
            "query": "MATCH (n:Function) RETURN n.fqn LIMIT 5"
        }))
        .send()
        .await;

    match read_resp {
        Ok(response) => {
            let status = response.status();
            assert!(
                status.is_success(),
                "query_graph rejected read query. Got HTTP {}. Should accept MATCH queries.",
                status
            );
            println!("  query_graph accepted read query: HTTP {}", status);
        }
        Err(e) => {
            println!("  query_graph read query error: {}", e);
        }
    }
}
