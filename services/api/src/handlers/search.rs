//! Search and semantic search handlers.
//!
//! Provides `POST /tools/search_semantic` (vector similarity via Qdrant with
//! Postgres keyword fallback) and `POST /tools/aggregate_search` (cross-database
//! enrichment: Qdrant + Postgres + Neo4j).
//!
//! # Notes
//!
//! As of commit `deb108b`, `search_semantic` gracefully falls back to a
//! Postgres keyword search when Ollama is unavailable, instead of returning
//! a hard error.

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{default_limit, CalleeInfo, CallerInfo};
use crate::errors::AppError;
use crate::neo4j::{get_callees_from_neo4j, get_callers_from_neo4j};
use crate::state::AppState;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request body for `POST /tools/search_semantic`.
#[derive(Debug, Deserialize)]
pub struct SearchSemanticRequest {
    /// Natural-language search query
    pub query: String,
    /// Maximum number of results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Minimum similarity score (0.0–1.0) to include a result
    #[serde(default)]
    pub score_threshold: Option<f32>,
    /// Restrict results to a specific crate
    #[serde(default)]
    pub crate_filter: Option<String>,
}

/// Request body for `POST /tools/search_docs`.
#[derive(Debug, Deserialize)]
pub struct SearchDocsRequest {
    /// Natural-language search query
    pub query: String,
    /// Maximum number of results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Minimum similarity score (0.0–1.0) to include a result
    #[serde(default)]
    pub score_threshold: Option<f32>,
}

/// Response for `POST /tools/search_semantic`.
#[derive(Debug, Serialize)]
pub struct SearchSemanticResponse {
    /// Matching code items ranked by similarity
    pub results: Vec<SearchResult>,
    /// Echo of the original query (suffixed with fallback notice if applicable)
    pub query: String,
    /// Number of results returned
    pub total: usize,
}

/// Response for `POST /tools/search_docs`.
#[derive(Debug, Serialize)]
pub struct SearchDocsResponse {
    /// Matching document snippets ranked by similarity
    pub results: Vec<DocResult>,
    /// Echo of the original query
    pub query: String,
    /// Number of results returned
    pub total: usize,
}

/// A single document search hit from Qdrant vector search.
#[derive(Debug, Serialize)]
pub struct DocResult {
    /// Source file path
    pub source_file: String,
    /// Content preview/snippet
    pub content_preview: String,
    /// Similarity score (0.0–1.0)
    pub score: f32,
}

/// A single search hit from Qdrant vector search or keyword fallback.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    /// Fully qualified name
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Item kind (`"function"`, `"struct"`, etc.)
    pub kind: String,
    /// Source file path
    pub file_path: String,
    /// Start line (1-indexed)
    pub start_line: u32,
    /// End line (1-indexed)
    pub end_line: u32,
    /// Similarity score (0.0–1.0 for vector search, decreasing pseudo-score for keyword)
    pub score: f32,
    /// Signature or code snippet
    pub snippet: Option<String>,
    /// Doc comment if available
    pub docstring: Option<String>,
}

/// Request body for `POST /tools/aggregate_search`.
#[derive(Debug, Deserialize)]
pub struct AggregateSearchRequest {
    /// Natural-language search query
    pub query: String,
    /// Maximum number of results (default: 10)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Minimum similarity score
    #[serde(default)]
    pub score_threshold: Option<f32>,
    /// Include caller/callee graph context for each result
    #[serde(default = "super::default_true")]
    pub include_graph: bool,
    /// Include full source body from Postgres
    #[serde(default)]
    pub include_source: bool,
}

