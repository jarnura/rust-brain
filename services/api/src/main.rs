//! rust-brain Tool API Server
//!
//! Provides REST endpoints for code intelligence queries.

mod audit;
mod gaps;

use axum::{
    extract::{Query, State, ws::{Message, WebSocket, WebSocketUpgrade}},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::broadcast;
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

use audit::{AuditEntry, AuditLog, Operation, Status};
use gaps::GapAnalysis;

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
    http_client: reqwest::Client,
    metrics: Arc<Metrics>,
    audit_log: Arc<AuditLog>,
    start_time: std::time::Instant,
    ws_broadcast: broadcast::Sender<AuditEntry>,
    query_count: Arc<AtomicU64>,
    error_count: Arc<AtomicU64>,
    total_latency_ms: Arc<AtomicU64>,
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

#[derive(Debug, Serialize)]
struct CallerInfo {
    fqn: String,
    name: String,
    file_path: String,
    line: u32,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize, Clone)]
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

// =============================================================================
// Playground Types
// =============================================================================

#[derive(Debug, Serialize)]
struct PlaygroundStatus {
    services: HashMap<String, ServiceStatus>,
    stats: SystemStats,
    recent_queries: Vec<QueryRecord>,
    uptime_seconds: u64,
}

#[derive(Debug, Serialize, Clone)]
struct ServiceStatus {
    name: String,
    status: String,
    latency_ms: Option<u64>,
    last_check: String,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SystemStats {
    total_queries: u64,
    total_errors: u64,
    avg_latency_ms: f64,
    queries_per_minute: f64,
}

#[derive(Debug, Serialize, Clone)]
struct QueryRecord {
    timestamp: String,
    endpoint: String,
    method: String,
    duration_ms: u64,
    status: String,
}

#[derive(Debug, Deserialize)]
struct AuditQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

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
    // Return a map object so Neo4j REST API returns named fields instead of array
    let cypher = r#"
        MATCH (impl:Impl)-[:IMPLEMENTS]->(trait:Trait {name: $trait_name})
        RETURN {impl_fqn: impl.fqn, impl_name: impl.name, trait_name: trait.name, 
                file_path: impl.file_path, start_line: impl.start_line} as node
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
            // r is an array like [{impl_fqn: ..., impl_name: ...}], get first element
            let node = r.as_array()?.first()?;
            Some(TraitImpl {
                impl_fqn: node.get("impl_fqn")?.as_str()?.to_string(),
                type_name: node.get("impl_name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                file_path: node.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                start_line: node.get("start_line").and_then(|v| v.as_i64()).unwrap_or(0) as u32,
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
    
    // Query all modules for this crate with their items
    // Modules have FQN like "crate_name::module::submodule"
    let cypher = r#"
        MATCH (c:Crate {name: $crate_name})
        OPTIONAL MATCH (c)-[:CONTAINS*]->(m:Module)
        WITH c, collect(DISTINCT m) as modules
        OPTIONAL MATCH (c)-[:DEFINES]->(crate_item)
        WITH c, modules, collect({fqn: crate_item.fqn, name: crate_item.name, kind: labels(crate_item)[0], visibility: crate_item.visibility}) as crate_items
        UNWIND modules as mod
        OPTIONAL MATCH (mod)-[:DEFINES]->(item)
        WITH c, modules, crate_items, mod, collect({name: item.name, kind: labels(item)[0], visibility: item.visibility}) as mod_items
        RETURN c.name as crate_name, 
               modules,
               crate_items,
               collect({fqn: mod.fqn, name: mod.name, items: mod_items}) as module_data
    "#;
    
    let params = serde_json::json!({
        "crate_name": query.crate_name,
    });
    
    let results = execute_neo4j_query(&state, cypher, params).await?;
    
    // Build module tree from flat results
    let root = build_module_tree(&query.crate_name, &results);
    
    Ok(Json(ModuleTreeResponse {
        crate_name: query.crate_name,
        root,
    }))
}

/// Build a hierarchical module tree from flat Neo4j results
/// Neo4j REST API returns rows as arrays: [crate_name, modules, crate_items, module_data]
fn build_module_tree(crate_name: &str, results: &[serde_json::Value]) -> ModuleNode {
    use std::collections::BTreeMap;
    
    // Collect all modules with their items
    let mut module_map: BTreeMap<String, (String, Vec<ModuleItem>)> = BTreeMap::new();
    let mut crate_items: Vec<ModuleItem> = Vec::new();
    
    if let Some(first) = results.first() {
        // Neo4j returns rows as arrays: [crate_name, modules, crate_items, module_data]
        let row = match first.as_array() {
            Some(arr) => arr,
            None => return ModuleNode {
                name: crate_name.to_string(),
                path: crate_name.to_string(),
                children: vec![],
                items: vec![],
            },
        };
        
        // Index 2: crate_items
        if let Some(items) = row.get(2).and_then(|v| v.as_array()) {
            crate_items = items.iter().filter_map(|item| {
                // Skip null items (crate may not define any items directly)
                if item.get("name").and_then(|v| v.as_str()).is_none() {
                    return None;
                }
                Some(ModuleItem {
                    name: item.get("name")?.as_str()?.to_string(),
                    kind: item.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                    visibility: item.get("visibility").and_then(|v| v.as_str()).unwrap_or("private").to_string(),
                })
            }).collect();
        }
        
        // Index 3: module_data
        if let Some(modules) = row.get(3).and_then(|v| v.as_array()) {
            for mod_entry in modules {
                if let (Some(fqn), Some(name)) = (
                    mod_entry.get("fqn").and_then(|v| v.as_str()),
                    mod_entry.get("name").and_then(|v| v.as_str())
                ) {
                    let items: Vec<ModuleItem> = mod_entry.get("items")
                        .and_then(|v| v.as_array())
                        .map(|items| {
                            items.iter().filter_map(|item| {
                                // Skip null items
                                if item.get("name").and_then(|v| v.as_str()).is_none() {
                                    return None;
                                }
                                Some(ModuleItem {
                                    name: item.get("name")?.as_str()?.to_string(),
                                    kind: item.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                                    visibility: item.get("visibility").and_then(|v| v.as_str()).unwrap_or("private").to_string(),
                                })
                            }).collect()
                        })
                        .unwrap_or_default();
                    
                    module_map.insert(fqn.to_string(), (name.to_string(), items));
                }
            }
        }
    }
    
    // Build tree structure - find direct children for each module
    fn build_children(
        parent_fqn: &str, 
        crate_name: &str,
        module_map: &BTreeMap<String, (String, Vec<ModuleItem>)>
    ) -> Vec<ModuleNode> {
        let mut children: Vec<ModuleNode> = Vec::new();
        
        for (fqn, (name, items)) in module_map {
            // Check if this module is a direct child of parent
            // Parent "crate" -> child "crate::module" (one :: separator more)
            // Parent "crate::mod" -> child "crate::mod::sub" 
            let expected_prefix = if parent_fqn.is_empty() {
                format!("")
            } else {
                format!("{}::", parent_fqn)
            };
            
            let is_direct_child = if parent_fqn.is_empty() {
                // For root, direct children have exactly one :: (crate::module)
                fqn.matches("::").count() == 1 && fqn.starts_with(&format!("{}::", crate_name))
            } else {
                // For non-root, child must start with "parent::" and have exactly one more :: than parent
                fqn.starts_with(&expected_prefix) && 
                fqn.matches("::").count() == parent_fqn.matches("::").count() + 1
            };
            
            if is_direct_child {
                let nested_children = build_children(fqn, crate_name, module_map);
                children.push(ModuleNode {
                    name: name.clone(),
                    path: fqn.clone(),
                    children: nested_children,
                    items: items.clone(),
                });
            }
        }
        
        children
    }
    
    // Build root node (crate itself) with direct module children
    let children = build_children(crate_name, crate_name, &module_map);
    
    ModuleNode {
        name: crate_name.to_string(),
        path: crate_name.to_string(),
        children,
        items: crate_items,
    }
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

// =============================================================================
// Playground API Handlers
// =============================================================================

async fn playground_status(State(state): State<AppState>) -> Json<PlaygroundStatus> {
    state.metrics.record_request("playground_status", "GET");
    
    let mut services = HashMap::new();
    
    // Check Postgres
    let start = std::time::Instant::now();
    let pg_status = match sqlx::query("SELECT 1").execute(&state.pg_pool).await {
        Ok(_) => ServiceStatus {
            name: "postgres".to_string(),
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: None,
        },
        Err(e) => ServiceStatus {
            name: "postgres".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: None,
            last_check: Utc::now().to_rfc3339(),
            error: Some(e.to_string()),
        },
    };
    services.insert("postgres".to_string(), pg_status);
    
    // Check Qdrant
    let start = std::time::Instant::now();
    let qdrant_status = match state.http_client
        .get(format!("{}/collections/{}", state.config.qdrant_host, state.config.collection_name))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => ServiceStatus {
            name: "qdrant".to_string(),
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: None,
        },
        Ok(resp) => ServiceStatus {
            name: "qdrant".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: Some(format!("Status: {}", resp.status())),
        },
        Err(e) => ServiceStatus {
            name: "qdrant".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: None,
            last_check: Utc::now().to_rfc3339(),
            error: Some(e.to_string()),
        },
    };
    services.insert("qdrant".to_string(), qdrant_status);
    
    // Check Ollama
    let start = std::time::Instant::now();
    let ollama_status = match state.http_client
        .get(format!("{}/api/tags", state.config.ollama_host))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => ServiceStatus {
            name: "ollama".to_string(),
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: None,
        },
        Ok(resp) => ServiceStatus {
            name: "ollama".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: Some(format!("Status: {}", resp.status())),
        },
        Err(e) => ServiceStatus {
            name: "ollama".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: None,
            last_check: Utc::now().to_rfc3339(),
            error: Some(e.to_string()),
        },
    };
    services.insert("ollama".to_string(), ollama_status);
    
    // Check Neo4j
    let start = std::time::Instant::now();
    let neo4j_status = match check_neo4j(&state).await {
        Ok(_) => ServiceStatus {
            name: "neo4j".to_string(),
            status: "healthy".to_string(),
            latency_ms: Some(start.elapsed().as_millis() as u64),
            last_check: Utc::now().to_rfc3339(),
            error: None,
        },
        Err(e) => ServiceStatus {
            name: "neo4j".to_string(),
            status: "unhealthy".to_string(),
            latency_ms: None,
            last_check: Utc::now().to_rfc3339(),
            error: Some(e.to_string()),
        },
    };
    services.insert("neo4j".to_string(), neo4j_status);
    
    // Calculate stats
    let total_queries = state.query_count.load(Ordering::Relaxed);
    let total_errors = state.error_count.load(Ordering::Relaxed);
    let total_latency = state.total_latency_ms.load(Ordering::Relaxed);
    let uptime_secs = state.start_time.elapsed().as_secs();
    
    let stats = SystemStats {
        total_queries,
        total_errors,
        avg_latency_ms: if total_queries > 0 { total_latency as f64 / total_queries as f64 } else { 0.0 },
        queries_per_minute: if uptime_secs > 0 { (total_queries as f64 * 60.0) / uptime_secs as f64 } else { 0.0 },
    };
    
    // Get recent queries from audit log
    let recent_entries = state.audit_log.get_recent(10).await;
    let recent_queries: Vec<QueryRecord> = recent_entries
        .into_iter()
        .map(|e| QueryRecord {
            timestamp: e.timestamp.to_rfc3339(),
            endpoint: e.operation.to_string(),
            method: e.input.get("method").and_then(|v| v.as_str()).unwrap_or("GET").to_string(),
            duration_ms: e.duration_ms,
            status: match e.status {
                Status::Success => "success".to_string(),
                Status::PartialSuccess => "partial_success".to_string(),
                Status::Failure => "failure".to_string(),
            },
        })
        .collect();
    
    Json(PlaygroundStatus {
        services,
        stats,
        recent_queries,
        uptime_seconds: uptime_secs,
    })
}

async fn playground_gaps(State(state): State<AppState>) -> Json<GapAnalysis> {
    state.metrics.record_request("playground_gaps", "GET");
    Json(GapAnalysis::analyze(&state).await)
}

async fn playground_audit(
    State(state): State<AppState>,
    Query(params): Query<AuditQuery>,
) -> Json<Vec<AuditEntry>> {
    state.metrics.record_request("playground_audit", "GET");
    
    let limit = params.limit.unwrap_or(100).min(1000);
    let operation_filter = params.operation.as_deref();
    let status_filter = params.status.as_deref();
    
    let entries = if let Some(op) = operation_filter {
        state.audit_log.get_by_operation(op).await
    } else if let Some(st) = status_filter {
        let status = match st.to_lowercase().as_str() {
            "success" => Status::Success,
            "partial_success" | "partialsuccess" => Status::PartialSuccess,
            "failure" | "error" => Status::Failure,
            _ => Status::Success,
        };
        state.audit_log.get_by_status(&status).await
    } else {
        state.audit_log.get_recent(limit).await
    };
    
    // Apply limit after filtering
    let entries: Vec<AuditEntry> = entries.into_iter().take(limit).collect();
    
    Json(entries)
}

/// Get audit statistics
async fn playground_audit_stats(
    State(state): State<AppState>,
) -> Json<audit::AuditStats> {
    state.metrics.record_request("playground_audit_stats", "GET");
    
    let stats = state.audit_log.get_stats().await;
    Json(stats)
}

async fn playground_ws(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_websocket(socket, state.ws_broadcast.clone()))
}

async fn handle_websocket(socket: WebSocket, broadcast: broadcast::Sender<AuditEntry>) {
    let (mut tx, mut rx) = socket.split();
    let mut recv = broadcast.subscribe();
    
    // Spawn a task to handle incoming messages (for close/ping)
    let read_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = rx.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });
    
    // Send audit events to the client
    while let Ok(entry) = recv.recv().await {
        let json = match serde_json::to_string(&entry) {
            Ok(j) => j,
            Err(_) => continue,
        };
        
        if tx.send(Message::Text(json)).await.is_err() {
            break;
        }
    }
    
    read_task.abort();
}

