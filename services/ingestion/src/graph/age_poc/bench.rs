//! AGE POC benchmark runner
//!
//! Runs all 10 AGE openCypher queries with timing and produces
//! a summary table of per-query performance.

use anyhow::Result;
use sqlx::PgPool;
use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use super::queries;
use super::{create_age_pool, AgeConfig};

/// Timing result for a single benchmark query.
#[derive(Debug, Clone)]
pub struct QueryTiming {
    pub query_name: String,
    pub duration_ms: u64,
    pub row_count: usize,
    pub success: bool,
    pub error: Option<String>,
}

/// Complete benchmark results for all 10 queries.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub total_time_ms: u64,
    pub query_timings: Vec<QueryTiming>,
    pub timestamp: String,
}

/// Benchmark runner for AGE POC queries.
pub struct BenchmarkRunner {
    pool: PgPool,
    graph_name: String,
    batch_size: usize,
}

impl BenchmarkRunner {
    pub async fn new(config: &AgeConfig) -> Result<Self> {
        let pool = create_age_pool(config).await?;
        Ok(Self {
            pool,
            graph_name: config.graph_name.clone(),
            batch_size: config.batch_size,
        })
    }

    /// Run all 10 benchmark queries and return results.
    pub async fn run_all(&self) -> Result<BenchmarkResult> {
        let overall_start = Instant::now();
        let mut timings = Vec::new();

        let gn = &self.graph_name;

        // Q10: test_connection
        let t = Instant::now();
        match queries::test_connection(&self.pool, gn).await {
            Ok(connected) => timings.push(QueryTiming {
                query_name: "test_connection".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: if connected { 1 } else { 0 },
                success: connected,
                error: if connected {
                    None
                } else {
                    Some("not connected".into())
                },
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "test_connection".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q9: clear_all
        let t = Instant::now();
        match queries::clear_all(&self.pool, gn).await {
            Ok(()) => timings.push(QueryTiming {
                query_name: "clear_all".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "clear_all".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q6: create_indexes
        let t = Instant::now();
        match queries::create_indexes(&self.pool, gn).await {
            Ok(()) => timings.push(QueryTiming {
                query_name: "create_indexes".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 12 * 3,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "create_indexes".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q1: batch_insert_nodes
        let nodes = generate_sample_nodes(self.batch_size, self.batch_size);
        let t = Instant::now();
        match queries::batch_insert_nodes(&self.pool, gn, "Function", &nodes).await {
            Ok(count) => timings.push(QueryTiming {
                query_name: format!("batch_insert_nodes ({})", self.batch_size),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: count,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: format!("batch_insert_nodes ({})", self.batch_size),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q4: merge_node (single)
        let mut struct_props = HashMap::new();
        struct_props.insert("fqn".to_string(), serde_json::json!("bench::MyStruct"));
        struct_props.insert("name".to_string(), serde_json::json!("MyStruct"));
        let t = Instant::now();
        match queries::merge_node(&self.pool, gn, "Struct", "bench::MyStruct", &struct_props).await
        {
            Ok(()) => timings.push(QueryTiming {
                query_name: "merge_node".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 1,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "merge_node".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q2: batch_insert_relationships (MATCH both ends)
        let calls_rels = generate_sample_rels(self.batch_size, self.batch_size.min(100));
        let t = Instant::now();
        match queries::batch_insert_relationships(
            &self.pool,
            gn,
            "Function",
            "Function",
            "CALLS",
            &calls_rels,
        )
        .await
        {
            Ok(count) => timings.push(QueryTiming {
                query_name: format!("batch_insert_relationships ({})", calls_rels.len()),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: count,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: format!("batch_insert_relationships ({})", calls_rels.len()),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q3: batch_insert_rels_merge_target (COALESCE workaround)
        let field_rels = generate_sample_rels(self.batch_size, self.batch_size.min(100));
        let t = Instant::now();
        match queries::batch_insert_rels_merge_target(
            &self.pool,
            gn,
            "Struct",
            "Type",
            "HAS_FIELD",
            &field_rels,
        )
        .await
        {
            Ok(count) => timings.push(QueryTiming {
                query_name: format!("batch_insert_rels_merge_target ({})", field_rels.len()),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: count,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: format!("batch_insert_rels_merge_target ({})", field_rels.len()),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q5: merge_relationship (single)
        let cont_props = HashMap::new();
        let t = Instant::now();
        match queries::merge_relationship(
            &self.pool,
            gn,
            "Function",
            "Struct",
            "CONTAINS",
            "bench::node_0",
            "bench::MyStruct",
            &cont_props,
        )
        .await
        {
            Ok(()) => timings.push(QueryTiming {
                query_name: "merge_relationship".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 1,
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "merge_relationship".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q7: find_node_by_fqn
        let t = Instant::now();
        match queries::find_node_by_fqn(&self.pool, gn, "bench::node_0").await {
            Ok(node) => timings.push(QueryTiming {
                query_name: "find_node_by_fqn".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: if node.is_some() { 1 } else { 0 },
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "find_node_by_fqn".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        // Q8: find_nodes_by_type
        let t = Instant::now();
        match queries::find_nodes_by_type(&self.pool, gn, "Function").await {
            Ok(nodes) => timings.push(QueryTiming {
                query_name: "find_nodes_by_type".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: nodes.len(),
                success: true,
                error: None,
            }),
            Err(e) => timings.push(QueryTiming {
                query_name: "find_nodes_by_type".into(),
                duration_ms: t.elapsed().as_millis() as u64,
                row_count: 0,
                success: false,
                error: Some(e.to_string()),
            }),
        }

        let total_ms = overall_start.elapsed().as_millis() as u64;

        Ok(BenchmarkResult {
            total_time_ms: total_ms,
            query_timings: timings,
            timestamp: format!("{:?}", std::time::SystemTime::now()),
        })
    }
}

fn generate_sample_nodes(_batch_size: usize, count: usize) -> Vec<serde_json::Value> {
    (0..count)
        .map(|i| {
            serde_json::json!({
                "id": format!("bench::node_{}", i),
                "fqn": format!("bench::node_{}", i),
                "name": format!("node_{}", i),
                "props": {
                    "id": format!("bench::node_{}", i),
                    "fqn": format!("bench::node_{}", i),
                    "name": format!("node_{}", i),
                    "visibility": "public",
                    "is_async": i % 3 == 0,
                    "start_line": i * 10,
                    "end_line": i * 10 + 5,
                    "file_path": "bench/lib.rs"
                }
            })
        })
        .collect()
}

fn generate_sample_rels(batch_size: usize, count: usize) -> Vec<serde_json::Value> {
    (0..count)
        .map(|i| {
            serde_json::json!({
                "from_id": format!("bench::node_{}", i % batch_size.max(1)),
                "to_id": format!("bench::type_{}", i),
                "props": {}
            })
        })
        .collect()
}

impl fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{:<35} | {:>10} | {:>6} | Status",
            "Query", "Time (ms)", "Rows"
        )?;
        writeln!(
            f,
            "{}-+-{}-+-{}-+-{}",
            "-".repeat(35),
            "-".repeat(10),
            "-".repeat(6),
            "-".repeat(8)
        )?;

        for t in &self.query_timings {
            let status = if t.success { "OK" } else { "FAIL" };
            writeln!(
                f,
                "{:<35} | {:>10} | {:>6} | {}",
                t.query_name, t.duration_ms, t.row_count, status
            )?;
            if let Some(ref err) = t.error {
                writeln!(f, "  Error: {}", err)?;
            }
        }

        writeln!(
            f,
            "{}-+-{}-+-{}-+-{}",
            "-".repeat(35),
            "-".repeat(10),
            "-".repeat(6),
            "-".repeat(8)
        )?;
        writeln!(f, "Total: {}ms", self.total_time_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::info;

    #[test]
    fn test_query_timing_fields() {
        let t = QueryTiming {
            query_name: "test".into(),
            duration_ms: 42,
            row_count: 10,
            success: true,
            error: None,
        };
        assert_eq!(t.query_name, "test");
        assert_eq!(t.duration_ms, 42);
        assert!(t.success);
    }

    #[test]
    fn test_benchmark_result_display() {
        let result = BenchmarkResult {
            total_time_ms: 1234,
            query_timings: vec![
                QueryTiming {
                    query_name: "test_connection".into(),
                    duration_ms: 2,
                    row_count: 1,
                    success: true,
                    error: None,
                },
                QueryTiming {
                    query_name: "batch_insert_nodes (100)".into(),
                    duration_ms: 150,
                    row_count: 100,
                    success: true,
                    error: None,
                },
            ],
            timestamp: "2026-04-17T12:00:00Z".into(),
        };

        let output = format!("{}", result);
        assert!(output.contains("test_connection"));
        assert!(output.contains("batch_insert_nodes"));
        assert!(output.contains("Total: 1234ms"));
        assert!(output.contains("OK"));
    }

    #[test]
    fn test_generate_sample_nodes() {
        let batch_size = 100;
        let nodes = generate_sample_nodes(batch_size, 5);
        assert_eq!(nodes.len(), 5);

        let first = &nodes[0];
        assert_eq!(first["id"], "bench::node_0");
        assert_eq!(first["name"], "node_0");
    }

    #[tokio::test]
    #[ignore] // Requires AGE-enabled PostgreSQL
    async fn test_benchmark_runner() {
        let config = AgeConfig::default();
        let runner = BenchmarkRunner::new(&config).await.unwrap();
        let result = runner.run_all().await.unwrap();
        assert!(!result.query_timings.is_empty());
        assert!(result.total_time_ms > 0);
        info!("Benchmark results:\n{}", result);
    }
}