/// Response for `POST /tools/aggregate_search`.
#[derive(Debug, Serialize)]
pub struct AggregateSearchResponse {
    /// Echo of the original query
    pub query: String,
    /// Number of results returned
    pub total: usize,
    /// Results enriched with Postgres metadata and Neo4j graph context
    pub results: Vec<AggregatedResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedResult {
    /// Semantic search score from Qdrant
    pub score: f32,
    /// Fully qualified name
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Item kind (function, struct, etc.)
    pub kind: String,
    /// File path from Postgres
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Enriched from Postgres
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub module_path: Option<String>,
    pub crate_name: Option<String>,
    /// Full source body (if requested)
    pub body_source: Option<String>,
    /// Graph context from Neo4j (if requested)
    pub callers: Vec<CallerInfo>,
    pub callees: Vec<CalleeInfo>,
}

// =============================================================================
// Internal Types
// =============================================================================

struct PgEnrichment {
    visibility: Option<String>,
    signature: Option<String>,
    doc_comment: Option<String>,
    file_path: Option<String>,
    module_path: Option<String>,
    crate_name: Option<String>,
    start_line: i32,
    end_line: i32,
    body_source: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PgEnrichmentBase {
    visibility: Option<String>,
    signature: Option<String>,
    doc_comment: Option<String>,
    file_path: Option<String>,
    module_path: Option<String>,
    crate_name: Option<String>,
    start_line: i32,
    end_line: i32,
}

#[derive(sqlx::FromRow)]
struct PgEnrichmentWithBody {
    visibility: Option<String>,
    signature: Option<String>,
    doc_comment: Option<String>,
    file_path: Option<String>,
    module_path: Option<String>,
    crate_name: Option<String>,
    start_line: i32,
    end_line: i32,
    body_source: Option<String>,
}

// =============================================================================
// Handlers
// =============================================================================

/// Searches for code items using natural-language similarity.
///
/// Generates an embedding via Ollama, then performs a vector search against
/// Qdrant. If Ollama is unavailable, falls back to Postgres `ILIKE` keyword
/// matching with a pseudo-score.
///
/// # Errors
///
/// Returns [`AppError::Qdrant`] if the vector search request fails.
/// Returns [`AppError::Database`] if the keyword fallback query fails.
///
/// # Notes
///
/// As of commit `deb108b`, this handler falls back to keyword search when
/// Ollama is down, instead of returning a hard `AppError::Ollama`.
pub async fn search_semantic(
    State(state): State<AppState>,
    Json(req): Json<SearchSemanticRequest>,
) -> Result<Json<SearchSemanticResponse>, AppError> {
    state.metrics.record_request("search_semantic", "POST");
    debug!("Semantic search for: {}", req.query);

    // Try semantic search via Ollama embedding → Qdrant vector search.
    // If Ollama is unavailable, fall back to Postgres keyword search.
    match get_embedding(&state, &req.query).await {
        Ok(embedding) => {
            // Vector search path — full semantic similarity
            let mut search_request = serde_json::json!({
                "vector": embedding,
                "limit": req.limit,
                "with_payload": true,
                "score_threshold": req.score_threshold,
            });

            // Apply crate filter as a Qdrant must-match condition
            if let Some(ref crate_name) = req.crate_filter {
                search_request["filter"] = serde_json::json!({
                    "must": [{
                        "key": "crate_name",
                        "match": { "value": crate_name }
                    }]
                });
            }

            let search_url = format!(
                "{}/collections/{}/points/search",
                state.config.qdrant_host, state.config.collection_name
            );

            let response = state
                .http_client
                .post(&search_url)
                .json(&search_request)
                .send()
                .await
                .map_err(|e| AppError::Qdrant(format!("Failed to search Qdrant: {}", e)))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(AppError::Qdrant(format!(
                    "Qdrant search failed: {} - {}",
                    status, body
                )));
            }

            let search_result: serde_json::Value = response
                .json()
                .await
                .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

            let results = parse_search_results(&search_result);

            Ok(Json(SearchSemanticResponse {
                query: req.query,
                total: results.len(),
                results,
            }))
        }
        Err(ollama_err) => {
            // Fallback: keyword search via Postgres when Ollama is unavailable
            debug!(
                "Ollama unavailable ({}), falling back to keyword search",
                ollama_err
            );
            keyword_search_fallback(&state, &req.query, req.limit, req.crate_filter.as_deref())
                .await
        }
    }
}

/// Keyword-based fallback search via Postgres when Ollama is unavailable.
/// Searches extracted_items by FQN, name, and doc_comment using ILIKE.
async fn keyword_search_fallback(
    state: &AppState,
    query: &str,
    limit: usize,
    crate_filter: Option<&str>,
) -> Result<Json<SearchSemanticResponse>, AppError> {
    let pattern = format!("%{}%", query);
    let limit_i64 = limit as i64;

    let (sql, has_crate_filter) = if crate_filter.is_some() {
        (
            r#"
            SELECT ei.fqn, ei.name, ei.item_type, COALESCE(sf.file_path, ''), ei.start_line, ei.end_line,
                   ei.signature, ei.doc_comment
            FROM extracted_items ei
            LEFT JOIN source_files sf ON ei.source_file_id = sf.id
            WHERE (ei.fqn ILIKE $1
               OR ei.name ILIKE $1
               OR ei.doc_comment ILIKE $1
               OR ei.signature ILIKE $1)
              AND sf.crate_name = $3
            ORDER BY
                CASE WHEN ei.name ILIKE $1 THEN 0 ELSE 1 END,
                CASE WHEN ei.fqn ILIKE $1 THEN 0 ELSE 1 END,
                ei.name
            LIMIT $2
            "#,
            true,
        )
    } else {
        (
            r#"
            SELECT ei.fqn, ei.name, ei.item_type, COALESCE(sf.file_path, ''), ei.start_line, ei.end_line,
                   ei.signature, ei.doc_comment
            FROM extracted_items ei
            LEFT JOIN source_files sf ON ei.source_file_id = sf.id
            WHERE ei.fqn ILIKE $1
               OR ei.name ILIKE $1
               OR ei.doc_comment ILIKE $1
               OR ei.signature ILIKE $1
            ORDER BY
                CASE WHEN ei.name ILIKE $1 THEN 0 ELSE 1 END,
                CASE WHEN ei.fqn ILIKE $1 THEN 0 ELSE 1 END,
                ei.name
            LIMIT $2
            "#,
            false,
        )
    };

    let rows = if has_crate_filter {
        sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                i32,
                i32,
                Option<String>,
                Option<String>,
            ),
        >(sql)
        .bind(&pattern)
        .bind(limit_i64)
        .bind(crate_filter.unwrap())
        .fetch_all(&state.pg_pool)
        .await
        .map_err(|e| AppError::Database(format!("Keyword search failed: {}", e)))?
    } else {
        sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                i32,
                i32,
                Option<String>,
                Option<String>,
            ),
        >(sql)
        .bind(&pattern)
        .bind(limit_i64)
        .fetch_all(&state.pg_pool)
        .await
        .map_err(|e| AppError::Database(format!("Keyword search failed: {}", e)))?
    };

    let results: Vec<SearchResult> = rows
        .into_iter()
        .enumerate()
        .map(
            |(i, (fqn, name, kind, file_path, start_line, end_line, signature, docstring))| {
                SearchResult {
                    fqn,
                    name,
                    kind,
                    file_path,
                    start_line: start_line as u32,
                    end_line: end_line as u32,
                    score: 1.0 - (i as f32 * 0.01), // decreasing pseudo-score for ranking display
                    snippet: signature,
                    docstring,
                }
            },
        )
        .collect();

    let total = results.len();
    Ok(Json(SearchSemanticResponse {
        query: format!("{} (keyword fallback — Ollama unavailable)", query),
        total,
        results,
    }))
}

