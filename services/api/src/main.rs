//! rust-brain Tool API Server
//!
//! Provides REST endpoints for code intelligence queries.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use neo4rs::Graph;
use prometheus::{Encoder, Registry, TextEncoder};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info};

// =============================================================================
// Playground - serve embedded HTML
// =============================================================================

async fn playground_html() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("Content-Type", "text/html; charset=utf-8")],
        include_str!("../static/playground.html"),
    )
}

// =============================================================================
// Configuration
// =============================================================================

#[derive(Debug, Clone)]
struct Config {
    database_url: String,
    neo4j_uri: String,
    neo4j_user: String,
    neo4j_password: String,
    qdrant_host: String,
    ollama_host: String,
    embedding_model: String,
    embedding_dimensions: usize,
    collection_name: String,
    port: u16,
}

impl Config {
    fn from_env() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgresql://rustbrain:rustbrain_dev_2024@postgres:5432/rustbrain".to_string()),
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "bolt://neo4j:7687".to_string()),
            neo4j_user: std::env::var("NEO4J_USER")
                .unwrap_or_else(|_| "neo4j".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD")
                .unwrap_or_else(|_| "rustbrain_dev_2024".to_string()),
            qdrant_host: std::env::var("QDRANT_HOST")
                .unwrap_or_else(|_| "http://qdrant:6333".to_string()),
            ollama_host: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://ollama:11434".to_string()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            embedding_dimensions: std::env::var("EMBEDDING_DIMENSIONS")
                .map(|s| s.parse().unwrap_or(768))
                .unwrap_or(768),
            collection_name: std::env::var("QDRANT_COLLECTION")
                .unwrap_or_else(|_| "rust_functions".to_string()),
            port: std::env::var("API_PORT")
                .map(|s| s.parse().unwrap_or(8080))
                .unwrap_or(8080),
        }
    }
}

// =============================================================================
// Application State
// =============================================================================

#[derive(Clone)]
struct AppState {
    config: Config,
    pg_pool: sqlx::postgres::PgPool,
    neo4j_graph: Arc<Graph>,
    http_client: reqwest::Client,
    metrics: Arc<Metrics>,
}

// =============================================================================
// Metrics
// =============================================================================

struct Metrics {
    registry: Registry,
    requests_total: prometheus::CounterVec,
    request_duration: prometheus::HistogramVec,
    errors_total: prometheus::CounterVec,
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new();
        
        let requests_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_requests_total", "Total API requests"),
            &["endpoint", "method"],
        ).expect("Failed to create requests_total metric");
        
        let request_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new("rustbrain_api_request_duration_seconds", "Request duration"),
            &["endpoint"],
        ).expect("Failed to create request_duration metric");
        
        let errors_total = prometheus::CounterVec::new(
            prometheus::Opts::new("rustbrain_api_errors_total", "Total API errors"),
            &["endpoint", "error_code"],
        ).expect("Failed to create errors_total metric");
        
        registry.register(Box::new(requests_total.clone())).expect("Failed to register requests_total");
        registry.register(Box::new(request_duration.clone())).expect("Failed to register request_duration");
        registry.register(Box::new(errors_total.clone())).expect("Failed to register errors_total");
        
        Self {
            registry,
            requests_total,
            request_duration,
            errors_total,
        }
    }
    
    fn record_request(&self, endpoint: &str, method: &str) {
        self.requests_total.with_label_values(&[endpoint, method]).inc();
    }
    
    fn record_error(&self, endpoint: &str, error_code: &str) {
        self.errors_total.with_label_values(&[endpoint, error_code]).inc();
    }
}

// =============================================================================
// Error Handling
// =============================================================================

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
    code: String,
}

#[derive(Debug)]
enum AppError {
    Database(String),
    Neo4j(String),
    Qdrant(String),
    Ollama(String),
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(msg) => write!(f, "Database error: {}", msg),
            AppError::Neo4j(msg) => write!(f, "Neo4j error: {}", msg),
            AppError::Qdrant(msg) => write!(f, "Qdrant error: {}", msg),
            AppError::Ollama(msg) => write!(f, "Ollama error: {}", msg),
            AppError::NotFound(msg) => write!(f, "Not found: {}", msg),
            AppError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            AppError::Internal(msg) => write!(f, "Internal error: {}", msg),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error, code) = match self {
            AppError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "DATABASE_ERROR"),
            AppError::Neo4j(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "NEO4J_ERROR"),
            AppError::Qdrant(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "QDRANT_ERROR"),
            AppError::Ollama(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "OLLAMA_ERROR"),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg, "NOT_FOUND"),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg, "BAD_REQUEST"),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "INTERNAL_ERROR"),
        };
        
        let body = Json(ApiError {
            error: error.to_string(),
            code: code.to_string(),
        });
        
        (status, body).into_response()
    }
}