/// Log an audit entry and broadcast to WebSocket clients
async fn log_audit(state: &AppState, entry: AuditEntry) {
    // Update counters
    state.query_count.fetch_add(1, Ordering::Relaxed);
    state.total_latency_ms.fetch_add(entry.duration_ms, Ordering::Relaxed);
    if entry.status == Status::Failure {
        state.error_count.fetch_add(1, Ordering::Relaxed);
    }
    
    // Store in ring buffer
    state.audit_log.log(entry.clone()).await;
    
    // Broadcast to WebSocket clients (ignore errors if no subscribers)
    let _ = state.ws_broadcast.send(entry);
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
    let cypher = "RETURN 1 as test";
    let params = serde_json::json!({});
    execute_neo4j_query(state, cypher, params).await?;
    Ok(())
}

async fn execute_neo4j_query(
    state: &AppState, 
    query: &str, 
    params: serde_json::Value
) -> Result<Vec<serde_json::Value>, AppError> {
    let request = serde_json::json!({
        "statements": [{
            "statement": query,
            "parameters": params,
        }]
    });
    
    // Convert bolt:// to http:// for REST API
    let http_uri = state.config.neo4j_uri
        .replace("bolt://", "http://")
        .replace(":7687", ":7474");
    
    let response = state.http_client
        .post(format!("{}/db/neo4j/tx/commit", http_uri))
        .basic_auth(&state.config.neo4j_user, Some(&state.config.neo4j_password))
        .json(&request)
        .send()
        .await
        .map_err(|e| AppError::Neo4j(format!("Failed to execute Neo4j query: {}", e)))?;
    
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(AppError::Neo4j(format!("Neo4j query failed: {} - {}", status, body)));
    }
    
    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AppError::Neo4j(format!("Failed to parse Neo4j response: {}", e)))?;
    
    let errors = result.get("errors")
        .and_then(|v| v.as_array())
        .map(|e| !e.is_empty())
        .unwrap_or(false);
    
    if errors {
        let error_msg = result.get("errors")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("Unknown Neo4j error");
        return Err(AppError::Neo4j(error_msg.to_string()));
    }
    
    let results = result.get("results")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|r| r.get("data"))
        .and_then(|d| d.as_array())
        .map(|data| {
            data.iter()
                .filter_map(|row| {
                    row.get("row").cloned()
                })
                .collect()
        })
        .unwrap_or_default();
    
    Ok(results)
}