/// Searches for documentation using natural-language similarity.
///
/// Generates an embedding via Ollama, then performs a vector search against
/// the `doc_embeddings` Qdrant collection. Returns document snippets with
/// source file paths and content previews.
///
/// # Errors
///
/// Returns [`AppError::Ollama`] if embedding generation fails.
/// Returns [`AppError::Qdrant`] if the vector search request fails.
pub async fn search_docs(
    State(state): State<AppState>,
    Json(req): Json<SearchDocsRequest>,
) -> Result<Json<SearchDocsResponse>, AppError> {
    state.metrics.record_request("search_docs", "POST");
    debug!("Doc search for: {}", req.query);

    let embedding = get_embedding(&state, &req.query).await?;

    let search_request = serde_json::json!({
        "vector": embedding,
        "limit": req.limit,
        "with_payload": true,
        "score_threshold": req.score_threshold,
    });

    let search_url = format!(
        "{}/collections/{}/points/search",
        state.config.qdrant_host, state.config.doc_collection_name
    );

    let response = state
        .http_client
        .post(&search_url)
        .json(&search_request)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to search Qdrant: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Qdrant(format!(
            "Qdrant search failed: {} - {}",
            status, body
        )));
    }

    let search_result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

    let results = parse_doc_search_results(&search_result);

    Ok(Json(SearchDocsResponse {
        query: req.query,
        total: results.len(),
        results,
    }))
}