// =============================================================================
// Request/Response Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct SearchSemanticRequest {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    score_threshold: Option<f32>,
    #[serde(default)]
    crate_filter: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchSemanticResponse {
    results: Vec<SearchResult>,
    query: String,
    total: usize,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    fqn: String,
    name: String,
    kind: String,
    file_path: String,
    start_line: u32,
    end_line: u32,
    score: f32,
    snippet: Option<String>,
    docstring: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GetFunctionQuery {
    fqn: String,
}

#[derive(Debug, Serialize)]
struct FunctionDetail {
    fqn: String,
    name: String,
    kind: String,
    visibility: Option<String>,
    signature: Option<String>,
    docstring: Option<String>,
    file_path: String,
    start_line: u32,
    end_line: u32,
    module_path: Option<String>,
    crate_name: Option<String>,
    callers: Vec<CallerInfo>,
    callees: Vec<CalleeInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallerInfo {
    fqn: String,
    name: String,
    file_path: String,
    line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CalleeInfo {
    fqn: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct GetCallersQuery {
    fqn: String,
    #[serde(default = "default_depth")]
    depth: usize,
}

#[derive(Debug, Serialize)]
struct CallersResponse {
    fqn: String,
    callers: Vec<CallerNode>,
    depth: usize,
}

#[derive(Debug, Serialize)]
struct CallerNode {
    fqn: String,
    name: String,
    file_path: String,
    line: u32,
    depth: usize,
}

#[derive(Debug, Deserialize)]
struct GetTraitImplsQuery {
    trait_name: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Serialize)]
struct TraitImplsResponse {
    trait_name: String,
    implementations: Vec<TraitImpl>,
}

#[derive(Debug, Serialize)]
struct TraitImpl {
    impl_fqn: String,
    type_name: String,
    file_path: String,
    start_line: u32,
}

#[derive(Debug, Deserialize)]
struct FindUsagesOfTypeQuery {
    type_name: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Serialize)]
struct UsagesResponse {
    type_name: String,
    usages: Vec<TypeUsage>,
}

#[derive(Debug, Serialize)]
struct TypeUsage {
    fqn: String,
    name: String,
    kind: String,
    file_path: String,
    line: u32,
}

#[derive(Debug, Deserialize)]
struct GetModuleTreeQuery {
    crate_name: String,
}

#[derive(Debug, Serialize)]
struct ModuleTreeResponse {
    crate_name: String,
    root: ModuleNode,
}

#[derive(Debug, Serialize)]
struct ModuleNode {
    name: String,
    path: String,
    children: Vec<ModuleNode>,
    items: Vec<ModuleItem>,
}

#[derive(Debug, Serialize)]
struct ModuleItem {
    name: String,
    kind: String,
    visibility: String,
}

#[derive(Debug, Deserialize)]
struct QueryGraphRequest {
    query: String,
    #[serde(default)]
    parameters: HashMap<String, serde_json::Value>,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Serialize)]
struct GraphQueryResponse {
    results: Vec<serde_json::Value>,
    query: String,
    row_count: usize,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: String,
    timestamp: String,
    version: String,
    dependencies: HashMap<String, DependencyStatus>,
}

#[derive(Debug, Serialize)]
struct DependencyStatus {
    status: String,
    latency_ms: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AggregateSearchRequest {
    query: String,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    score_threshold: Option<f32>,
    /// Include caller/callee graph context for each result
    #[serde(default = "default_true")]
    include_graph: bool,
    /// Include full source body from Postgres
    #[serde(default)]
    include_source: bool,
}

#[derive(Debug, Serialize)]
struct AggregateSearchResponse {
    query: String,
    total: usize,
    results: Vec<AggregatedResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AggregatedResult {
    /// Semantic search score from Qdrant
    score: f32,
    /// Fully qualified name
    fqn: String,
    /// Short name
    name: String,
    /// Item kind (function, struct, etc.)
    kind: String,
    /// File path from Postgres
    file_path: String,
    start_line: u32,
    end_line: u32,
    /// Enriched from Postgres
    visibility: Option<String>,
    signature: Option<String>,
    docstring: Option<String>,
    module_path: Option<String>,
    crate_name: Option<String>,
    /// Full source body (if requested)
    body_source: Option<String>,
    /// Graph context from Neo4j (if requested)
    callers: Vec<CallerInfo>,
    callees: Vec<CalleeInfo>,
}

fn default_true() -> bool { true }
fn default_limit() -> usize { 10 }
fn default_depth() -> usize { 1 }

// =============================================================================
// API Handlers
// =============================================================================

async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError> {
    let mut dependencies = HashMap::new();
    
    // Check Postgres
    let start = std::time::Instant::now();
    match sqlx::query("SELECT 1").execute(&state.pg_pool).await {
        Ok(_) => {
            dependencies.insert("postgres".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Err(e) => {
            dependencies.insert("postgres".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }
    
    // Check Qdrant
    let start = std::time::Instant::now();
    match state.http_client
        .get(format!("{}/collections/{}", state.config.qdrant_host, state.config.collection_name))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Ok(resp) => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("Status: {}", resp.status())),
            });
        }
        Err(e) => {
            dependencies.insert("qdrant".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }
    
    // Check Ollama
    let start = std::time::Instant::now();
    match state.http_client
        .get(format!("{}/api/tags", state.config.ollama_host))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Ok(resp) => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: Some(format!("Status: {}", resp.status())),
            });
        }
        Err(e) => {
            dependencies.insert("ollama".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }
    
    // Check Neo4j
    let start = std::time::Instant::now();
    match check_neo4j(&state).await {
        Ok(_) => {
            dependencies.insert("neo4j".to_string(), DependencyStatus {
                status: "healthy".to_string(),
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            });
        }
        Err(e) => {
            dependencies.insert("neo4j".to_string(), DependencyStatus {
                status: "unhealthy".to_string(),
                latency_ms: None,
                error: Some(e.to_string()),
            });
        }
    }
    
    let all_healthy = dependencies.values().all(|d| d.status == "healthy");
    let status = if all_healthy { "healthy" } else { "degraded" };
    
    Ok(Json(HealthResponse {
        status: status.to_string(),
        timestamp: Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        dependencies,
    }))
}

async fn metrics_handler(State(state): State<AppState>) -> Response {
    let encoder = TextEncoder::new();
    let metric_families = state.metrics.registry.gather();
    let mut buffer = Vec::new();
    
    if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
        error!("Failed to encode metrics: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics").into_response();
    }
    
    match String::from_utf8(buffer) {
        Ok(metrics_text) => (StatusCode::OK, metrics_text).into_response(),
        Err(e) => {
            error!("Failed to convert metrics to string: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to encode metrics").into_response()
        }
    }
}

async fn search_semantic(
    State(state): State<AppState>,
    Json(req): Json<SearchSemanticRequest>,
) -> Result<Json<SearchSemanticResponse>, AppError> {
    state.metrics.record_request("search_semantic", "POST");
    debug!("Semantic search for: {}", req.query);
    
    // Get embedding from Ollama
    let embedding = get_embedding(&state, &req.query).await?;
    
    // Search Qdrant
    let search_request = serde_json::json!({
        "vector": embedding,
        "limit": req.limit,
        "with_payload": true,
        "score_threshold": req.score_threshold,
    });
    
    let search_url = format!(
        "{}/collections/{}/points/search",
        state.config.qdrant_host,
        state.config.collection_name
    );
    
    let response = state.http_client
        .post(&search_url)
        .json(&search_request)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to search Qdrant: {}", e)))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Qdrant(format!("Qdrant search failed: {} - {}", status, body)));
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

async fn get_function(
    State(state): State<AppState>,
    Query(query): Query<GetFunctionQuery>,
) -> Result<Json<FunctionDetail>, AppError> {
    state.metrics.record_request("get_function", "GET");
    debug!("Get function: {}", query.fqn);
    
    // Query Postgres for function details
    let row = sqlx::query_as::<_, (String, String, String, String, Option<String>, Option<String>, Option<String>, i32, i32, Option<String>, Option<String>)>(
        r#"
        SELECT e.fqn, e.name, e.item_type, e.visibility, e.signature, e.doc_comment as docstring, 
               sf.file_path, e.start_line, e.end_line, sf.module_path, sf.crate_name
        FROM extracted_items e
        LEFT JOIN source_files sf ON e.source_file_id = sf.id
        WHERE e.fqn = $1
        "#
    )
    .bind(&query.fqn)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to query function: {}", e)))?;
    
    let (fqn, name, item_type, visibility, signature, docstring, file_path, start_line, end_line, module_path, crate_name) = 
        row.ok_or_else(|| AppError::NotFound(format!("Item not found: {}", query.fqn)))?;
    
    // Get callers from Neo4j and convert to CallerInfo
    let caller_nodes = get_callers_from_neo4j(&state, &query.fqn, 1).await?;
    let callers: Vec<CallerInfo> = caller_nodes
        .into_iter()
        .map(|n| CallerInfo {
            fqn: n.fqn,
            name: n.name,
            file_path: n.file_path,
            line: n.line,
        })
        .collect();
    
    // Get callees from Neo4j
    let callees = get_callees_from_neo4j(&state, &query.fqn).await?;
    
    Ok(Json(FunctionDetail {
        fqn,
        name,
        kind: item_type,
        visibility: Some(visibility),
        signature,
        docstring,
        file_path: file_path.unwrap_or_default(),
        start_line: start_line as u32,
        end_line: end_line as u32,
        module_path,
        crate_name,
        callers,
        callees,
    }))
}

async fn get_callers(
    State(state): State<AppState>,
    Query(query): Query<GetCallersQuery>,
) -> Result<Json<CallersResponse>, AppError> {
    state.metrics.record_request("get_callers", "GET");
    debug!("Get callers for: {} (depth: {})", query.fqn, query.depth);
    
    let callers = get_callers_from_neo4j(&state, &query.fqn, query.depth).await?;
    
    Ok(Json(CallersResponse {
        fqn: query.fqn,
        callers,
        depth: query.depth,
    }))
}

async fn get_trait_impls(
    State(state): State<AppState>,
    Query(query): Query<GetTraitImplsQuery>,
) -> Result<Json<TraitImplsResponse>, AppError> {
    state.metrics.record_request("get_trait_impls", "GET");
    debug!("Get trait impls for: {}", query.trait_name);
    
    // Query matches the actual IMPLEMENTS relationship structure:
    // (impl:Impl)-[:IMPLEMENTS]->(trait:Trait)
    let cypher = r#"
        MATCH (impl:Impl)-[:IMPLEMENTS]->(trait:Trait {name: $trait_name})
        RETURN impl.fqn as impl_fqn, impl.name as impl_name, trait.name as trait_name, 
               impl.start_line as start_line
        LIMIT $limit
        "#;
    
    let params = serde_json::json!({
        "trait_name": query.trait_name,
        "limit": query.limit as i32,
    });
    
    let results = execute_neo4j_query(&state, cypher, params).await?;
    
    let implementations = results
        .into_iter()
        .filter_map(|r| {
            Some(TraitImpl {
                impl_fqn: r.get("impl_fqn")?.as_str()?.to_string(),
                type_name: r.get("impl_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                file_path: r.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                start_line: r.get("start_line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
            })
        })
        .collect();
    
    Ok(Json(TraitImplsResponse {
        trait_name: query.trait_name,
        implementations,
    }))
}

async fn find_usages_of_type(
    State(state): State<AppState>,
    Query(query): Query<FindUsagesOfTypeQuery>,
) -> Result<Json<UsagesResponse>, AppError> {
    state.metrics.record_request("find_usages_of_type", "GET");
    debug!("Find usages of type: {}", query.type_name);
    
    let cypher = format!(
        r#"
        MATCH (n)-[:USES_TYPE]->(t:Type {{name: $type_name}})
        RETURN n.fqn as fqn, n.name as name, labels(n)[0] as kind, n.file_path as file_path, n.start_line as line
        LIMIT $limit
        "#
    );
    
    let params = serde_json::json!({
        "type_name": query.type_name,
        "limit": query.limit as i32,
    });
    
    let results = execute_neo4j_query(&state, &cypher, params).await?;
    
    let usages = results
        .into_iter()
        .filter_map(|r| {
            Some(TypeUsage {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
                kind: r.get("kind")?.as_str()?.to_string(),
                file_path: r.get("file_path")?.as_str()?.to_string(),
                line: r.get("line")?.as_i64()? as u32,
            })
        })
        .collect();
    
    Ok(Json(UsagesResponse {
        type_name: query.type_name,
        usages,
    }))
}

async fn get_module_tree(
    State(state): State<AppState>,
    Query(query): Query<GetModuleTreeQuery>,
) -> Result<Json<ModuleTreeResponse>, AppError> {
    state.metrics.record_request("get_module_tree", "GET");
    debug!("Get module tree for crate: {}", query.crate_name);
    
    let cypher = format!(
        r#"
        MATCH (root:Module {{crate_name: $crate_name, is_crate_root: true}})
        OPTIONAL MATCH (root)-[r:CONTAINS*]->(child:Module)
        WITH root, collect(DISTINCT child) as modules
        OPTIONAL MATCH (root)-[:DEFINES]->(item)
        WITH root, modules, collect({{name: item.name, kind: labels(item)[0], visibility: item.visibility}}) as root_items
        RETURN root.name as root_name, root.path as root_path, modules, root_items
        "#
    );
    
    let params = serde_json::json!({
        "crate_name": query.crate_name,
    });
    
    let results = execute_neo4j_query(&state, &cypher, params).await?;
    
    let root = if let Some(first) = results.first() {
        let root_name = first.get("root_name")
            .and_then(|v| v.as_str())
            .unwrap_or(&query.crate_name)
            .to_string();
        let root_path = first.get("root_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        
        let root_items: Vec<ModuleItem> = first.get("root_items")
            .and_then(|v| v.as_array())
            .map(|items| {
                items.iter().filter_map(|item| {
                    Some(ModuleItem {
                        name: item.get("name")?.as_str()?.to_string(),
                        kind: item.get("kind")?.as_str()?.to_string(),
                        visibility: item.get("visibility").and_then(|v| v.as_str()).unwrap_or("private").to_string(),
                    })
                }).collect()
            })
            .unwrap_or_default();
        
        let modules: Vec<ModuleNode> = first.get("modules")
            .and_then(|v| v.as_array())
            .map(|mods| {
                mods.iter().filter_map(|m| {
                    Some(ModuleNode {
                        name: m.get("name")?.as_str()?.to_string(),
                        path: m.get("path")?.as_str()?.to_string(),
                        children: vec![],
                        items: vec![],
                    })
                }).collect()
            })
            .unwrap_or_default();
        
        ModuleNode {
            name: root_name,
            path: root_path,
            children: modules,
            items: root_items,
        }
    } else {
        ModuleNode {
            name: query.crate_name.clone(),
            path: query.crate_name.clone(),
            children: vec![],
            items: vec![],
        }
    };
    
    Ok(Json(ModuleTreeResponse {
        crate_name: query.crate_name,
        root,
    }))
}

async fn query_graph(
    State(state): State<AppState>,
    Json(req): Json<QueryGraphRequest>,
) -> Result<Json<GraphQueryResponse>, AppError> {
    state.metrics.record_request("query_graph", "POST");
    
    // Validate query is read-only
    let query_lower = req.query.to_lowercase();
    if query_lower.contains("create") || query_lower.contains("delete") || 
       query_lower.contains("set") || query_lower.contains("remove") ||
       query_lower.contains("merge") {
        return Err(AppError::BadRequest("Only read-only queries are allowed".to_string()));
    }
    
    debug!("Executing Cypher query: {}", req.query);
    
    let results = execute_neo4j_query(&state, &req.query, serde_json::Value::Object(
        req.parameters.into_iter().map(|(k, v)| (k, v)).collect()
    )).await?;
    
    let row_count = results.len();
    
    Ok(Json(GraphQueryResponse {
        query: req.query,
        results,
        row_count,
    }))
}

/// Cross-database aggregation: Qdrant (semantic) + Postgres (metadata) + Neo4j (graph)
async fn aggregate_search(
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
        state.config.qdrant_host,
        state.config.collection_name
    );

    let response = state.http_client
        .post(&search_url)
        .json(&search_request)
        .send()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to search Qdrant: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Qdrant(format!("Qdrant search failed: {} - {}", status, body)));
    }

    let search_result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Qdrant(format!("Failed to parse Qdrant response: {}", e)))?;

    let qdrant_results = parse_search_results(&search_result);

    // Step 3: Enrich each result with Postgres metadata and Neo4j graph context
    let mut aggregated = Vec::with_capacity(qdrant_results.len());

    for result in &qdrant_results {
        let enriched = enrich_search_result(&state, result, req.include_graph, req.include_source).await;
        aggregated.push(enriched);
    }

    Ok(Json(AggregateSearchResponse {
        query: req.query,
        total: aggregated.len(),
        results: aggregated,
    }))
}