async fn get_callers_from_neo4j(
    state: &AppState,
    fqn: &str,
    depth: usize,
) -> Result<Vec<CallerNode>, AppError> {
    // Return a map object so Neo4j REST API returns named fields instead of array
    let cypher = format!(
        r#"
        MATCH path = (caller)-[:CALLS*1..{}]->(callee:Function {{fqn: $fqn}})
        RETURN {{fqn: caller.fqn, name: caller.name, line: caller.start_line, depth: length(path)}} as node
        ORDER BY node.depth, node.fqn
        "#,
        depth
    );
    
    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, &cypher, params).await?;
    
    let callers = results
        .into_iter()
        .filter_map(|r| {
            // r is an array like [{fqn: ..., name: ...}], get first element
            let node = r.as_array()?.first()?;
            Some(CallerNode {
                fqn: node.get("fqn")?.as_str()?.to_string(),
                name: node.get("name")?.as_str()?.to_string(),
                file_path: node.get("file_path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                line: node.get("line")?.as_i64()? as u32,
                depth: node.get("depth")?.as_i64()? as usize,
            })
        })
        .collect();
    
    Ok(callers)
}

async fn get_callees_from_neo4j(
    state: &AppState,
    fqn: &str,
) -> Result<Vec<CalleeInfo>, AppError> {
    // Return a map object so Neo4j REST API returns named fields instead of array
    let cypher = r#"
        MATCH (caller:Function {fqn: $fqn})-[:CALLS]->(callee:Function)
        RETURN {fqn: callee.fqn, name: callee.name} as node
        ORDER BY node.name
    "#;
    
    let params = serde_json::json!({"fqn": fqn});
    let results = execute_neo4j_query(state, cypher, params).await?;
    
    let callees = results
        .into_iter()
        .filter_map(|r| {
            // r is an array like [{fqn: ..., name: ...}], get first element
            let node = r.as_array()?.first()?;
            Some(CalleeInfo {
                fqn: node.get("fqn")?.as_str()?.to_string(),
                name: node.get("name")?.as_str()?.to_string(),
            })
        })
        .collect();
    
    Ok(callees)
}

