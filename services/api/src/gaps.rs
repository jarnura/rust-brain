//! Gap Analysis System for rust-brain
//!
//! Analyzes the current state of code intelligence features,
//! data quality, and provides actionable recommendations.

#![allow(dead_code, clippy::single_match)]

use serde::{Deserialize, Serialize};

// =============================================================================
// Data Structures
// =============================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GapAnalysis {
    pub features: Vec<FeatureStatus>,
    pub data_quality: DataQuality,
    pub known_issues: Vec<KnownIssue>,
    pub recommendations: Vec<Recommendation>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FeatureStatus {
    pub name: String,
    pub status: FeatureState,
    pub description: String,
    pub details: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum FeatureState {
    Working,
    Partial,
    Broken,
    NotImplemented,
}

impl std::fmt::Display for FeatureState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeatureState::Working => write!(f, "working"),
            FeatureState::Partial => write!(f, "partial"),
            FeatureState::Broken => write!(f, "broken"),
            FeatureState::NotImplemented => write!(f, "not_implemented"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DataQuality {
    pub neo4j_nodes: usize,
    pub neo4j_relationships: usize,
    pub qdrant_points: usize,
    pub postgres_items: usize,
    pub has_embeddings: bool,
    pub has_call_graph: bool,
    pub has_trait_impls: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KnownIssue {
    pub id: String,
    pub severity: String, // critical, high, medium, low
    pub title: String,
    pub description: String,
    pub workaround: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Recommendation {
    pub priority: String,
    pub action: String,
    pub impact: String,
}

// =============================================================================
// Gap Analysis Implementation
// =============================================================================

use crate::neo4j::execute_neo4j_query;
use crate::state::AppState;

impl GapAnalysis {
    /// Perform comprehensive gap analysis of the rust-brain system
    pub async fn analyze(state: &AppState) -> Self {
        let mut features = Vec::new();

        // Check each feature
        features.push(check_semantic_search(state, &state.config.collection_name).await);
        features.push(check_get_function(state).await);
        features.push(check_get_callers(state).await);
        features.push(check_trait_impls(state).await);
        features.push(check_find_usages(state).await);
        features.push(check_module_tree(state).await);
        features.push(check_graph_query(state).await);

        // Get data quality metrics
        let data_quality = get_data_quality(state, &state.config.collection_name).await;

        // Get known issues
        let known_issues = get_known_issues();

        // Generate recommendations based on features and data quality
        let recommendations = generate_recommendations(&features, &data_quality);

        GapAnalysis {
            features,
            data_quality,
            known_issues,
            recommendations,
        }
    }
}

// =============================================================================
// Feature Check Functions
// =============================================================================

/// Check semantic search functionality (Qdrant + Ollama embeddings)
async fn check_semantic_search(state: &AppState, collection_name: &str) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check if Qdrant collection exists
    match state
        .http_client
        .get(format!(
            "{}/collections/{}",
            state.config.qdrant_host, collection_name
        ))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(count) = json
                    .get("result")
                    .and_then(|r| r.get("points_count"))
                    .and_then(|c| c.as_u64())
                {
                    if count == 0 {
                        details.push("Qdrant collection exists but has no points".to_string());
                        status = FeatureState::Partial;
                    } else {
                        details.push(format!("Qdrant has {} indexed points", count));
                    }
                }
            }
        }
        Ok(resp) => {
            details.push(format!(
                "Qdrant collection not found (status: {})",
                resp.status()
            ));
            status = FeatureState::Broken;
        }
        Err(e) => {
            details.push(format!("Failed to connect to Qdrant: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check if Ollama embedding model is available
    match state
        .http_client
        .get(format!("{}/api/tags", state.config.ollama_host))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                let models: Vec<&str> = json
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();

                let model_name = &state.config.embedding_model;
                let has_model = models.iter().any(|m| m.contains(model_name));

                if has_model {
                    details.push(format!("Embedding model '{}' is available", model_name));
                } else {
                    details.push(format!(
                        "Embedding model '{}' not found. Available: {:?}",
                        model_name, models
                    ));
                    status = if status == FeatureState::Working {
                        FeatureState::Partial
                    } else {
                        status
                    };
                }
            }
        }
        Ok(_) => {
            details.push("Ollama returned non-success status".to_string());
            status = FeatureState::Partial;
        }
        Err(e) => {
            details.push(format!("Failed to connect to Ollama: {}", e));
            status = FeatureState::Broken;
        }
    }

    FeatureStatus {
        name: "semantic_search".to_string(),
        status,
        description: "Search code using natural language queries via vector embeddings".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check get_function functionality (Postgres lookup + Neo4j callers/callees)
async fn check_get_function(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check Postgres for function data
    match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM extracted_items WHERE item_type IN ('function', 'method')",
    )
    .fetch_one(&state.pg_pool)
    .await
    {
        Ok(count) => {
            if count == 0 {
                details.push("No functions found in Postgres database".to_string());
                status = FeatureState::Partial;
            } else {
                details.push(format!("Postgres has {} function items", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Postgres: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check Neo4j for function nodes
    match execute_neo4j_query(
        state,
        "MATCH (f:Function) RETURN count(f) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Function nodes in Neo4j".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} Function nodes", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Neo4j: {}", e));
            status = if status == FeatureState::Working {
                FeatureState::Partial
            } else {
                status
            };
        }
    }

    FeatureStatus {
        name: "get_function".to_string(),
        status,
        description: "Retrieve detailed information about a specific function".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check get_callers functionality (CALLS relationships in Neo4j)
async fn check_get_callers(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check for CALLS relationships
    match execute_neo4j_query(
        state,
        "MATCH ()-[r:CALLS]->() RETURN count(r) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No CALLS relationships found in Neo4j".to_string());
                status = FeatureState::Partial;
            } else {
                details.push(format!("Neo4j has {} CALLS relationships", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Neo4j: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check for Function nodes (prerequisite for callers)
    match execute_neo4j_query(
        state,
        "MATCH (f:Function) RETURN count(f) as count LIMIT 1",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Function nodes exist (required for caller analysis)".to_string());
                status = FeatureState::Partial;
            }
        }
        Err(_) => {} // Already reported above
    }

    FeatureStatus {
        name: "get_callers".to_string(),
        status,
        description: "Find functions that call a given function (call graph analysis)".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check get_trait_impls functionality (IMPLEMENTS relationships)
async fn check_trait_impls(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check for Trait nodes
    match execute_neo4j_query(
        state,
        "MATCH (t:Trait) RETURN count(t) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Trait nodes found in Neo4j".to_string());
                status = FeatureState::Partial;
            } else {
                details.push(format!("Neo4j has {} Trait nodes", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Neo4j: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check for IMPLEMENTS relationships
    match execute_neo4j_query(
        state,
        "MATCH ()-[r:IMPLEMENTS]->() RETURN count(r) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No IMPLEMENTS relationships found".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} IMPLEMENTS relationships", count));
            }
        }
        Err(_) => {} // Already reported above
    }

    FeatureStatus {
        name: "get_trait_impls".to_string(),
        status,
        description: "Find all implementations of a given trait".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check find_usages functionality (USES_TYPE relationships)
async fn check_find_usages(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check for Type nodes
    match execute_neo4j_query(
        state,
        "MATCH (t:Type) RETURN count(t) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Type nodes found in Neo4j".to_string());
                status = FeatureState::Partial;
            } else {
                details.push(format!("Neo4j has {} Type nodes", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Neo4j: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check for USES_TYPE relationships
    match execute_neo4j_query(
        state,
        "MATCH ()-[r:USES_TYPE]->() RETURN count(r) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No USES_TYPE relationships found".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} USES_TYPE relationships", count));
            }
        }
        Err(_) => {} // Already reported above
    }

    FeatureStatus {
        name: "find_usages".to_string(),
        status,
        description: "Find all usages of a specific type across the codebase".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check module_tree functionality (Module hierarchy in Neo4j)
async fn check_module_tree(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Check for Module nodes
    match execute_neo4j_query(
        state,
        "MATCH (m:Module) RETURN count(m) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Module nodes found in Neo4j".to_string());
                status = FeatureState::Partial;
            } else {
                details.push(format!("Neo4j has {} Module nodes", count));
            }
        }
        Err(e) => {
            details.push(format!("Failed to query Neo4j: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check for Crate nodes
    match execute_neo4j_query(
        state,
        "MATCH (c:Crate) RETURN count(c) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No Crate nodes found".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} Crate nodes", count));
            }
        }
        Err(_) => {} // Already reported above
    }

    // Check for CONTAINS relationships
    match execute_neo4j_query(
        state,
        "MATCH ()-[r:CONTAINS]->() RETURN count(r) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("No CONTAINS relationships (module hierarchy not built)".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} CONTAINS relationships", count));
            }
        }
        Err(_) => {} // Already reported above
    }

    FeatureStatus {
        name: "module_tree".to_string(),
        status,
        description: "Display hierarchical module structure of a crate".to_string(),
        details: Some(details.join("; ")),
    }
}

/// Check graph_query functionality (raw Cypher execution)
async fn check_graph_query(state: &AppState) -> FeatureStatus {
    let mut details = Vec::new();
    let mut status = FeatureState::Working;

    // Test basic Cypher execution
    match execute_neo4j_query(state, "RETURN 1 as test", serde_json::json!({})).await {
        Ok(results) => {
            let test_val = results.first().and_then(|r| r.get("test")?.as_i64());

            if test_val == Some(1) {
                details.push("Neo4j Cypher queries are working".to_string());
            } else {
                details.push("Neo4j returned unexpected result".to_string());
                status = FeatureState::Broken;
            }
        }
        Err(e) => {
            details.push(format!("Failed to execute Cypher query: {}", e));
            status = FeatureState::Broken;
        }
    }

    // Check for any data to query
    match execute_neo4j_query(
        state,
        "MATCH (n) RETURN count(n) as count",
        serde_json::json!({}),
    )
    .await
    {
        Ok(results) => {
            let count = extract_count(&results);

            if count == 0 {
                details.push("Neo4j has no nodes - database is empty".to_string());
                status = if status == FeatureState::Working {
                    FeatureState::Partial
                } else {
                    status
                };
            } else {
                details.push(format!("Neo4j has {} total nodes", count));
            }
        }
        Err(_) => {} // Already reported above
    }

    FeatureStatus {
        name: "graph_query".to_string(),
        status,
        description: "Execute arbitrary read-only Cypher queries".to_string(),
        details: Some(details.join("; ")),
    }
}

// =============================================================================
// Data Quality Assessment
// =============================================================================

/// Helper to extract count from Neo4j row result (row is a JSON object with a "count" key)
fn extract_count(results: &[serde_json::Value]) -> usize {
    results
        .first()
        .and_then(|r| r.get("count")?.as_i64())
        .unwrap_or(0) as usize
}

/// Helper to extract boolean from Neo4j row result (row is a JSON object with a "has" key)
fn extract_bool(results: &[serde_json::Value]) -> bool {
    results
        .first()
        .and_then(|r| r.get("has")?.as_bool())
        .unwrap_or(false)
}

async fn get_data_quality(state: &AppState, code_collection: &str) -> DataQuality {
    // Get Neo4j node count
    let neo4j_nodes = execute_neo4j_query(
        state,
        "MATCH (n) RETURN count(n) as count",
        serde_json::json!({}),
    )
    .await
    .map(|r| extract_count(&r))
    .unwrap_or(0);

    // Get Neo4j relationship count
    let neo4j_relationships = execute_neo4j_query(
        state,
        "MATCH ()-[r]->() RETURN count(r) as count",
        serde_json::json!({}),
    )
    .await
    .map(|r| extract_count(&r))
    .unwrap_or(0);

    // Get Qdrant point count
    let qdrant_points = match state
        .http_client
        .get(format!(
            "{}/collections/{}",
            state.config.qdrant_host, code_collection
        ))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
            Ok(json) => json
                .get("result")
                .and_then(|r| r.get("points_count"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0) as usize,
            Err(_) => 0,
        },
        _ => 0,
    };

    // Get Postgres item count
    let postgres_items = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM extracted_items")
        .fetch_one(&state.pg_pool)
        .await
        .unwrap_or(0) as usize;

    // Check for embeddings (Qdrant has points)
    let has_embeddings = qdrant_points > 0;

    // Check for call graph (CALLS relationships exist)
    let has_call_graph = execute_neo4j_query(
        state,
        "MATCH ()-[r:CALLS]->() RETURN count(r) > 0 as has",
        serde_json::json!({}),
    )
    .await
    .map(|r| extract_bool(&r))
    .unwrap_or(false);

    // Check for trait implementations (IMPLEMENTS relationships exist)
    let has_trait_impls = execute_neo4j_query(
        state,
        "MATCH ()-[r:IMPLEMENTS]->() RETURN count(r) > 0 as has",
        serde_json::json!({}),
    )
    .await
    .map(|r| extract_bool(&r))
    .unwrap_or(false);

    DataQuality {
        neo4j_nodes,
        neo4j_relationships,
        qdrant_points,
        postgres_items,
        has_embeddings,
        has_call_graph,
        has_trait_impls,
    }
}

// =============================================================================
// Known Issues Database
// =============================================================================

fn get_known_issues() -> Vec<KnownIssue> {
    vec![
        KnownIssue {
            id: "GAP-001".to_string(),
            severity: "medium".to_string(),
            title: "USES_TYPE relationships not generated".to_string(),
            description: "Type usage extraction not implemented in pipeline. The indexer does not extract which types are used by functions/methods.".to_string(),
            workaround: Some("Use Graph Query to search function signatures for type names using string matching".to_string()),
        },
        KnownIssue {
            id: "GAP-002".to_string(),
            severity: "low".to_string(),
            title: "Module hierarchy incomplete".to_string(),
            description: "CONTAINS relationships between modules may be missing for deeply nested modules. Only direct parent-child relationships are captured.".to_string(),
            workaround: Some("Parse module paths manually from FQN strings (e.g., 'crate::module::submodule')".to_string()),
        },
        KnownIssue {
            id: "GAP-003".to_string(),
            severity: "medium".to_string(),
            title: "Generic type parameters not tracked".to_string(),
            description: "Generic type parameters in function signatures are not extracted as separate entities. Types like Vec<T> are stored as raw strings.".to_string(),
            workaround: Some("Parse signatures manually or use fuzzy matching for generic types".to_string()),
        },
        KnownIssue {
            id: "GAP-004".to_string(),
            severity: "low".to_string(),
            title: "Doc comments may be truncated".to_string(),
            description: "Long doc comments (>10000 chars) may be truncated in the database due to field size limits.".to_string(),
            workaround: Some("Read source files directly for full documentation".to_string()),
        },
        KnownIssue {
            id: "GAP-005".to_string(),
            severity: "high".to_string(),
            title: "Macro-expanded code not indexed".to_string(),
            description: "Functions generated by macros (like derive macros) are not captured in the index. Only source-level items are tracked.".to_string(),
            workaround: Some("Check the macro definition and manually trace generated code".to_string()),
        },
        KnownIssue {
            id: "GAP-006".to_string(),
            severity: "medium".to_string(),
            title: "Cross-crate calls not fully resolved".to_string(),
            description: "CALLS relationships to external crate functions may use incomplete FQNs if the external crate is not indexed.".to_string(),
            workaround: Some("Use function name matching for cross-crate calls, or index dependent crates".to_string()),
        },
        KnownIssue {
            id: "GAP-007".to_string(),
            severity: "low".to_string(),
            title: "Associated types not tracked".to_string(),
            description: "Associated types from trait implementations (like <T as Iterator>::Item) are not extracted as separate entities.".to_string(),
            workaround: Some("Parse trait implementation signatures manually".to_string()),
        },
        KnownIssue {
            id: "GAP-008".to_string(),
            severity: "medium".to_string(),
            title: "Semantic search limited to functions".to_string(),
            description: "Qdrant embeddings are only generated for functions and methods. Structs, enums, and other items are not semantically searchable.".to_string(),
            workaround: Some("Use graph queries or Postgres full-text search for non-function items".to_string()),
        },
    ]
}

// =============================================================================
// Recommendations Generator
// =============================================================================

fn generate_recommendations(
    features: &[FeatureStatus],
    data_quality: &DataQuality,
) -> Vec<Recommendation> {
    let mut recommendations = Vec::new();

    // Check semantic search status
    let semantic = features.iter().find(|f| f.name == "semantic_search");
    if let Some(f) = semantic {
        if f.status == FeatureState::Broken {
            recommendations.push(Recommendation {
                priority: "high".to_string(),
                action: "Fix Qdrant connection and ensure embedding model is pulled in Ollama".to_string(),
                impact: "Restores semantic search capability - core feature for natural language queries".to_string(),
            });
        } else if f.status == FeatureState::Partial && data_quality.qdrant_points == 0 {
            recommendations.push(Recommendation {
                priority: "high".to_string(),
                action: "Run indexer to populate Qdrant with function embeddings".to_string(),
                impact: "Enables semantic search across all indexed functions".to_string(),
            });
        }
    }

    // Check call graph status
    let callers = features.iter().find(|f| f.name == "get_callers");
    if let Some(f) = callers {
        if f.status == FeatureState::Partial && !data_quality.has_call_graph {
            recommendations.push(Recommendation {
                priority: "high".to_string(),
                action: "Run indexer with --enable-call-graph to generate CALLS relationships"
                    .to_string(),
                impact:
                    "Enables call graph analysis - find callers, callees, and trace execution paths"
                        .to_string(),
            });
        }
    }

    // Check trait impls status
    let trait_impls = features.iter().find(|f| f.name == "get_trait_impls");
    if let Some(f) = trait_impls {
        if f.status == FeatureState::Partial && !data_quality.has_trait_impls {
            recommendations.push(Recommendation {
                priority: "medium".to_string(),
                action: "Ensure indexer is extracting trait implementations (IMPLEMENTS relationships)".to_string(),
                impact: "Enables finding all implementations of a trait - useful for polymorphic code analysis".to_string(),
            });
        }
    }

    // Check find_usages status
    let usages = features.iter().find(|f| f.name == "find_usages");
    if let Some(f) = usages {
        if f.status == FeatureState::Partial {
            recommendations.push(Recommendation {
                priority: "medium".to_string(),
                action: "Implement type usage extraction in indexer pipeline (GAP-001)".to_string(),
                impact: "Enables 'find all usages of type X' queries - essential for refactoring and impact analysis".to_string(),
            });
        }
    }

    // Data quality recommendations
    if data_quality.neo4j_nodes == 0 && data_quality.postgres_items > 0 {
        recommendations.push(Recommendation {
            priority: "high".to_string(),
            action: "Sync Postgres data to Neo4j - run graph sync process".to_string(),
            impact: "Enables all graph-based queries (callers, trait impls, module tree)"
                .to_string(),
        });
    }

    if data_quality.postgres_items == 0 {
        recommendations.push(Recommendation {
            priority: "critical".to_string(),
            action: "Run indexer on target codebase to populate database".to_string(),
            impact: "All features require indexed data - this is the first step".to_string(),
        });
    }

    // Indexing coverage
    if data_quality.neo4j_nodes > 0 && data_quality.neo4j_relationships == 0 {
        recommendations.push(Recommendation {
            priority: "medium".to_string(),
            action: "Run relationship extraction phase of indexer (CALLS, IMPLEMENTS, CONTAINS)"
                .to_string(),
            impact: "Enables relationship-based queries and graph traversal".to_string(),
        });
    }

    // Embedding coverage
    if data_quality.postgres_items > data_quality.qdrant_points * 2 {
        recommendations.push(Recommendation {
            priority: "low".to_string(),
            action: "Re-run embedding generation to ensure all functions have vector embeddings"
                .to_string(),
            impact: "Improves semantic search coverage for recently added code".to_string(),
        });
    }

    // Macro expansion (GAP-005)
    recommendations.push(Recommendation {
        priority: "low".to_string(),
        action: "Consider adding macro expansion analysis for derive macro generated code"
            .to_string(),
        impact: "Would capture auto-generated implementations (Debug, Clone, Serialize, etc.)"
            .to_string(),
    });

    // Extend semantic search (GAP-008)
    if data_quality.has_embeddings {
        recommendations.push(Recommendation {
            priority: "low".to_string(),
            action:
                "Extend embedding generation to structs, enums, and traits (not just functions)"
                    .to_string(),
            impact: "Enables semantic search across all code item types".to_string(),
        });
    }

    recommendations
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature_state_display() {
        assert_eq!(FeatureState::Working.to_string(), "working");
        assert_eq!(FeatureState::Partial.to_string(), "partial");
        assert_eq!(FeatureState::Broken.to_string(), "broken");
        assert_eq!(FeatureState::NotImplemented.to_string(), "not_implemented");
    }

    #[test]
    fn test_known_issues_count() {
        let issues = get_known_issues();
        assert!(
            !issues.is_empty(),
            "Known issues database should not be empty"
        );

        // Check all issues have required fields
        for issue in &issues {
            assert!(!issue.id.is_empty(), "Issue ID should not be empty");
            assert!(!issue.title.is_empty(), "Issue title should not be empty");
            assert!(
                ["critical", "high", "medium", "low"].contains(&issue.severity.as_str()),
                "Severity should be valid: {}",
                issue.severity
            );
        }
    }

    #[test]
    fn test_recommendation_generation() {
        let features = vec![
            FeatureStatus {
                name: "semantic_search".to_string(),
                status: FeatureState::Broken,
                description: "Test".to_string(),
                details: None,
            },
            FeatureStatus {
                name: "get_callers".to_string(),
                status: FeatureState::Partial,
                description: "Test".to_string(),
                details: None,
            },
        ];

        let data_quality = DataQuality {
            neo4j_nodes: 100,
            neo4j_relationships: 0,
            qdrant_points: 0,
            postgres_items: 100,
            has_embeddings: false,
            has_call_graph: false,
            has_trait_impls: false,
        };

        let recommendations = generate_recommendations(&features, &data_quality);
        assert!(
            !recommendations.is_empty(),
            "Should generate recommendations for broken features"
        );
    }

    #[test]
    fn test_extract_count_from_object_row() {
        // execute_neo4j_query returns Vec<serde_json::Value> where each row is an object,
        // e.g. RETURN count(n) as count → [{"count": 42}]
        let rows = vec![serde_json::json!({"count": 42})];
        assert_eq!(extract_count(&rows), 42);

        // Zero rows
        assert_eq!(extract_count(&[]), 0);

        // Row missing "count" key
        let bad_rows = vec![serde_json::json!({"other": 5})];
        assert_eq!(extract_count(&bad_rows), 0);
    }

    #[test]
    fn test_extract_bool_from_object_row() {
        // RETURN count(r) > 0 as has → [{"has": true}]
        let rows_true = vec![serde_json::json!({"has": true})];
        assert!(extract_bool(&rows_true));

        let rows_false = vec![serde_json::json!({"has": false})];
        assert!(!extract_bool(&rows_false));

        // Empty / missing key defaults to false
        assert!(!extract_bool(&[]));
        let bad_rows = vec![serde_json::json!({"other": true})];
        assert!(!extract_bool(&bad_rows));
    }

    #[test]
    fn test_gap_analysis_output_structure() {
        let analysis = GapAnalysis {
            features: vec![
                FeatureStatus {
                    name: "semantic_search".to_string(),
                    status: FeatureState::Working,
                    description: "Semantic search".to_string(),
                    details: Some("Qdrant and Ollama healthy".to_string()),
                },
                FeatureStatus {
                    name: "get_callers".to_string(),
                    status: FeatureState::Partial,
                    description: "Get callers".to_string(),
                    details: None,
                },
            ],
            data_quality: DataQuality {
                neo4j_nodes: 500,
                neo4j_relationships: 300,
                qdrant_points: 200,
                postgres_items: 150,
                has_embeddings: true,
                has_call_graph: true,
                has_trait_impls: false,
            },
            known_issues: get_known_issues(),
            recommendations: generate_recommendations(
                &[FeatureStatus {
                    name: "get_callers".to_string(),
                    status: FeatureState::Partial,
                    description: "Get callers".to_string(),
                    details: None,
                }],
                &DataQuality {
                    neo4j_nodes: 500,
                    neo4j_relationships: 300,
                    qdrant_points: 200,
                    postgres_items: 150,
                    has_embeddings: true,
                    has_call_graph: true,
                    has_trait_impls: false,
                },
            ),
        };

        // Verify structure
        assert_eq!(analysis.features.len(), 2);
        assert_eq!(analysis.data_quality.neo4j_nodes, 500);
        assert!(analysis.data_quality.has_embeddings);
        assert!(!analysis.data_quality.has_trait_impls);
        assert!(!analysis.known_issues.is_empty());

        // Verify JSON serialization round-trip preserves numeric fields as non-zero
        let json = serde_json::to_value(&analysis).expect("should serialize");
        assert_eq!(json["data_quality"]["neo4j_nodes"], 500);
        assert_eq!(json["data_quality"]["neo4j_relationships"], 300);
        assert_eq!(json["data_quality"]["qdrant_points"], 200);
        assert_eq!(json["features"][0]["name"], "semantic_search");
        assert_eq!(json["features"][1]["status"], "Partial");
    }

    #[test]
    fn test_serialization() {
        let analysis = GapAnalysis {
            features: vec![FeatureStatus {
                name: "test".to_string(),
                status: FeatureState::Working,
                description: "Test feature".to_string(),
                details: Some("Details here".to_string()),
            }],
            data_quality: DataQuality {
                neo4j_nodes: 100,
                neo4j_relationships: 50,
                qdrant_points: 200,
                postgres_items: 150,
                has_embeddings: true,
                has_call_graph: true,
                has_trait_impls: false,
            },
            known_issues: vec![KnownIssue {
                id: "TEST-001".to_string(),
                severity: "low".to_string(),
                title: "Test issue".to_string(),
                description: "Test description".to_string(),
                workaround: None,
            }],
            recommendations: vec![Recommendation {
                priority: "high".to_string(),
                action: "Do something".to_string(),
                impact: "Big impact".to_string(),
            }],
        };

        let json = serde_json::to_string(&analysis).expect("Should serialize");
        // Serde serializes enum variants using their name by default
        assert!(
            json.contains("\"status\":\"Working\"") || json.contains("\"status\":\"working\""),
            "Should serialize feature state, got: {}",
            json
        );
        assert!(
            json.contains("\"neo4j_nodes\":100"),
            "Should serialize data quality"
        );
    }
}