/// Cross-database aggregation: Qdrant (semantic) + Postgres (metadata) + Neo4j (graph).
///
/// 1. Embeds the query via Ollama
/// 2. Performs a vector search in Qdrant
/// 3. Enriches each result with Postgres metadata (visibility, signature, source)
/// 4. Optionally fetches call-graph context from Neo4j
///
/// # Errors
///
/// Returns [`AppError::Ollama`] if embedding generation fails.
/// Returns [`AppError::Qdrant`] if the vector search fails.
/// Postgres and Neo4j enrichment errors are silently ignored per-result.
pub async fn aggregate_search(
    State(state): State<AppState>,
    Json(req): Json<AggregateSearchRequest>,
) -> Result<Json<AggregateSearchResponse>, AppError> {
    state.metrics.record_request("aggregate_search", "POST");
    debug!("Aggregate search for: {}", req.query);

    // Step 1: Get embedding from Ollama
    let embedding = get_embedding(&state, &req.query).await?;

    // Step 2: Search Qdrant for semantic matches
    let search_request = serde_json::json!({
        "vector": embedding,
        "limit": req.limit,
        "with_payload": true,
        "score_threshold": req.score_threshold,
    });

    let search_url = format!(
        "{}/collections/{}/points/search",
        state.config.qdrant_host, state.config.collection_name
    );

    let response = state
        .http_client
        .post(&search_url)
        .json(&search_request)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to search Qdrant: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Qdrant(format!(
            "Qdrant search failed: {} - {}",
            status, body
        )));
    }

    let search_result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

    let qdrant_results = parse_search_results(&search_result);

    // Step 3: Enrich each result with Postgres metadata and Neo4j graph context
    let mut aggregated = Vec::with_capacity(qdrant_results.len());

    for result in &qdrant_results {
        let enriched =
            enrich_search_result(&state, result, req.include_graph, req.include_source).await;
        aggregated.push(enriched);
    }

    Ok(Json(AggregateSearchResponse {
        query: req.query,
        total: aggregated.len(),
        results: aggregated,
    }))
}

// =============================================================================
// Helper Functions
// =============================================================================

async fn get_embedding(state: &AppState, text: &str) -> Result<Vec<f32>, AppError> {
    let request = serde_json::json!({
        "model": state.config.embedding_model,
        "input": text,
    });

    let response = state
        .http_client
        .post(format!("{}/api/embed", state.config.ollama_host))
        .json(&request)
        .send()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to get embedding: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Ollama(format!(
            "Ollama embedding failed: {} - {}",
            status, body
        )));
    }

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to parse Ollama response: {}", e)))?;

    let embedding = result
        .get("embeddings")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::Ollama("No embedding in response".to_string()))?;

    let embedding: Vec<f32> = embedding
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();

    if embedding.is_empty() {
        return Err(AppError::Ollama("Invalid embedding format".to_string()));
    }

    Ok(embedding)
}