/// Enrich a Qdrant search result with Postgres metadata and Neo4j graph context
async fn enrich_search_result(
    state: &AppState,
    result: &SearchResult,
    include_graph: bool,
    include_source: bool,
) -> AggregatedResult {
    // Query Postgres for full metadata
    let select_body = if include_source { ", e.body_source" } else { "" };
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
        file_path: pg_data.as_ref()
            .and_then(|d| d.file_path.clone())
            .unwrap_or_else(|| result.file_path.clone()),
        start_line: pg_data.as_ref()
            .map(|d| d.start_line as u32)
            .unwrap_or(result.start_line),
        end_line: pg_data.as_ref()
            .map(|d| d.end_line as u32)
            .unwrap_or(result.end_line),
        visibility: pg_data.as_ref().and_then(|d| d.visibility.clone()),
        signature: pg_data.as_ref().and_then(|d| d.signature.clone()),
        docstring: pg_data.as_ref().and_then(|d| d.doc_comment.clone())
            .or_else(|| result.docstring.clone()),
        module_path: pg_data.as_ref().and_then(|d| d.module_path.clone()),
        crate_name: pg_data.as_ref().and_then(|d| d.crate_name.clone()),
        body_source: pg_data.as_ref().and_then(|d| d.body_source.clone()),
        callers,
        callees,
    }
}