// =============================================================================
// Audit Middleware
// =============================================================================

async fn audit_middleware(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let start = std::time::Instant::now();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    
    // Execute the request
    let response = next.run(request).await;
    
    // Calculate duration
    let duration_ms = start.elapsed().as_millis() as u64;
    
    // Determine operation type from path
    let operation = determine_operation_from_path(&path);
    
    // Determine status
    let status = if response.status().is_success() {
        Status::Success
    } else {
        Status::Failure
    };
    
    // Create audit entry
    let entry = state.audit_log.create_entry(
        operation,
        status.clone(),
        duration_ms,
        serde_json::json!({
            "method": method,
            "path": path,
        }),
        serde_json::json!({
            "status_code": response.status().as_u16(),
        }),
        if response.status().is_success() { None } else { Some(format!("HTTP {}", response.status().as_u16())) },
    ).await;
    
    // Log and broadcast
    log_audit(&state, entry).await;
    
    response
}

/// Determine operation type from request path
fn determine_operation_from_path(path: &str) -> Operation {
    match path {
        p if p.contains("/tools/search_semantic") => Operation::SemanticSearch { query: String::new() },
        p if p.contains("/tools/get_function") => Operation::GetFunction { fqn: String::new() },
        p if p.contains("/tools/get_callers") => Operation::GetCallers { fqn: String::new(), depth: 1 },
        p if p.contains("/tools/get_trait_impls") => Operation::GetTraitImpls { trait_name: String::new() },
        p if p.contains("/tools/find_usages_of_type") => Operation::FindUsages { type_name: String::new() },
        p if p.contains("/tools/get_module_tree") => Operation::ModuleTree { crate_name: String::new() },
        p if p.contains("/tools/query_graph") => Operation::GraphQuery { query: String::new() },
        p if p.contains("/health") => Operation::HealthCheck,
        _ => Operation::HealthCheck, // Default fallback
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
    
    // Create HTTP client
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    
    // Create metrics
    let metrics = Arc::new(Metrics::new());
    
    // Create audit log with 1000 entry capacity
    let audit_log = Arc::new(AuditLog::new(1000));
    
    // Create WebSocket broadcast channel
    let (ws_broadcast, _) = broadcast::channel(256);
    
    // Create atomic counters for stats
    let query_count = Arc::new(AtomicU64::new(0));
    let error_count = Arc::new(AtomicU64::new(0));
    let total_latency_ms = Arc::new(AtomicU64::new(0));
    
    // Record start time
    let start_time = std::time::Instant::now();
    
    // Create app state
    let state = AppState {
        config: config.clone(),
        pg_pool,
        http_client,
        metrics,
        audit_log,
        start_time,
        ws_broadcast,
        query_count,
        error_count,
        total_latency_ms,
    };
    
    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        // Core tool endpoints
        .route("/tools/search_semantic", post(search_semantic))
        .route("/tools/get_function", get(get_function))
        .route("/tools/get_callers", get(get_callers))
        .route("/tools/get_trait_impls", get(get_trait_impls))
        .route("/tools/find_usages_of_type", get(find_usages_of_type))
        .route("/tools/get_module_tree", get(get_module_tree))
        .route("/tools/query_graph", post(query_graph))
        // Playground endpoints
        .route("/playground/status", get(playground_status))
        .route("/playground/gaps", get(playground_gaps))
        .route("/playground/audit", get(playground_audit))
        .route("/playground/audit/stats", get(playground_audit_stats))
        .route("/playground/ws", get(playground_ws))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn_with_state(state.clone(), audit_middleware))
        .with_state(state);
    
    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Listening on {}", addr);
    
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}