/// Parses a Qdrant search response into a list of [`SearchResult`]s.
///
/// Extracts entries from the `result` array, reading `payload` fields for
/// metadata and `score` for similarity. Items missing required fields
/// (`fqn`, `name`, `score`) are silently skipped.
pub fn parse_search_results(search_result: &serde_json::Value) -> Vec<SearchResult> {
    search_result
        .get("result")
        .and_then(|v| v.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    let payload = r.get("payload")?;
                    // Qdrant payload uses "item_type" not "kind"
                    let kind = payload
                        .get("item_type")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("kind").and_then(|v| v.as_str()))
                        .unwrap_or("unknown")
                        .to_string();
                    // file_path may not exist in all payloads
                    let file_path = payload
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // start_line/end_line are stored as i64 in Qdrant
                    let start_line = payload
                        .get("start_line")
                        .and_then(|v| v.as_i64())
                        .or_else(|| {
                            payload
                                .get("start_line")
                                .and_then(|v| v.as_u64())
                                .map(|n| n as i64)
                        })
                        .unwrap_or(0) as u32;
                    let end_line = payload
                        .get("end_line")
                        .and_then(|v| v.as_i64())
                        .or_else(|| {
                            payload
                                .get("end_line")
                                .and_then(|v| v.as_u64())
                                .map(|n| n as i64)
                        })
                        .unwrap_or(0) as u32;
                    Some(SearchResult {
                        fqn: payload.get("fqn")?.as_str()?.to_string(),
                        name: payload.get("name")?.as_str()?.to_string(),
                        kind,
                        file_path,
                        start_line,
                        end_line,
                        score: r.get("score")?.as_f64()? as f32,
                        snippet: payload
                            .get("snippet")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        docstring: payload
                            .get("docstring")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_doc_search_results(search_result: &serde_json::Value) -> Vec<DocResult> {
    search_result
        .get("result")
        .and_then(|v| v.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    let payload = r.get("payload")?;
                    // doc_embeddings uses file_path (not source_file)
                    let source_file = payload
                        .get("file_path")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("source_file").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    // doc_embeddings uses text (not content or content_preview)
                    let content_preview = payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("content").and_then(|v| v.as_str()))
                        .or_else(|| payload.get("content_preview").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                    let preview = if content_preview.len() > 500 {
                        format!("{}...", &content_preview[..500])
                    } else {
                        content_preview
                    };
                    Some(DocResult {
                        source_file,
                        content_preview: preview,
                        score: r.get("score")?.as_f64()? as f32,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Enrich a Qdrant search result with Postgres metadata and Neo4j graph context
async fn enrich_search_result(
    state: &AppState,
    result: &SearchResult,
    include_graph: bool,
    include_source: bool,
) -> AggregatedResult {
    // Query Postgres for full metadata
    let select_body = if include_source {
        ", e.body_source"
    } else {
        ""
    };
    let query_str = format!(
        r#"
        SELECT e.visibility, e.signature, e.doc_comment,
               sf.file_path, sf.module_path, sf.crate_name, e.start_line, e.end_line{}
        FROM extracted_items e
        LEFT JOIN source_files sf ON e.source_file_id = sf.id
        WHERE e.fqn = $1
        "#,
        select_body
    );

    let pg_data: Option<PgEnrichment> = if include_source {
        sqlx::query_as::<_, PgEnrichmentWithBody>(&query_str)
            .bind(&result.fqn)
            .fetch_optional(&state.pg_pool)
            .await
            .ok()
            .flatten()
            .map(|r| PgEnrichment {
                visibility: r.visibility,
                signature: r.signature,
                doc_comment: r.doc_comment,
                file_path: r.file_path,
                module_path: r.module_path,
                crate_name: r.crate_name,
                start_line: r.start_line,
                end_line: r.end_line,
                body_source: r.body_source,
            })
    } else {
        sqlx::query_as::<_, PgEnrichmentBase>(&query_str)
            .bind(&result.fqn)
            .fetch_optional(&state.pg_pool)
            .await
            .ok()
            .flatten()
            .map(|r| PgEnrichment {
                visibility: r.visibility,
                signature: r.signature,
                doc_comment: r.doc_comment,
                file_path: r.file_path,
                module_path: r.module_path,
                crate_name: r.crate_name,
                start_line: r.start_line,
                end_line: r.end_line,
                body_source: None,
            })
    };

    // Get graph context from Neo4j (callers/callees)
    let (callers, callees) = if include_graph {
        let callers = get_callers_from_neo4j(state, &result.fqn, 1)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|c| CallerInfo {
                fqn: c.fqn,
                name: c.name,
                file_path: c.file_path,
                line: c.line,
            })
            .collect();
        let callees = get_callees_from_neo4j(state, &result.fqn)
            .await
            .unwrap_or_default();
        (callers, callees)
    } else {
        (vec![], vec![])
    };

    AggregatedResult {
        score: result.score,
        fqn: result.fqn.clone(),
        name: result.name.clone(),
        kind: result.kind.clone(),
        file_path: pg_data
            .as_ref()
            .and_then(|d| d.file_path.clone())
            .unwrap_or_else(|| result.file_path.clone()),
        start_line: pg_data
            .as_ref()
            .map(|d| d.start_line as u32)
            .unwrap_or(result.start_line),
        end_line: pg_data
            .as_ref()
            .map(|d| d.end_line as u32)
            .unwrap_or(result.end_line),
        visibility: pg_data.as_ref().and_then(|d| d.visibility.clone()),
        signature: pg_data.as_ref().and_then(|d| d.signature.clone()),
        docstring: pg_data
            .as_ref()
            .and_then(|d| d.doc_comment.clone())
            .or_else(|| result.docstring.clone()),
        module_path: pg_data.as_ref().and_then(|d| d.module_path.clone()),
        crate_name: pg_data.as_ref().and_then(|d| d.crate_name.clone()),
        body_source: pg_data.as_ref().and_then(|d| d.body_source.clone()),
        callers,
        callees,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_search_results_empty() {
        let data = serde_json::json!({"result": []});
        let results = parse_search_results(&data);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_search_results_no_result_key() {
        let data = serde_json::json!({});
        let results = parse_search_results(&data);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_search_results_valid() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.95,
                    "payload": {
                        "fqn": "crate::my_fn",
                        "name": "my_fn",
                        "item_type": "function",
                        "file_path": "src/lib.rs",
                        "start_line": 10,
                        "end_line": 20,
                        "snippet": "fn my_fn() {}",
                        "docstring": "A function"
                    }
                }
            ]
        });

        let results = parse_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fqn, "crate::my_fn");
        assert_eq!(results[0].name, "my_fn");
        assert_eq!(results[0].kind, "function");
        assert_eq!(results[0].file_path, "src/lib.rs");
        assert_eq!(results[0].start_line, 10);
        assert_eq!(results[0].end_line, 20);
        assert!((results[0].score - 0.95).abs() < f32::EPSILON);
        assert_eq!(results[0].snippet, Some("fn my_fn() {}".to_string()));
        assert_eq!(results[0].docstring, Some("A function".to_string()));
    }

    #[test]
    fn test_parse_search_results_missing_optional_fields() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.8,
                    "payload": {
                        "fqn": "crate::item",
                        "name": "item",
                        "item_type": "struct"
                    }
                }
            ]
        });

        let results = parse_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, "struct");
        assert_eq!(results[0].file_path, "");
        assert_eq!(results[0].start_line, 0);
        assert!(results[0].snippet.is_none());
        assert!(results[0].docstring.is_none());
    }

    #[test]
    fn test_parse_search_results_skips_invalid_items() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.9,
                    "payload": {
                        // Missing required "fqn" and "name"
                        "kind": "function"
                    }
                },
                {
                    "score": 0.8,
                    "payload": {
                        "fqn": "valid::item",
                        "name": "item",
                        "item_type": "function"
                    }
                }
            ]
        });

        let results = parse_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fqn, "valid::item");
    }

    #[test]
    fn test_search_semantic_request_deserialization() {
        let json = serde_json::json!({
            "query": "find authentication functions",
            "limit": 5,
            "score_threshold": 0.7,
            "crate_filter": "my_crate"
        });

        let req: SearchSemanticRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "find authentication functions");
        assert_eq!(req.limit, 5);
        assert_eq!(req.score_threshold, Some(0.7));
        assert_eq!(req.crate_filter, Some("my_crate".to_string()));
    }

    #[test]
    fn test_search_semantic_request_defaults() {
        let json = serde_json::json!({
            "query": "test"
        });

        let req: SearchSemanticRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.limit, 10); // default_limit
        assert!(req.score_threshold.is_none());
        assert!(req.crate_filter.is_none());
    }

    #[test]
    fn test_aggregate_search_request_deserialization() {
        let json = serde_json::json!({
            "query": "find error handlers",
            "limit": 5,
            "score_threshold": 0.6,
            "include_graph": true,
            "include_source": true
        });

        let req: AggregateSearchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "find error handlers");
        assert_eq!(req.limit, 5);
        assert_eq!(req.score_threshold, Some(0.6));
        assert!(req.include_graph);
        assert!(req.include_source);
    }

    #[test]
    fn test_aggregate_search_request_defaults() {
        let json = serde_json::json!({
            "query": "test"
        });

        let req: AggregateSearchRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.limit, 10);
        assert!(req.score_threshold.is_none());
        assert!(req.include_graph); // defaults to true
        assert!(!req.include_source); // defaults to false
    }

    #[test]
    fn test_aggregated_result_serialization() {
        let result = AggregatedResult {
            score: 0.95,
            fqn: "crate::module::func".to_string(),
            name: "func".to_string(),
            kind: "function".to_string(),
            file_path: "src/lib.rs".to_string(),
            start_line: 10,
            end_line: 20,
            visibility: Some("pub".to_string()),
            signature: Some("pub fn func() -> i32".to_string()),
            docstring: Some("A function".to_string()),
            module_path: Some("crate::module".to_string()),
            crate_name: Some("my_crate".to_string()),
            body_source: None,
            callers: vec![CallerInfo {
                fqn: "crate::caller".to_string(),
                name: "caller".to_string(),
                file_path: "src/main.rs".to_string(),
                line: 5,
            }],
            callees: vec![CalleeInfo {
                fqn: "crate::callee".to_string(),
                name: "callee".to_string(),
            }],
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["fqn"], "crate::module::func");
        assert_eq!(json["visibility"], "pub");
        assert_eq!(json["callers"][0]["fqn"], "crate::caller");
        assert_eq!(json["callees"][0]["name"], "callee");
        assert!(json["body_source"].is_null());
    }

    #[test]
    fn test_aggregated_result_roundtrip() {
        let result = AggregatedResult {
            score: 0.8,
            fqn: "test::item".to_string(),
            name: "item".to_string(),
            kind: "struct".to_string(),
            file_path: "src/types.rs".to_string(),
            start_line: 1,
            end_line: 10,
            visibility: None,
            signature: None,
            docstring: None,
            module_path: None,
            crate_name: None,
            body_source: Some("struct item {}".to_string()),
            callers: vec![],
            callees: vec![],
        };

        let json_str = serde_json::to_string(&result).unwrap();
        let deserialized: AggregatedResult = serde_json::from_str(&json_str).unwrap();
        assert_eq!(deserialized.fqn, "test::item");
        assert_eq!(deserialized.body_source, Some("struct item {}".to_string()));
        assert!(deserialized.callers.is_empty());
    }

    /// Verify that the Qdrant filter JSON matches the expected structure
    /// when crate_filter is applied.
    #[test]
    fn test_qdrant_crate_filter_json_structure() {
        let crate_name = "my_crate";
        let filter = serde_json::json!({
            "must": [{
                "key": "crate_name",
                "match": { "value": crate_name }
            }]
        });

        // Verify structure matches Qdrant's expected filter format
        let must = filter["must"].as_array().unwrap();
        assert_eq!(must.len(), 1);
        assert_eq!(must[0]["key"], "crate_name");
        assert_eq!(must[0]["match"]["value"], "my_crate");
    }

    #[test]
    fn test_search_request_with_crate_filter_builds_filter() {
        // Simulate what the handler does: build a search request then conditionally add filter
        let crate_filter = Some("tokio".to_string());
        let embedding = vec![0.1_f32; 3];

        let mut search_request = serde_json::json!({
            "vector": embedding,
            "limit": 10,
            "with_payload": true,
            "score_threshold": null,
        });

        if let Some(ref crate_name) = crate_filter {
            search_request["filter"] = serde_json::json!({
                "must": [{
                    "key": "crate_name",
                    "match": { "value": crate_name }
                }]
            });
        }

        assert!(search_request.get("filter").is_some());
        assert_eq!(
            search_request["filter"]["must"][0]["match"]["value"],
            "tokio"
        );
    }

    #[test]
    fn test_search_request_without_crate_filter_has_no_filter() {
        let crate_filter: Option<String> = None;
        let embedding = vec![0.1_f32; 3];

        let mut search_request = serde_json::json!({
            "vector": embedding,
            "limit": 10,
            "with_payload": true,
            "score_threshold": null,
        });

        if let Some(ref crate_name) = crate_filter {
            search_request["filter"] = serde_json::json!({
                "must": [{
                    "key": "crate_name",
                    "match": { "value": crate_name }
                }]
            });
        }

        assert!(search_request.get("filter").is_none());
    }

    #[test]
    fn test_search_docs_request_deserialization() {
        let json = serde_json::json!({
            "query": "how to authenticate users",
            "limit": 5,
            "score_threshold": 0.7
        });

        let req: SearchDocsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "how to authenticate users");
        assert_eq!(req.limit, 5);
        assert_eq!(req.score_threshold, Some(0.7));
    }

    #[test]
    fn test_search_docs_request_defaults() {
        let json = serde_json::json!({
            "query": "test"
        });

        let req: SearchDocsRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.limit, 10);
        assert!(req.score_threshold.is_none());
    }

    #[test]
    fn test_parse_doc_search_results_empty() {
        let data = serde_json::json!({"result": []});
        let results = parse_doc_search_results(&data);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_doc_search_results_valid() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.92,
                    "payload": {
                        "source_file": "docs/api/authentication.md",
                        "content": "Authentication is handled via JWT tokens..."
                    }
                }
            ]
        });

        let results = parse_doc_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_file, "docs/api/authentication.md");
        assert_eq!(
            results[0].content_preview,
            "Authentication is handled via JWT tokens..."
        );
        assert!((results[0].score - 0.92).abs() < f32::EPSILON);
    }

    #[test]
    fn test_parse_doc_search_results_qdrant_schema() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.92,
                    "payload": {
                        "file_path": "docs/api/authentication.md",
                        "text": "Authentication is handled via JWT tokens...",
                        "section_title": "Auth",
                        "crate_name": "rust_brain"
                    }
                }
            ]
        });

        let results = parse_doc_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_file, "docs/api/authentication.md");
        assert_eq!(
            results[0].content_preview,
            "Authentication is handled via JWT tokens..."
        );
        assert!((results[0].score - 0.92).abs() < f32::EPSILON);
    }

    #[test]
    fn test_parse_doc_search_results_content_preview_fallback() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.85,
                    "payload": {
                        "source_file": "README.md",
                        "content_preview": "Short preview"
                    }
                }
            ]
        });

        let results = parse_doc_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content_preview, "Short preview");
    }

    #[test]
    fn test_parse_doc_search_results_truncates_long_content() {
        let long_content = "x".repeat(600);
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.9,
                    "payload": {
                        "source_file": "docs/guide.md",
                        "content": long_content.clone()
                    }
                }
            ]
        });

        let results = parse_doc_search_results(&data);
        assert_eq!(results.len(), 1);
        assert!(results[0].content_preview.ends_with("..."));
        assert!(results[0].content_preview.len() <= 503);
    }

    #[test]
    fn test_parse_doc_search_results_missing_fields() {
        let data = serde_json::json!({
            "result": [
                {
                    "score": 0.8,
                    "payload": {}
                }
            ]
        });

        let results = parse_doc_search_results(&data);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_file, "");
        assert_eq!(results[0].content_preview, "");
    }

    #[test]
    fn test_doc_result_serialization() {
        let result = DocResult {
            source_file: "docs/api.md".to_string(),
            content_preview: "API documentation".to_string(),
            score: 0.95,
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["source_file"], "docs/api.md");
        assert_eq!(json["content_preview"], "API documentation");
        let score = json["score"].as_f64().unwrap();
        assert!((score - 0.95).abs() < 1e-6);
    }
}