/// Internal enrichment data from Postgres
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
// Helper Functions
// =============================================================================

async fn get_embedding(state: &AppState, text: &str) -> Result<Vec<f32>, AppError> {
    let request = serde_json::json!({
        "model": state.config.embedding_model,
        "input": text,
    });
    
    let response = state.http_client
        .post(format!("{}/api/embed", state.config.ollama_host))
        .json(&request)
        .send()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to get embedding: {}", e)))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Ollama(format!("Ollama embedding failed: {} - {}", status, body)));
    }
    
    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Ollama(format!("Failed to parse Ollama response: {}", e)))?;
    
    let embedding = result.get("embeddings")
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

fn parse_search_results(search_result: &serde_json::Value) -> Vec<SearchResult> {
    search_result
        .get("result")
        .and_then(|v| v.as_array())
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    let payload = r.get("payload")?;
                    // Qdrant payload uses "item_type" not "kind"
                    let kind = payload.get("item_type")
                        .and_then(|v| v.as_str())
                        .or_else(|| payload.get("kind").and_then(|v| v.as_str()))
                        .unwrap_or("unknown")
                        .to_string();
                    // file_path may not exist in all payloads
                    let file_path = payload.get("file_path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // start_line/end_line are stored as i64 in Qdrant
                    let start_line = payload.get("start_line")
                        .and_then(|v| v.as_i64())
                        .or_else(|| payload.get("start_line").and_then(|v| v.as_u64()).map(|n| n as i64))
                        .unwrap_or(0) as u32;
                    let end_line = payload.get("end_line")
                        .and_then(|v| v.as_i64())
                        .or_else(|| payload.get("end_line").and_then(|v| v.as_u64()).map(|n| n as i64))
                        .unwrap_or(0) as u32;
                    Some(SearchResult {
                        fqn: payload.get("fqn")?.as_str()?.to_string(),
                        name: payload.get("name")?.as_str()?.to_string(),
                        kind,
                        file_path,
                        start_line,
                        end_line,
                        score: r.get("score")?.as_f64()? as f32,
                        snippet: payload.get("snippet").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        docstring: payload.get("docstring").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn check_neo4j(state: &AppState) -> Result<(), AppError> {
    let mut result = state.neo4j_graph
        .execute(neo4rs::query("RETURN 1 as test"))
        .await
        .map_err(|e| AppError::Neo4j(format!("Neo4j health check failed: {}", e)))?;

    // Consume the result to verify the connection works
    let _row = result.next().await
        .map_err(|e| AppError::Neo4j(format!("Neo4j health check failed: {}", e)))?;

    Ok(())
}

/// Execute a Cypher query via Neo4j Bolt protocol and return results as JSON values.
///
/// Each row is returned as a JSON object with column names as keys.
async fn execute_neo4j_query(
    state: &AppState,
    query: &str,
    params: serde_json::Value
) -> Result<Vec<serde_json::Value>, AppError> {
    // Build the neo4rs query with parameters
    let mut q = neo4rs::query(query);

    // Add parameters from the JSON value
    if let serde_json::Value::Object(map) = &params {
        for (key, value) in map {
            q = match value {
                serde_json::Value::String(s) => q.param(key.as_str(), s.as_str()),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        q.param(key.as_str(), i)
                    } else if let Some(f) = n.as_f64() {
                        q.param(key.as_str(), f)
                    } else {
                        q
                    }
                }
                serde_json::Value::Bool(b) => q.param(key.as_str(), *b),
                serde_json::Value::Null => q,
                // For complex types, convert to string
                other => q.param(key.as_str(), other.to_string()),
            };
        }
    }

    let mut result = state.neo4j_graph
        .execute(q)
        .await
        .map_err(|e| AppError::Neo4j(format!("Failed to execute Neo4j query: {}", e)))?;

    let mut rows = Vec::new();
    while let Some(row) = result.next().await
        .map_err(|e| AppError::Neo4j(format!("Failed to fetch Neo4j row: {}", e)))?
    {
        // Convert each row to a JSON value by extracting columns
        // neo4rs Row supports get() by column name, but we need to handle
        // the row data generically. We'll extract known column patterns.
        let row_json = row_to_json(&row);
        rows.push(row_json);
    }

    Ok(rows)
}

/// Convert a neo4rs Row to a serde_json::Value.
///
/// Attempts to extract values by trying common types in order.
fn row_to_json(row: &neo4rs::Row) -> serde_json::Value {
    // neo4rs Row provides get::<T>(column_name) but doesn't expose column names directly.
    // The row internally tracks columns. We need to access them through the row's keys.
    // neo4rs::Row has a `to::<T>()` method for single-column results and `get::<T>(key)`.
    // Since we can't iterate column names, we rely on the query returning named columns
    // and the caller knowing what to expect.
    //
    // For the generic execute_neo4j_query, we return the row's BoltType conversion.
    // The callers (get_callers_from_neo4j, etc.) will extract specific columns.

    // Try to get the row as a BoltMap which we can iterate
    if let Ok(node) = row.to::<neo4rs::BoltMap>() {
        bolt_map_to_json(&node)
    } else {
        // Fallback: try single value
        serde_json::Value::Null
    }
}

/// Convert a BoltMap to a JSON object
fn bolt_map_to_json(map: &neo4rs::BoltMap) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (key, value) in &map.value {
        obj.insert(key.to_string(), bolt_type_to_json(value));
    }
    serde_json::Value::Object(obj)
}

/// Convert a BoltType to a JSON value
fn bolt_type_to_json(value: &neo4rs::BoltType) -> serde_json::Value {
    match value {
        neo4rs::BoltType::String(s) => serde_json::Value::String(s.to_string()),
        neo4rs::BoltType::Integer(i) => serde_json::json!(i.value),
        neo4rs::BoltType::Float(f) => serde_json::json!(f.value),
        neo4rs::BoltType::Boolean(b) => serde_json::Value::Bool(b.value),
        neo4rs::BoltType::Null(_) => serde_json::Value::Null,
        neo4rs::BoltType::List(list) => {
            let items: Vec<serde_json::Value> = list.iter()
                .map(bolt_type_to_json)
                .collect();
            serde_json::Value::Array(items)
        }
        neo4rs::BoltType::Map(map) => bolt_map_to_json(map),
        neo4rs::BoltType::Node(node) => {
            let mut obj = serde_json::Map::new();
            for (key, value) in &node.properties.value {
                obj.insert(key.to_string(), bolt_type_to_json(value));
            }
            let labels: Vec<serde_json::Value> = node.labels.iter()
                .map(bolt_type_to_json)
                .collect();
            obj.insert("_labels".to_string(), serde_json::Value::Array(labels));
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}

async fn get_callers_from_neo4j(
    state: &AppState,
    fqn: &str,
    depth: usize,
) -> Result<Vec<CallerNode>, AppError> {
    let cypher = format!(
        r#"
        MATCH path = (caller)-[:CALLS*1..{}]->(callee:Function {{fqn: $fqn}})
        RETURN caller.fqn as fqn, caller.name as name, caller.file_path as file_path,
               caller.start_line as line, length(path) as depth
        ORDER BY depth, fqn
        "#,
        depth
    );

    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, &cypher, params).await?;

    let callers = results
        .into_iter()
        .filter_map(|r| {
            Some(CallerNode {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
                file_path: r.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                line: r.get("line")?.as_i64()? as u32,
                depth: r.get("depth")?.as_i64()? as usize,
            })
        })
        .collect();

    Ok(callers)
}

async fn get_callees_from_neo4j(
    state: &AppState,
    fqn: &str,
) -> Result<Vec<CalleeInfo>, AppError> {
    let cypher = r#"
        MATCH (caller:Function {fqn: $fqn})-[:CALLS]->(callee:Function)
        RETURN callee.fqn as fqn, callee.name as name
        ORDER BY name
    "#;

    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, cypher, params).await?;

    let callees = results
        .into_iter()
        .filter_map(|r| {
            Some(CalleeInfo {
                fqn: r.get("fqn")?.as_str()?.to_string(),
                name: r.get("name")?.as_str()?.to_string(),
            })
        })
        .collect();

    Ok(callees)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }

    #[test]
    fn test_default_depth() {
        assert_eq!(default_depth(), 1);
    }

    #[test]
    fn test_config_from_env_defaults() {
        // Unset env vars to test defaults
        let config = Config {
            database_url: "postgresql://rustbrain:rustbrain_dev_2024@postgres:5432/rustbrain".to_string(),
            neo4j_uri: "bolt://neo4j:7687".to_string(),
            neo4j_user: "neo4j".to_string(),
            neo4j_password: "rustbrain_dev_2024".to_string(),
            qdrant_host: "http://qdrant:6333".to_string(),
            ollama_host: "http://ollama:11434".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            embedding_dimensions: 768,
            collection_name: "rust_functions".to_string(),
            port: 8080,
        };

        assert_eq!(config.embedding_dimensions, 768);
        assert_eq!(config.port, 8080);
        assert_eq!(config.embedding_model, "nomic-embed-text");
    }

    #[test]
    fn test_app_error_display() {
        assert_eq!(
            AppError::Database("conn refused".to_string()).to_string(),
            "Database error: conn refused"
        );
        assert_eq!(
            AppError::NotFound("item".to_string()).to_string(),
            "Not found: item"
        );
        assert_eq!(
            AppError::BadRequest("invalid".to_string()).to_string(),
            "Bad request: invalid"
        );
        assert_eq!(
            AppError::Neo4j("timeout".to_string()).to_string(),
            "Neo4j error: timeout"
        );
        assert_eq!(
            AppError::Qdrant("error".to_string()).to_string(),
            "Qdrant error: error"
        );
        assert_eq!(
            AppError::Ollama("error".to_string()).to_string(),
            "Ollama error: error"
        );
        assert_eq!(
            AppError::Internal("panic".to_string()).to_string(),
            "Internal error: panic"
        );
    }

    #[test]
    fn test_app_error_into_response_status_codes() {
        use axum::http::StatusCode;

        let cases = vec![
            (AppError::Database("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Neo4j("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Qdrant("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Ollama("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Internal("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::NotFound("err".into()), StatusCode::NOT_FOUND),
            (AppError::BadRequest("err".into()), StatusCode::BAD_REQUEST),
        ];

        for (error, expected_status) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), expected_status);
        }
    }

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
    fn test_metrics_creation() {
        let metrics = Metrics::new();
        metrics.record_request("test_endpoint", "GET");
        metrics.record_error("test_endpoint", "500");
        // No panic = success
    }

    #[test]
    fn test_api_error_serialization() {
        let api_error = ApiError {
            error: "Something went wrong".to_string(),
            code: "INTERNAL_ERROR".to_string(),
        };
        let json = serde_json::to_value(&api_error).unwrap();
        assert_eq!(json["error"], "Something went wrong");
        assert_eq!(json["code"], "INTERNAL_ERROR");
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
    fn test_query_graph_request_deserialization() {
        let json = serde_json::json!({
            "query": "MATCH (n) RETURN n LIMIT 10",
            "parameters": {"name": "test"},
            "limit": 20
        });

        let req: QueryGraphRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.query, "MATCH (n) RETURN n LIMIT 10");
        assert_eq!(req.parameters.get("name").unwrap(), "test");
        assert_eq!(req.limit, 20);
    }

    #[test]
    fn test_health_response_serialization() {
        let mut deps = HashMap::new();
        deps.insert("postgres".to_string(), DependencyStatus {
            status: "healthy".to_string(),
            latency_ms: Some(5),
            error: None,
        });

        let resp = HealthResponse {
            status: "healthy".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            version: "0.1.0".to_string(),
            dependencies: deps,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "healthy");
        assert_eq!(json["dependencies"]["postgres"]["status"], "healthy");
        assert_eq!(json["dependencies"]["postgres"]["latency_ms"], 5);
    }

    // Neo4j Bolt conversion tests

    #[test]
    fn test_bolt_type_to_json_string() {
        let bolt = neo4rs::BoltType::String(neo4rs::BoltString::from("hello"));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::String("hello".to_string()));
    }

    #[test]
    fn test_bolt_type_to_json_integer() {
        let bolt = neo4rs::BoltType::Integer(neo4rs::BoltInteger::new(42));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::json!(42));
    }

    #[test]
    fn test_bolt_type_to_json_float() {
        let bolt = neo4rs::BoltType::Float(neo4rs::BoltFloat::new(3.14));
        let json = bolt_type_to_json(&bolt);
        assert!((json.as_f64().unwrap() - 3.14).abs() < f64::EPSILON);
    }

    #[test]
    fn test_bolt_type_to_json_boolean() {
        let bolt = neo4rs::BoltType::Boolean(neo4rs::BoltBoolean::new(true));
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::Bool(true));
    }

    #[test]
    fn test_bolt_type_to_json_null() {
        let bolt = neo4rs::BoltType::Null(neo4rs::BoltNull);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::Value::Null);
    }

    #[test]
    fn test_bolt_type_to_json_list() {
        let list = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("a"),
            neo4rs::BoltType::from("b"),
        ]);
        let bolt = neo4rs::BoltType::List(list);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json, serde_json::json!(["a", "b"]));
    }

    #[test]
    fn test_bolt_map_to_json() {
        let mut map = neo4rs::BoltMap::new();
        map.put("name".into(), neo4rs::BoltType::from("test_fn"));
        map.put("line".into(), neo4rs::BoltType::from(42_i64));
        let json = bolt_map_to_json(&map);
        assert_eq!(json["name"], "test_fn");
        assert_eq!(json["line"], 42);
    }

    #[test]
    fn test_bolt_node_to_json() {
        let properties: neo4rs::BoltMap = vec![
            (neo4rs::BoltString::from("fqn"), neo4rs::BoltType::from("crate::func")),
            (neo4rs::BoltString::from("name"), neo4rs::BoltType::from("func")),
        ].into_iter().collect();
        let labels = neo4rs::BoltList::from(vec![neo4rs::BoltType::from("Function")]);
        let node = neo4rs::BoltNode::new(neo4rs::BoltInteger::new(1), labels, properties);
        let bolt = neo4rs::BoltType::Node(node);
        let json = bolt_type_to_json(&bolt);
        assert_eq!(json["fqn"], "crate::func");
        assert_eq!(json["name"], "func");
        assert!(json["_labels"].is_array());
    }

    #[test]
    fn test_row_to_json_with_bolt_map() {
        // Row is constructed from fields and data
        let fields = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("fqn"),
            neo4rs::BoltType::from("name"),
            neo4rs::BoltType::from("line"),
        ]);
        let data = neo4rs::BoltList::from(vec![
            neo4rs::BoltType::from("crate::module::func"),
            neo4rs::BoltType::from("func"),
            neo4rs::BoltType::from(10_i64),
        ]);
        let row = neo4rs::Row::new(fields, data);
        let json = row_to_json(&row);
        assert_eq!(json["fqn"], "crate::module::func");
        assert_eq!(json["name"], "func");
        assert_eq!(json["line"], 10);
    }

    // Aggregate search tests

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
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rustbrain_api=debug".parse()?)
        )
        .json()
        .init();
    
    info!("Starting rust-brain API server");
    
    // Load configuration
    let config = Config::from_env();
    info!("Configuration loaded: port={}", config.port);
    
    // Connect to Postgres
    info!("Connecting to Postgres: {}", config.database_url);
    let pg_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    
    info!("Connected to Postgres");
    
    // Connect to Neo4j via Bolt protocol
    info!("Connecting to Neo4j: {}", config.neo4j_uri);
    let neo4j_graph = Graph::new(
        &config.neo4j_uri,
        &config.neo4j_user,
        &config.neo4j_password,
    ).await?;
    info!("Connected to Neo4j");

    // Create HTTP client (for Qdrant/Ollama)
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // Create metrics
    let metrics = Arc::new(Metrics::new());

    // Create app state
    let state = AppState {
        config: config.clone(),
        pg_pool,
        neo4j_graph: Arc::new(neo4j_graph),
        http_client,
        metrics,
    };
    
    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .route("/playground", get(playground_html))
        .route("/tools/search_semantic", post(search_semantic))
        .route("/tools/get_function", get(get_function))
        .route("/tools/get_callers", get(get_callers))
        .route("/tools/get_trait_impls", get(get_trait_impls))
        .route("/tools/find_usages_of_type", get(find_usages_of_type))
        .route("/tools/get_module_tree", get(get_module_tree))
        .route("/tools/query_graph", post(query_graph))
        .route("/tools/aggregate_search", post(aggregate_search))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    
    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}
