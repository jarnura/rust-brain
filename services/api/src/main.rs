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
    
    // Create app state
    let state = AppState {
        config: config.clone(),
        pg_pool,
        http_client,
        metrics,
    };
    
    // Build router
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .route("/tools/search_semantic", post(search_semantic))
        .route("/tools/get_function", get(get_function))
        .route("/tools/get_callers", get(get_callers))
        .route("/tools/get_trait_impls", get(get_trait_impls))
        .route("/tools/find_usages_of_type", get(find_usages_of_type))
        .route("/tools/get_module_tree", get(get_module_tree))
        .route("/tools/query_graph", post(query_graph))
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
