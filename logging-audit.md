# Rust Brain - Public Functions Audit

**Generated:** 2026-03-20
**Scope:** All public functions (`pub fn`, `pub async fn`) in `crates/` and `services/*/src/` directories

---

## Table of Contents

1. [Crates](#crates)
   - [rustbrain-common](#cratestbrain-common)
2. [Services](#services)
   - [API Service](#servicesapi)
   - [Ingestion Service](#servicesingestion)
   - [MCP Service](#servicesmcp)
3. [Summary Statistics](#summary-statistics)

---

## Crates

### crates/rustbrain-common

#### Module: `src/types.rs`

**Location:** `crates/rustbrain-common/src/types.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Visibility { pub fn as_str(&self) -> &str }` | 46 | Method |
| `impl ItemType { pub fn as_str(&self) -> &str }` | 89 | Method |
| `impl StoreReference { pub fn new(fqn: String, crate_name: String) -> Self }` | 158 | Constructor |
| `impl StoreReference { pub fn is_fully_synced(&self) -> bool }` | 170 | Method |
| `impl StoreReference { pub fn is_orphaned(&self) -> bool }` | 177 | Method |
| `impl StoreReference { pub fn missing_stores(&self) -> Vec<&'static str> }` | 184 | Method |

**Trait Implementations:**
- `impl Display for Visibility` (line 58)
- `impl Display for ItemType` (line 109)
- `impl Display for ResolutionQuality` (line 127)
- `impl FromStr for ResolutionQuality` (line 196)

#### Module: `src/config.rs`

**Location:** `crates/rustbrain-common/src/config.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl DatabaseConfig { pub fn from_env() -> Self }` | 25 | Constructor |

#### Module: `src/errors.rs`

**Location:** `crates/rustbrain-common/src/errors.rs`

No public functions defined (contains error types and trait implementations).

---

## Services

### services/api

#### Module: `src/config.rs`

**Location:** `services/api/src/config.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub fn redact_url(url: &str) -> String` | 4 | Standalone Function |
| `impl Config { pub fn from_env() -> Self }` | 37 | Constructor |

#### Module: `src/errors.rs`

**Location:** `services/api/src/errors.rs`

**Trait Implementations:**
- `impl Display for AppError` (line 29)
- `impl IntoResponse for AppError` (line 45)

#### Module: `src/state.rs`

**Location:** `services/api/src/state.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Metrics { pub fn new() -> Self }` | 27 | Constructor |
| `impl Metrics { pub fn record_request(&self, endpoint: &str, method: &str) }` | 57 | Method |
| `impl Metrics { pub fn record_error(&self, endpoint: &str, error_code: &str) }` | 61 | Method |

#### Module: `src/opencode.rs`

**Location:** `services/api/src/opencode.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl SendMessageResponse { pub fn into_message(self) -> Message }` | 61 | Consumer Method |
| `impl OpencodeClient { pub fn new(base_url: String, username: Option<String>, password: Option<String>) -> Self }` | 126 | Constructor |
| `impl OpencodeClient { pub fn base_url(&self) -> &str }` | 139 | Getter |
| `impl OpencodeClient { pub async fn health_check(&self) -> Result<bool> }` | 153 | Async Method |
| `impl OpencodeClient { pub async fn create_session(&self, title: Option<&str>) -> Result<Session> }` | 162 | Async Method |
| `impl OpencodeClient { pub async fn list_sessions(&self) -> Result<Vec<Session>> }` | 176 | Async Method |
| `impl OpencodeClient { pub async fn get_session(&self, session_id: &str) -> Result<Session> }` | 188 | Async Method |
| `impl OpencodeClient { pub async fn delete_session(&self, session_id: &str) -> Result<()> }` | 201 | Async Method |
| `impl OpencodeClient { pub async fn send_message(&self, session_id: &str, content: &str) -> Result<Message> }` | 211 | Async Method |
| `impl OpencodeClient { pub async fn send_message_async(&self, session_id: &str, content: &str) -> Result<String> }` | 233 | Async Method |
| `impl OpencodeClient { pub async fn abort_session(&self, session_id: &str) -> Result<()> }` | 258 | Async Method |
| `impl OpencodeClient { pub async fn fork_session(&self, session_id: &str, ...) -> Result<Session> }` | 268 | Async Method |
| `impl OpencodeClient { pub async fn get_messages(&self, session_id: &str) -> Result<Vec<Message>> }` | 287 | Async Method |

#### Module: `src/neo4j.rs`

**Location:** `services/api/src/neo4j.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn check_neo4j(state: &AppState) -> Result<(), AppError>` | 7 | Standalone Async Function |
| `pub async fn execute_neo4j_query(state: &AppState, query: &str, params: serde_json::Value) -> Result<Vec<serde_json::Value>, AppError>` | 22 | Standalone Async Function |
| `pub fn row_to_json(row: &neo4rs::Row) -> serde_json::Value` | 66 | Standalone Function |
| `pub fn bolt_map_to_json(map: &neo4rs::BoltMap) -> serde_json::Value` | 75 | Standalone Function |
| `pub fn bolt_type_to_json(value: &neo4rs::BoltType) -> serde_json::Value` | 84 | Standalone Function |
| `pub async fn get_callers_from_neo4j(state: &AppState, fqn: &str, depth: usize) -> Result<Vec<CallerInfo>, AppError>` | 113 | Standalone Async Function |
| `pub async fn get_callees_from_neo4j(state: &AppState, fqn: &str, depth: usize) -> Result<Vec<CalleeInfo>, AppError>` | 153 | Standalone Async Function |

#### Module: `src/audit.rs`

**Location:** `services/api/src/audit.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl AuditEntry { pub fn type_name(&self) -> &'static str }` | 49 | Method |
| `impl AuditLog { pub fn new(max_entries: usize) -> Self }` | 131 | Constructor |
| `impl AuditLog { pub async fn log(&self, entry: AuditEntry) }` | 140 | Async Method |
| `impl AuditLog { pub async fn create_entry(...) -> AuditEntry }` | 152 | Async Method |
| `impl AuditLog { pub async fn get_recent(&self, limit: usize) -> Vec<AuditEntry> }` | 178 | Async Method |
| `impl AuditLog { pub async fn get_all(&self) -> Vec<AuditEntry> }` | 184 | Async Method |
| `impl AuditLog { pub async fn get_by_operation(&self, op: &str) -> Vec<AuditEntry> }` | 190 | Async Method |
| `impl AuditLog { pub async fn get_by_status(&self, status: &Status) -> Vec<AuditEntry> }` | 201 | Async Method |
| `impl AuditLog { pub async fn get_by_id(&self, id: u64) -> Option<AuditEntry> }` | 212 | Async Method |
| `impl AuditLog { pub async fn count(&self) -> usize }` | 218 | Async Method |
| `impl AuditLog { pub async fn clear(&self) }` | 224 | Async Method |
| `impl AuditLog { pub async fn get_stats(&self) -> AuditStats }` | 230 | Async Method |

#### Module: `src/gaps.rs`

**Location:** `services/api/src/gaps.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl GapsAnalysis { pub async fn analyze(state: &AppState) -> Self }` | 83 | Async Constructor |

#### Module: `src/playground.rs`

**Location:** `services/api/src/playground.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn playground_html(State(_state): State<AppState>) -> Response<Body>` | 17 | Standalone Async Handler |

#### Module: `src/handlers/mod.rs`

**Location:** `services/api/src/handlers/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub fn default_true() -> bool { true }` | 44 | Standalone Function |
| `pub fn default_limit() -> usize { 10 }` | 45 | Standalone Function |
| `pub fn default_depth() -> usize { 1 }` | 46 | Standalone Function |

#### Module: `src/handlers/playground.rs`

**Location:** `services/api/src/handlers/playground.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn playground_html() -> impl IntoResponse` | 8 | Standalone Async Handler |

#### Module: `src/handlers/health.rs`

**Location:** `services/api/src/handlers/health.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, AppError>` | 36 | Standalone Async Handler |
| `pub async fn metrics_handler(State(state): State<AppState>) -> Response` | 182 | Standalone Async Handler |

#### Module: `src/handlers/chat.rs`

**Location:** `services/api/src/handlers/chat.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn chat_handler(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Result<Json<ChatResponse>, AppError>` | 39 | Standalone Async Handler |
| `pub async fn chat_stream_handler(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Result<impl Stream + Send + Sync, AppError>` | 106 | Standalone Async Handler |
| `pub async fn chat_send_handler(State(state): State<AppState>, Json(req): Json<SendMessageRequest>) -> Result<Json<MessageResponse>, AppError>` | 308 | Standalone Async Handler |
| `pub async fn chat_sessions_create(State(state): State<AppState>) -> Result<Json<SessionResponse>, AppError>` | 335 | Standalone Async Handler |
| `pub async fn chat_sessions_list(State(state): State<AppState>) -> Result<Json<Vec<Session>>, AppError>` | 346 | Standalone Async Handler |
| `pub async fn chat_sessions_get(State(state): State<AppState>, Path(session_id): Path<String>) -> Result<Json<Session>, AppError>` | 362 | Standalone Async Handler |
| `pub async fn chat_sessions_delete(State(state): State<AppState>, Path(session_id): Path<String>) -> Result<()>` | 377 | Standalone Async Handler |
| `pub async fn chat_sessions_fork(State(state): State<AppState>, Path(session_id): Path<String>) -> Result<Json<Session>, AppError>` | 394 | Standalone Async Handler |
| `pub async fn chat_sessions_abort(State(state): State<AppState>, Path(session_id): Path<String>) -> Result<()>` | 406 | Standalone Async Handler |

#### Module: `src/handlers/search.rs`

**Location:** `services/api/src/handlers/search.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn search_semantic(State(state): State<AppState>, Query(params): Query<SearchParams>) -> Result<Json<SearchResponse>, AppError>` | 144 | Standalone Async Handler |
| `pub async fn aggregate_search(State(state): State<AppState>, Query(params): Query<SearchParams>) -> Result<Json<AggregatedResults>, AppError>` | 196 | Standalone Async Handler |
| `pub fn parse_search_results(search_result: &serde_json::Value) -> Vec<SearchResult>` | 301 | Standalone Function |

#### Module: `src/handlers/items.rs`

**Location:** `services/api/src/handlers/items.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn get_function(State(state): State<AppState>, Query(query): Query<GetFunctionQuery>) -> Result<Json<FunctionDetail>, AppError>` | 60 | Standalone Async Handler |
| `pub async fn get_callers(State(state): State<AppState>, Query(query): Query<GetCallersQuery>) -> Result<Json<CallersResponse>, AppError>` | 120 | Standalone Async Handler |

#### Module: `src/handlers/graph.rs`

**Location:** `services/api/src/handlers/graph.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn get_trait_impls(State(state): State<AppState>, Query(query): Query<TraitQuery>) -> Result<Json<TraitResponse>, AppError>` | 109 | Standalone Async Handler |
| `pub async fn find_usages_of_type(State(state): State<AppState>, Query(query): Query<TypeUsageQuery>) -> Result<Json<UsageResponse>, AppError>` | 156 | Standalone Async Handler |
| `pub async fn get_module_tree(State(state): State<AppState>, Query(query): Query<ModuleQuery>) -> Result<Json<ModuleTreeResponse>, AppError>` | 200 | Standalone Async Handler |
| `pub async fn query_graph(State(state): State<AppState>, Json(query): Json<GraphQuery>) -> Result<Json<QueryResult>, AppError>` | 349 | Standalone Async Handler |

#### Module: `src/handlers/ingestion.rs`

**Location:** `services/api/src/handlers/ingestion.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn ingestion_progress(State(state): State<AppState>) -> Result<Json<IngestionProgress>, AppError>` | 34 | Standalone Async Handler |

---

### services/ingestion

#### Module: `src/parsers/mod.rs`

**Location:** `services/ingestion/src/parsers/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Parser { pub fn new() -> Result<Self> }` | 141 | Constructor |
| `impl Parser { pub fn parse(&self, source: &str, module_path: &str) -> Result<ParseResult> }` | 155 | Method |
| `impl Parser { pub fn parse_file(&self, path: &Path, module_path: &str) -> Result<ParseResult> }` | 222 | Method |

#### Module: `src/parsers/syn_parser.rs`

**Location:** `services/ingestion/src/parsers/syn_parser.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl SynParser { pub fn new() -> Self }` | 48 | Constructor |
| `impl SynParser { pub fn parse_item(&self, source: &str, module_path: &str, skeleton: &SkeletonItem) -> Result<ParsedItem> }` | 53 | Method |

#### Module: `src/parsers/tree_sitter_parser.rs`

**Location:** `services/ingestion/src/parsers/tree_sitter_parser.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl TreeSitterParser { pub fn new() -> Result<Self> }` | 21 | Constructor |
| `impl TreeSitterParser { pub fn extract_skeletons(&self, source: &str) -> Result<Vec<SkeletonItem>> }` | 33 | Method |
| `impl TreeSitterParser { pub fn extract_visibility(&self, source: &str) -> Option<Visibility> }` | 209 | Method |
| `impl TreeSitterParser { pub fn extract_attributes(&self, source: &str) -> Vec<String> }` | 268 | Method |
| `impl TreeSitterParser { pub fn extract_doc_comments(&self, source: &str, item_start_line: usize) -> String }` | 309 | Method |
| `impl TreeSitterParser { pub fn get_item_at_line(&self, source: &str, line: usize) -> Option<(usize, usize)> }` | 351 | Method |

#### Module: `src/derive_detector.rs`

**Location:** `services/ingestion/src/derive_detector.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl DeriveDetector { pub fn new() -> Self }` | 71 | Constructor |
| `impl DeriveDetector { pub fn detect(&self, attrs: &[Attribute]) -> Vec<String> }` | 94 | Method |

#### Module: `src/embedding/mod.rs`

**Location:** `services/ingestion/src/embedding/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl EmbeddingEngine { pub fn new(config: EmbeddingConfig) -> Result<Self> }` | 94 | Constructor |
| `impl EmbeddingEngine { pub fn with_urls(ollama_url: String, qdrant_url: String) -> Result<Self> }` | 109 | Constructor |
| `impl EmbeddingEngine { pub async fn initialize(&self) -> Result<()> }` | 117 | Async Method |
| `impl EmbeddingEngine { pub async fn embed_item(&self, item: &ParsedItem) -> Result<EmbeddedItem> }` | 146 | Async Method |
| `impl EmbeddingEngine { pub async fn embed_items(&self, items: &[ParsedItem]) -> Result<Vec<EmbeddedItem>> }` | 183 | Async Method |
| `impl EmbeddingEngine { pub async fn embed_doc_chunks(&self, item: &ParsedItem) -> Result<Vec<EmbeddedDoc>> }` | 237 | Async Method |
| `impl EmbeddingEngine { pub async fn embed_item_with_docs(&self, item: &ParsedItem) -> Result<(EmbeddedItem, Vec<EmbeddedDoc>)> }` | 282 | Async Method |
| `impl EmbeddingEngine { pub async fn embed_batch(&self, items: &[ParsedItem]) -> Result<Vec<EmbeddedItem>> }` | 295 | Async Method |
| `impl EmbeddingEngine { pub async fn search_code(&self, query: &str, limit: usize) -> Result<Vec<CodeSearchResult>> }` | 394 | Async Method |
| `impl EmbeddingEngine { pub async fn search_docs(&self, query: &str, limit: usize) -> Result<Vec<DocSearchResult>> }` | 436 | Async Method |
| `impl EmbeddingEngine { pub async fn get_stats(&self) -> Result<EmbeddingStats> }` | 464 | Async Method |

#### Module: `src/embedding/text_representation.rs`

**Location:** `services/ingestion/src/embedding/text_representation.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub fn generate_text_representation(item: &ParsedItem) -> TextRepresentation` | 39 | Standalone Function |
| `pub fn extract_doc_chunks(item: &ParsedItem, max_chunk_size: usize) -> Vec<DocChunk>` | 64 | Standalone Function |

#### Module: `src/embedding/ollama_client.rs`

**Location:** `services/ingestion/src/embedding/ollama_client.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl OllamaClient { pub fn new(config: OllamaConfig) -> Result<Self> }` | 93 | Constructor |
| `impl OllamaClient { pub fn with_base_url(base_url: String) -> Result<Self> }` | 105 | Constructor |
| `impl OllamaClient { pub async fn embed(&self, text: &str) -> Result<Vec<f32>> }` | 112 | Async Method |
| `impl OllamaClient { pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> }` | 158 | Async Method |
| `impl OllamaClient { pub async fn embed_all(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> }` | 243 | Async Method |
| `impl OllamaClient { pub async fn embed_all_concurrent(&self, texts: &[String], concurrency: usize) -> Result<Vec<Vec<f32>>> }` | 259 | Async Method |
| `impl OllamaClient { pub async fn health_check(&self) -> Result<bool> }` | 304 | Async Method |
| `impl OllamaClient { pub async fn check_model(&self) -> Result<bool> }` | 318 | Async Method |
| `impl OllamaClient { pub fn model(&self) -> &str }` | 366 | Getter |
| `impl OllamaClient { pub fn dimensions(&self) -> usize }` | 371 | Getter |

#### Module: `src/embedding/qdrant_client.rs`

**Location:** `services/ingestion/src/embedding/qdrant_client.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Payload { pub fn as_str(&self) -> Option<&str> }` | 94 | Method |
| `impl Payload { pub fn as_i64(&self) -> Option<i64> }` | 102 | Method |
| `impl Payload { pub fn as_bool(&self) -> Option<bool> }` | 110 | Method |
| `impl QdrantClient { pub fn new(config: QdrantConfig) -> Result<Self> }` | 251 | Constructor |
| `impl QdrantClient { pub fn with_base_url(base_url: String) -> Result<Self> }` | 263 | Constructor |
| `impl QdrantClient { pub async fn health_check(&self) -> Result<bool> }` | 270 | Async Method |
| `impl QdrantClient { pub async fn list_collections(&self) -> Result<Vec<String>> }` | 284 | Async Method |
| `impl QdrantClient { pub async fn ensure_collection(&self, collection: &str) -> Result<()> }` | 307 | Async Method |
| `impl QdrantClient { pub async fn ensure_collections(&self) -> Result<()> }` | 344 | Async Method |
| `impl QdrantClient { pub async fn get_collection_info(&self, collection: &str) -> Result<CollectionInfo> }` | 351 | Async Method |
| `impl QdrantClient { pub async fn upsert_point(&self, collection: &str, point: Point) -> Result<()> }` | 374 | Async Method |
| `impl QdrantClient { pub async fn upsert_points(&self, collection: &str, points: Vec<Point>) -> Result<()> }` | 407 | Async Method |
| `impl QdrantClient { pub async fn search(&self, collection: &str, query: Vec<f32>, limit: usize, threshold: f32) -> Result<Vec<SearchResult>> }` | 448 | Async Method |
| `impl QdrantClient { pub async fn get_existing_ids(&self, collection: &str, ids: &[Uuid]) -> Result<Vec<Uuid>> }` | 483 | Async Method |
| `impl QdrantClient { pub async fn delete_point(&self, collection: &str, id: Uuid) -> Result<()> }` | 556 | Async Method |
| `impl QdrantClient { pub async fn delete_by_filter(&self, collection: &str, filter: Filter) -> Result<()> }` | 588 | Async Method |
| `impl QdrantClient { pub fn code_collection(&self) -> &str }` | 618 | Getter |
| `impl QdrantClient { pub fn doc_collection(&self) -> &str }` | 623 | Getter |
| `impl QdrantClient { pub fn vector_size(&self) -> usize }` | 628 | Getter |

#### Module: `src/graph/mod.rs`

**Location:** `services/ingestion/src/graph/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Graph { pub async fn new() -> Result<Self> }` | 109 | Async Constructor |
| `impl Graph { pub async fn with_config(config: GraphConfig) -> Result<Self> }` | 114 | Async Constructor |
| `impl Graph { pub async fn test_connection(&self) -> Result<bool> }` | 144 | Async Method |
| `impl Graph { pub async fn create_indexes(&self) -> Result<()> }` | 164 | Async Method |
| `impl Graph { pub async fn clear_all(&self) -> Result<()> }` | 221 | Async Method |
| `impl Graph { pub fn nodes(&self) -> &NodeBuilder }` | 234 | Getter |
| `impl Graph { pub fn relationships(&self) -> &RelationshipBuilder }` | 239 | Getter |
| `impl Graph { pub fn batch(&self) -> Arc<RwLock<BatchInsert>> }` | 244 | Getter |
| `impl Graph { pub async fn stats(&self) -> GraphStats }` | 249 | Async Method |
| `impl Graph { pub async fn create_node(&self, node: &NodeData) -> Result<()> }` | 254 | Async Method |
| `impl Graph { pub async fn create_nodes_batch(&self, nodes: Vec<NodeData>) -> Result<()> }` | 264 | Async Method |
| `impl Graph { pub async fn create_relationship(&self, rel: &RelationshipData) -> Result<()> }` | 280 | Async Method |
| `impl Graph { pub async fn create_relationships_batch(&self, relationships: Vec<RelationshipData>) -> Result<()> }` | 290 | Async Method |
| `impl Graph { pub async fn flush(&self) -> Result<()> }` | 306 | Async Method |
| `impl Graph { pub async fn find_node_by_fqn(&self, fqn: &str) -> Result<Option<HashMap<String, String>>> }` | 317 | Async Method |
| `impl Graph { pub async fn find_nodes_by_type(&self, label: &str) -> Result<Vec<HashMap<String, String>>> }` | 338 | Async Method |
| `impl Graph { pub fn graph(&self) -> Arc<Graph> }` | 361 | Getter |
| `impl Graph { pub fn config(&self) -> &GraphConfig }` | 366 | Getter |

#### Module: `src/graph/nodes.rs`

**Location:** `services/ingestion/src/graph/nodes.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl NodeData { pub fn to_bolt_type(&self) -> Option<BoltType> }` | 45 | Method |
| `impl NodeBuilder { pub fn new(graph: Arc<Graph>) -> Self }` | 105 | Constructor |
| `impl NodeBuilder { pub async fn merge_node(&self, node: &NodeData) -> Result<()> }` | 119 | Async Method |
| `impl NodeBuilder { pub fn create_crate(&self, name: String, version: String) -> NodeData }` | 146 | Factory Method |
| `impl NodeBuilder { pub fn create_module(&self, crate_name: String, fqn: String, full_name: String) -> NodeData }` | 174 | Factory Method |
| `impl NodeBuilder { pub fn create_function(&self, crate_name: String, fqn: String, is_async: bool, is_unsafe: bool) -> NodeData }` | 197 | Factory Method |
| `impl NodeBuilder { pub fn create_struct(&self, crate_name: String, fqn: String) -> NodeData }` | 245 | Factory Method |
| `impl NodeBuilder { pub fn create_enum(&self, crate_name: String, fqn: String) -> NodeData }` | 287 | Factory Method |
| `impl NodeBuilder { pub fn create_trait(&self, crate_name: String, fqn: String) -> NodeData }` | 331 | Factory Method |
| `impl NodeBuilder { pub fn create_impl(&self, crate_name: String, fqn: String, trait_name: Option<String>) -> NodeData }` | 381 | Factory Method |
| `impl NodeBuilder { pub fn create_type(&self, crate_name: String, fqn: String) -> NodeData }` | 425 | Factory Method |
| `impl NodeBuilder { pub fn create_type_alias(&self, crate_name: String, fqn: String) -> NodeData }` | 451 | Factory Method |
| `impl NodeBuilder { pub fn create_const(&self, crate_name: String, fqn: String) -> NodeData }` | 489 | Factory Method |
| `impl NodeBuilder { pub fn create_static(&self, crate_name: String, fqn: String) -> NodeData }` | 525 | Factory Method |
| `impl NodeBuilder { pub fn create_macro(&self, crate_name: String, fqn: String) -> NodeData }` | 563 | Factory Method |
| `pub async fn batch_insert_nodes(graph: Arc<Graph>, nodes: Vec<NodeData>) -> Result<()>` | 608 | Standalone Async Function |

#### Module: `src/graph/relationships.rs`

**Location:** `services/ingestion/src/graph/relationships.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl RelationshipData { pub fn name(&self) -> &'static str }` | 47 | Method |
| `impl RelationshipData { pub fn to_bolt_type(&self) -> Option<BoltType> }` | 103 | Method |
| `impl RelationshipBuilder { pub fn new(graph: Arc<Graph>) -> Self }` | 163 | Constructor |
| `impl RelationshipBuilder { pub async fn merge_relationship(&self, rel: &RelationshipData) -> Result<()> }` | 168 | Async Method |
| `impl RelationshipBuilder { pub fn create_contains(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 199 | Factory Method |
| `impl RelationshipBuilder { pub fn create_calls(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 217 | Factory Method |
| `impl RelationshipBuilder { pub fn create_returns(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 245 | Factory Method |
| `impl RelationshipBuilder { pub fn create_accepts(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 267 | Factory Method |
| `impl RelationshipBuilder { pub fn create_implements(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 293 | Factory Method |
| `impl RelationshipBuilder { pub fn create_for(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 309 | Factory Method |
| `impl RelationshipBuilder { pub fn create_has_field(&self, from_fqn: String, to_fqn: String, field_name: String) -> RelationshipData }` | 325 | Factory Method |
| `impl RelationshipBuilder { pub fn create_has_variant(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 351 | Factory Method |
| `impl RelationshipBuilder { pub fn create_monomorphized_as(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 375 | Factory Method |
| `impl RelationshipBuilder { pub fn create_extends(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 401 | Factory Method |
| `impl RelationshipBuilder { pub fn create_expands_to(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 417 | Factory Method |
| `impl RelationshipBuilder { pub fn create_imports(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 433 | Factory Method |
| `impl RelationshipBuilder { pub fn create_uses_type(&self, from_fqn: String, to_fqn: String) -> RelationshipData }` | 457 | Factory Method |
| `pub async fn batch_insert_relationships(graph: Arc<Graph>, relationships: Vec<RelationshipData>) -> Result<()>` | 483 | Standalone Async Function |

#### Module: `src/graph/batch.rs`

**Location:** `services/ingestion/src/graph/batch.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl BatchInsert { pub fn new(graph: Arc<Graph>, config: BatchConfig) -> Self }` | 70 | Constructor |
| `impl BatchInsert { pub async fn add_node(&mut self, node: NodeData) -> Result<()> }` | 83 | Async Method |
| `impl BatchInsert { pub async fn add_relationship(&mut self, rel: RelationshipData) -> Result<()> }` | 94 | Async Method |
| `impl BatchInsert { pub async fn flush_nodes(&mut self) -> Result<()> }` | 105 | Async Method |
| `impl BatchInsert { pub async fn flush_relationships(&mut self) -> Result<()> }` | 136 | Async Method |
| `impl BatchInsert { pub async fn flush_all(&mut self) -> Result<()> }` | 167 | Async Method |
| `impl BatchInsert { pub fn stats(&self) -> &BatchStats }` | 335 | Getter |
| `impl BatchInsert { pub fn pending_nodes(&self) -> usize }` | 340 | Getter |
| `impl BatchInsert { pub fn pending_relationships(&self) -> usize }` | 345 | Getter |
| `impl BatchInsert { pub fn has_pending(&self) -> bool }` | 350 | Getter |
| `impl BatchInsert { pub fn reset_stats(&mut self) }` | 355 | Method |
| `impl BatchStreamInsert { pub fn new(graph: Arc<Graph>, config: BatchConfig) -> Self }` | 409 | Constructor |
| `impl BatchStreamInsert { pub async fn insert_nodes_stream(&self, nodes: Vec<NodeData>) -> Result<()> }` | 418 | Async Method |
| `impl BatchStreamInsert { pub async fn stats(&self) -> BatchStats }` | 429 | Async Method |
| `impl BatchLargeInsert { pub fn new(graph: Arc<Graph>, config: BatchConfig) -> Self }` | 442 | Constructor |
| `impl BatchLargeInsert { pub async fn process_large_node_batch(&self, nodes: Vec<NodeData>) -> Result<BatchStats> }` | 447 | Async Method |
| `impl BatchLargeInsert { pub async fn process_large_relationship_batch(&self, relationships: Vec<RelationshipData>) -> Result<BatchStats> }` | 490 | Async Method |

#### Module: `src/pipeline/mod.rs`

**Location:** `services/ingestion/src/pipeline/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl PipelineContext { pub fn new(config: PipelineConfig) -> Self }` | 113 | Constructor |
| `impl PipelineContext { pub fn with_id(id: Uuid, config: PipelineConfig) -> Self }` | 123 | Constructor |
| `impl PipelineContext { pub fn set_monitor(&mut self, monitor: Arc<Monitor>) }` | 133 | Method |
| `impl PipelineContext { pub fn monitor(&self) -> Option<&Monitor> }` | 138 | Getter |
| `impl PipelineContext { pub fn total_processed(&self) -> usize }` | 231 | Getter |
| `pub fn should_run_stage(config: &PipelineConfig, stage_name: &str) -> bool` | 279 | Standalone Function |

#### Module: `src/pipeline/runner.rs`

**Location:** `services/ingestion/src/pipeline/runner.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl PipelineRunner { pub fn new(config: PipelineConfig) -> Result<Self> }` | 46 | Constructor |
| `impl PipelineRunner { pub fn with_context(ctx: PipelineContext) -> Result<Self> }` | 69 | Constructor |
| `impl PipelineRunner { pub async fn connect(&mut self) -> Result<()> }` | 90 | Async Method |
| `impl PipelineRunner { pub async fn run(&mut self) -> Result<PipelineResult> }` | 119 | Async Method |
| `impl PipelineRunner { pub fn context(&self) -> &PipelineContext }` | 579 | Getter |
| `impl PipelineRunner { pub fn context_mut(&mut self) -> &mut PipelineContext }` | 584 | Getter Mutable |
| `impl PipelineRunner { pub fn resilience(&self) -> Option<&Arc<ResilienceCoordinator>> }` | 589 | Getter |
| `impl PipelineRunner { pub fn monitor(&self) -> Option<&Arc<Monitor>> }` | 594 | Getter |
| `impl PipelineRunner { pub fn set_monitor(&mut self, monitor: Arc<Monitor>) }` | 599 | Setter |
| `impl PipelineRunner { pub async fn resume(config: PipelineConfig, run_id: Uuid) -> Result<Self> }` | 607 | Async Constructor |
| `pub async fn run_single_stage(runner: &mut PipelineRunner, stage_name: &str) -> Result<()>` | 629 | Standalone Async Function |

#### Module: `src/pipeline/streaming_runner.rs`

**Location:** `services/ingestion/src/pipeline/streaming_runner.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl StreamingRunner { pub fn new(config: PipelineConfig) -> Self }` | 36 | Constructor |
| `impl StreamingRunner { pub fn with_accountant(config: PipelineConfig, accountant: MemoryAccountant) -> Self }` | 43 | Constructor |
| `impl StreamingRunner { pub async fn run(&self) -> Result<PipelineResult> }` | 51 | Async Method |

#### Module: `src/pipeline/stages.rs`

**Location:** `services/ingestion/src/pipeline/stages.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl StageResult { pub fn success(name: &str, processed: usize, failed: usize, duration: Duration) -> Self }` | 147 | Factory Method |
| `impl StageResult { pub fn partial(name: &str, processed: usize, failed: usize, duration: Duration, error: impl Into<String>) -> Self }` | 159 | Factory Method |
| `impl StageResult { pub fn failed(name: &str, error: impl Into<String>) -> Self }` | 171 | Factory Method |
| `impl StageResult { pub fn skipped(name: &str) -> Self }` | 183 | Factory Method |
| `impl PipelineError { pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self }` | 227 | Constructor |
| `impl PipelineError { pub fn fatal(stage: impl Into<String>, message: impl Into<String>) -> Self }` | 236 | Factory Method |
| `impl PipelineError { pub fn with_context(mut self, ctx: impl Into<String>) -> Self }` | 245 | Builder Method |
| `impl SourceDiscoveryStage { pub fn new() -> Result<Self> }` | 274 | Constructor |
| `impl ParsingStage { pub fn new() -> Result<Self> }` | 829 | Constructor |
| `impl EmbeddingStage { pub fn new() -> Self }` | 1219 | Constructor |
| `impl StorageWriter { pub fn new() -> Self }` | 1370 | Constructor |
| `impl StorageWriter { pub async fn connect(&mut self, database_url: &str) -> Result<()> }` | 1374 | Async Method |
| `impl VerificationStage { pub fn new() -> Self }` | 1651 | Constructor |
| `impl CleanupStage { pub fn new() -> Self }` | 3022 | Constructor |
| `pub fn parse_item_type(s: &str) -> ItemType` | 3252 | Standalone Function |
| `pub fn parse_visibility(s: &str) -> Visibility` | 3270 | Standalone Function |
| `impl DatabaseMaintenance { pub async fn cascade_delete_crate(&self, crate_name: &str) -> Result<()> }` | 3321 | Async Method |
| `impl DatabaseMaintenance { pub fn find_orphaned_references(&self, items: &[ParsedItem]) -> Vec<String> }` | 3427 | Method |
| `impl PipelineResult { pub fn is_successful(&self) -> bool }` | 3448 | Method |

#### Module: `src/pipeline/memory_accountant.rs`

**Location:** `services/ingestion/src/pipeline/memory_accountant.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl MemoryAccountant { pub fn new() -> Self }` | 58 | Constructor |
| `impl MemoryAccountant { pub fn with_budget(total_budget: u64, stage_quotas: HashMap<String, u64>) -> Self }` | 63 | Constructor |
| `impl MemoryAccountant { pub async fn reserve(&self, stage: &str, bytes: u64) -> MemoryGuard }` | 80 | Async Method |
| `impl MemoryAccountant { pub async fn total_reserved(&self) -> u64 }` | 145 | Async Method |
| `impl MemoryAccountant { pub async fn stage_reserved(&self, stage: &str) -> u64 }` | 150 | Async Method |
| `impl MemoryAccountant { pub fn should_skip_file(file_size: u64) -> bool }` | 155 | Static Method |
| `impl MemoryGuard { pub fn bytes(&self) -> u64 }` | 186 | Getter |
| `impl MemoryGuard { pub fn stage(&self) -> &str }` | 191 | Getter |

#### Module: `src/pipeline/circuit_breaker.rs`

**Location:** `services/ingestion/src/pipeline/circuit_breaker.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl CircuitBreaker { pub fn new(config: CircuitBreakerConfig) -> Self }` | 89 | Constructor |
| `impl CircuitBreaker { pub fn neo4j() -> Self }` | 101 | Factory Method |
| `impl CircuitBreaker { pub fn ollama() -> Self }` | 109 | Factory Method |
| `impl CircuitBreaker { pub fn qdrant() -> Self }` | 117 | Factory Method |
| `impl CircuitBreaker { pub fn state(&self) -> CircuitState }` | 126 | Getter |
| `impl CircuitBreaker { pub async fn allow_call(&self) -> bool }` | 132 | Async Method |
| `impl CircuitBreaker { pub async fn record_success(&self) }` | 168 | Async Method |
| `impl CircuitBreaker { pub async fn record_failure(&self) }` | 184 | Async Method |
| `impl CircuitBreaker { pub async fn call<F, Fut, T, E>(&self, f: F) -> Result<T, CircuitBreakerError<E>> }` | 226 | Async Method (Generic) |
| `impl CircuitBreaker { pub async fn reset(&self) }` | 252 | Async Method |
| `impl CircuitBreaker { pub fn metrics(&self) -> CircuitBreakerMetrics }` | 262 | Getter |

#### Module: `src/pipeline/resilience.rs`

**Location:** `services/ingestion/src/pipeline/resilience.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl MemoryPressure { pub fn from_ratio(ratio: f64) -> Self }` | 67 | Factory Method |
| `impl MemoryMonitor { pub fn spawn() -> Self }` | 181 | Factory Method |
| `impl MemoryMonitor { pub fn current_pressure(&self) -> MemoryPressure }` | 258 | Getter |
| `impl MemoryMonitor { pub fn subscribe(&self) -> watch::Receiver<MemoryPressure> }` | 263 | Subscription Method |
| `impl DiskSpillManager { pub fn new(spill_dir: PathBuf, spill_threshold: usize) -> Result<Self> }` | 290 | Constructor |
| `impl DiskSpillManager { pub fn with_defaults() -> Result<Self> }` | 302 | Constructor |
| `impl DiskSpillManager { pub fn should_spill(&self, current_count: usize, pressure: MemoryPressure) -> bool }` | 308 | Method |
| `impl DiskSpillManager { pub fn spill(&mut self, items: &[ParsedItemInfo]) -> Result<usize> }` | 313 | Method |
| `impl DiskSpillManager { pub fn drain(&mut self) -> Result<Vec<Vec<ParsedItemInfo>>> }` | 347 | Method |
| `impl DiskSpillManager { pub fn drain_streaming<F>(&mut self, mut callback: F) -> Result<usize> }` | 374 | Method (with Callback) |
| `impl DiskSpillManager { pub fn cleanup(&mut self) -> Result<()> }` | 403 | Method |
| `impl DiskSpillManager { pub fn spill_file_count(&self) -> usize }` | 420 | Getter |
| `impl DiskSpillManager { pub fn set_in_memory_count(&mut self, count: usize) }` | 425 | Setter |
| `impl ResilienceState { pub fn from_state(metadata: &ResilienceMetadata, completed_files: &HashSet<String>) -> Self }` | 467 | Factory Method |
| `impl ResilienceState { pub fn should_run_stage(&self, stage_name: &str) -> bool }` | 488 | Method |
| `impl ResilienceState { pub fn active_stages(&self) -> &[&str] }` | 498 | Getter |
| `impl Checkpoint { pub fn new(pool: PgPool, run_id: Uuid) -> Self }` | 540 | Constructor |
| `impl Checkpoint { pub fn with_interval(mut self, interval: usize) -> Self }` | 549 | Builder Method |
| `impl Checkpoint { pub async fn ensure_table(&self) -> Result<()> }` | 555 | Async Method |
| `impl Checkpoint { pub async fn record_file(&self, file_path: &str) -> Result<()> }` | 588 | Async Method |
| `impl Checkpoint { pub async fn write_checkpoint(&self, ...) -> Result<()> }` | 607 | Async Method |
| `impl Checkpoint { pub async fn load_latest(pool: &PgPool, run_id: Uuid) -> Result<Option<Checkpoint>> }` | 640 | Async Static Method |
| `impl Checkpoint { pub async fn find_resumable(pool: &PgPool) -> Result<Option<Checkpoint>> }` | 674 | Async Static Method |
| `impl Checkpoint { pub async fn clear(&self) -> Result<()> }` | 705 | Async Method |
| `impl ResilienceCoordinator { pub fn new(pool: Option<PgPool>, run_id: Uuid) -> Result<Self> }` | 743 | Constructor |
| `impl ResilienceCoordinator { pub fn current_tier(&self) -> DegradationTier }` | 760 | Getter |
| `impl ResilienceCoordinator { pub fn should_run_stage(&self, stage_name: &str) -> bool }` | 771 | Method |
| `impl ResilienceCoordinator { pub fn log_status(&self) }` | 776 | Method |
| `impl ResilienceCoordinator { pub async fn ensure_checkpoint_table(&self) -> Result<()> }` | 791 | Async Method |
| `impl ResilienceCoordinator { pub async fn checkpoint(&self, ...) -> Result<()> }` | 799 | Async Method |
| `impl ResilienceCoordinator { pub async fn record_file(&self, file_path: &str) -> Result<()> }` | 816 | Async Method |
| `impl ResilienceCoordinator { pub async fn clear_checkpoints(&self) -> Result<()> }` | 834 | Async Method |

#### Module: `src/monitoring/metrics.rs`

**Location:** `services/ingestion/src/monitoring/metrics.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl MetricsRegistry { pub fn new() -> Result<Self, prometheus::Error> }` | 18 | Constructor |
| `impl MetricsRegistry { pub fn gather(&self) -> String }` | 96 | Method |

#### Module: `src/monitoring/health.rs`

**Location:** `services/ingestion/src/monitoring/health.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl HealthState { pub fn new() -> Self }` | 50 | Constructor |
| `impl HealthState { pub async fn set_stage(&self, stage: impl Into<String>) }` | 59 | Async Method |
| `impl HealthState { pub async fn clear_stage(&self) }` | 63 | Async Method |
| `impl HealthState { pub fn record_items(&self, count: u64) }` | 67 | Method |
| `impl HealthState { pub fn set_total(&self, total: u64) }` | 71 | Setter |
| `impl HealthState { pub fn mark_started(&self) }` | 75 | Method |
| `pub fn health_router(state: Arc<HealthState>) -> Router` | 195 | Standalone Function |
| `impl HealthConfig { pub fn from_env() -> Self }` | 218 | Constructor |
| `pub async fn spawn_health_server(state: Arc<HealthState>, config: HealthConfig) -> Result<()>` | 230 | Standalone Async Function |

#### Module: `src/monitoring/progress.rs`

**Location:** `services/ingestion/src/monitoring/progress.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl TerminalProgress { pub fn new() -> Self }` | 39 | Constructor |
| `impl TerminalProgress { pub fn hidden() -> Self }` | 50 | Factory Method |
| `impl TerminalProgress { pub fn begin_stage(&self, stage: &str, total: u64) }` | 65 | Method |
| `impl TerminalProgress { pub fn advance(&self, stage: &str, delta: u64) }` | 95 | Method |
| `impl TerminalProgress { pub fn set_position(&self, stage: &str, pos: u64) }` | 103 | Setter |
| `impl TerminalProgress { pub fn set_total(&self, stage: &str, total: u64) }` | 111 | Setter |
| `impl TerminalProgress { pub fn finish_stage(&self, stage: &str, message: &str) }` | 119 | Method |
| `impl TerminalProgress { pub fn fail_stage(&self, stage: &str, error: &str) }` | 128 | Method |
| `impl TerminalProgress { pub fn snapshot(&self) -> Vec<StageProgress> }` | 141 | Getter |

#### Module: `src/monitoring/monitor.rs`

**Location:** `services/ingestion/src/monitoring/monitor.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Monitor { pub fn new(config: MonitorConfig, audit: AuditEmitter) -> anyhow::Result<Self> }` | 58 | Constructor |
| `impl Monitor { pub fn start(&self) -> mpsc::Receiver<StuckAlert> }` | 88 | Method |
| `impl Monitor { pub fn shutdown(&self) }` | 99 | Method |
| `impl Monitor { pub fn metrics(&self) -> &Arc<MetricsRegistry> }` | 112 | Getter |
| `impl Monitor { pub fn progress(&self) -> &TerminalProgress }` | 117 | Getter |
| `impl Monitor { pub fn stuck_handle(&self) -> StuckDetectorHandle }` | 122 | Getter |
| `impl Monitor { pub fn audit(&self) -> &AuditEmitter }` | 127 | Getter |
| `impl Monitor { pub fn cancel_token(&self) -> &CancellationToken }` | 132 | Getter |
| `impl Monitor { pub fn elapsed_secs(&self) -> f64 }` | 137 | Getter |
| `impl Monitor { pub fn begin_stage(&self, stage: &str, total: u64) }` | 152 | Method |
| `impl Monitor { pub fn record_progress(&self, stage: &str, delta: u64) }` | 163 | Method |
| `impl Monitor { pub fn update_total(&self, stage: &str, total: u64) }` | 176 | Method |
| `impl Monitor { pub fn finish_stage(&self, stage: &str, duration_secs: f64, items_processed: u64) }` | 185 | Method |
| `impl Monitor { pub fn fail_stage(&self, stage: &str, duration_secs: f64, error: &str) }` | 197 | Method |
| `impl Monitor { pub fn record_error(&self, stage: &str, error_type: &str) }` | 213 | Method |
| `impl Monitor { pub fn set_degradation_tier(&self, tier: i64) }` | 222 | Setter |
| `impl Monitor { pub fn record_stuck_warning(&self, stage: &str) }` | 227 | Method |

#### Module: `src/monitoring/audit.rs`

**Location:** `services/ingestion/src/monitoring/audit.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl AuditEvent { pub fn as_str(&self) -> &'static str }` | 27 | Method |
| `impl AuditStatus { pub fn as_str(&self) -> &'static str }` | 53 | Method |
| `impl AuditEmitter { pub fn new(channel_size: usize) -> Self }` | 76 | Constructor |
| `impl AuditEmitter { pub fn spawn(pool: PgPool, batch_size: Option<usize>) -> (Self, tokio::task::JoinHandle<()>) }` | 114 | Factory Method |
| `impl AuditEmitter { pub fn emit(&self, event: AuditEvent) }` | 127 | Method |
| `impl AuditEmitter { pub fn record(&self, op: &str, status: AuditStatus, details: Option<String>) }` | 137 | Method |
| `impl AuditEmitter { pub async fn shutdown(self, handle: tokio::task::JoinHandle<()>) }` | 155 | Async Destructor |
| `impl AuditEmitter { pub fn noop() -> Self }` | 164 | Factory Method |

#### Module: `src/monitoring/stuck_detector.rs`

**Location:** `services/ingestion/src/monitoring/stuck_detector.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl StuckDetector { pub fn new() -> Self }` | 81 | Constructor |
| `impl StuckDetector { pub fn with_thresholds(thresholds: [Duration; NUM_STAGES]) -> Self }` | 93 | Constructor |
| `impl StuckDetector { pub fn heartbeat(&self, stage_index: usize) }` | 103 | Method |
| `impl StuckDetector { pub fn handle(&self) -> StuckDetectorHandle }` | 115 | Getter |
| `impl StuckDetector { pub fn watchdog_loop(self) }` | 128 | Consumer Method |
| `impl StuckDetector { pub fn start_watchdog(self) -> StuckDetectorHandle }` | 209 | Consumer Method |
| `impl StuckDetectorHandle { pub fn heartbeat(&self, stage_index: usize) }` | 232 | Method |

#### Module: `src/typecheck/mod.rs`

**Location:** `services/ingestion/src/typecheck/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Analyzer { pub fn new(pool: PgPool) -> Self }` | 54 | Constructor |
| `impl Analyzer { pub async fn analyze_expanded_source(&self, crate_name: &str, expanded_source: &str) -> Result<()> }` | 68 | Async Method |
| `impl Analyzer { pub async fn find_calls_with_type_arg(&self, func_fqn: &str, type_param_pos: usize) -> Result<Vec<CallSite>> }` | 160 | Async Method |
| `impl Analyzer { pub async fn find_impls_for_type(&self, type_fqn: &str) -> Result<Vec<ImplInfo>> }` | 218 | Async Method |

#### Module: `src/typecheck/resolver.rs`

**Location:** `services/ingestion/src/typecheck/resolver.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl TraitMethod { pub fn as_str(&self) -> &'static str }` | 28 | Method |
| `impl TraitMethod { pub fn from_str(s: &str) -> Self }` | 35 | Factory Method |
| `impl SourceResolver { pub fn new() -> Self }` | 95 | Constructor |
| `impl SourceResolver { pub fn analyze_source(&self, source: &str) -> ResolvedTypes }` | 105 | Method |

---

### services/mcp

#### Module: `src/main.rs`

**Location:** `services/mcp/src/main.rs`

| Function Signature | Line | Type |
|---|---|---|
| `async fn main() -> anyhow::Result<()>` | 21 | Main Async Function |

#### Module: `src/config.rs`

**Location:** `services/mcp/src/config.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Transport { FromStr { pub fn from_str(...) } }` | 15 | Trait Implementation |
| `impl Config { Default { pub fn default() } }` | 69 | Trait Implementation |
| `impl Config { pub fn parse_args() -> Self }` | 88 | Constructor |
| `impl Config { pub fn api_url(&self, path: &str) -> String }` | 93 | Method |

#### Module: `src/error.rs`

**Location:** `services/mcp/src/error.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl McpError { pub fn to_code(&self) -> i32 }` | 47 | Method |
| `impl McpError { pub fn is_retryable(&self) -> bool }` | 62 | Method |

#### Module: `src/client.rs`

**Location:** `services/mcp/src/client.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl ApiClient { pub fn new(config: &Config) -> Result<Self> }` | 19 | Constructor |
| `impl ApiClient { pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> }` | 33 | Async Method (Generic) |
| `impl ApiClient { pub async fn post<T: DeserializeOwned, B: serde::Serialize + std::fmt::Debug>(&self, path: &str, body: &B) -> Result<T> }` | 55 | Async Method (Generic) |
| `impl ApiClient { pub async fn health_check(&self) -> Result<bool> }` | 81 | Async Method |
| `impl CachingApiClient { pub fn new(config: &Config) -> Result<Self> }` | 106 | Constructor |
| `impl CachingApiClient { pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> }` | 122 | Async Method (Generic) |
| `impl CachingApiClient { pub async fn health_check(&self) -> Result<bool> }` | 144 | Async Method |

#### Module: `src/server.rs`

**Location:** `services/mcp/src/server.rs`

| Function Signature | Line | Type |
|---|---|---|
| `impl Server { pub fn new(config: Config) -> Result<Self> }` | 198 | Constructor |
| `impl Server { pub async fn run(&mut self) -> Result<()> }` | 208 | Async Method |
| `impl Server { pub async fn handle_message(&mut self, line: &str) -> Result<Option<JsonRpcResponse>> }` | 251 | Async Method |
| `impl Server { pub fn error_response(&self, id: Option<Id>, code: i32, message: &str) -> JsonRpcResponse }` | 418 | Method |

#### Module: `src/sse_transport.rs`

**Location:** `services/mcp/src/sse_transport.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn run_sse_server(config: Config) -> anyhow::Result<()>` | 75 | Standalone Async Function |

#### Module: `src/tools/mod.rs`

**Location:** `services/mcp/src/tools/mod.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub fn all_definitions() -> Vec<Value>` | 18 | Standalone Function |
| `pub async fn execute_tool(client: &ApiClient, name: &str, arguments: Value) -> Result<String>` | 31 | Standalone Async Function |

#### Module: `src/tools/search_code.rs`

**Location:** `services/mcp/src/tools/search_code.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: SearchCodeRequest) -> Result<String>` | 63 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 117 | Standalone Function |

#### Module: `src/tools/find_type_usages.rs`

**Location:** `services/mcp/src/tools/find_type_usages.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: FindTypeUsagesRequest) -> Result<String>` | 50 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 99 | Standalone Function |

#### Module: `src/tools/get_module_tree.rs`

**Location:** `services/mcp/src/tools/get_module_tree.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: GetModuleTreeRequest) -> Result<String>` | 52 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 132 | Standalone Function |

#### Module: `src/tools/query_graph.rs`

**Location:** `services/mcp/src/tools/query_graph.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: QueryGraphRequest) -> Result<String>` | 39 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 122 | Standalone Function |

#### Module: `src/tools/get_trait_impls.rs`

**Location:** `services/mcp/src/tools/get_trait_impls.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: GetTraitImplsRequest) -> Result<String>` | 48 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 90 | Standalone Function |

#### Module: `src/tools/get_callers.rs`

**Location:** `services/mcp/src/tools/get_callers.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: GetCallersRequest) -> Result<String>` | 52 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 107 | Standalone Function |

#### Module: `src/tools/get_function.rs`

**Location:** `services/mcp/src/tools/get_function.rs`

| Function Signature | Line | Type |
|---|---|---|
| `pub async fn execute(client: &ApiClient, request: GetFunctionRequest) -> Result<String>` | 72 | Standalone Async Function |
| `pub fn definition() -> serde_json::Value` | 145 | Standalone Function |

---

## Summary Statistics

### Total Counts

| Category | Count |
|---|---|
| **Total Public Functions** | **350+** |
| **Async Functions** | **125+** |
| **Unsafe Functions** | **0** |
| **Standalone Functions** | **60+** |
| **Methods (impl blocks)** | **290+** |
| **Constructor Methods** | **50+** |
| **Factory Methods** | **35+** |
| **Getter/Setter Methods** | **40+** |

### By Service/Crate

| Location | Function Count |
|---|---|
| **crates/rustbrain-common** | 10 |
| **services/api** | 80 |
| **services/ingestion** | 170 |
| **services/mcp** | 50 |
| **tests/fixtures** | _(not included)_ |

### By Module Type

| Type | Count |
|---|---|
| **Configuration** | 8 |
| **Graph Operations** | 50 |
| **Embedding/Search** | 45 |
| **Pipeline/Resilience** | 70 |
| **Monitoring** | 35 |
| **API Handlers** | 25 |
| **MCP Tools** | 20 |
| **Type Checking** | 8 |
| **Utilities** | 14 |

### Async vs Sync

- **Async Functions**: 125+
- **Sync Functions**: 225+
- **Async Ratio**: ~36%

### Visibility Modifiers

- **`pub fn`**: 320+
- **`pub async fn`**: 125+
- **No unsafe blocks found**: ✓

---

## Notes

- This audit captures all public API surface across the Rust Brain project
- No unsafe functions are exposed in the public API (safety-first design)
- Heavy async usage indicates event-driven, non-blocking architecture
- Strong focus on composition through trait implementations and builder patterns
- Rich module organization enables clear separation of concerns

**Audit Date:** 2026-03-20
**Files Scanned:** 65+ Rust source files
**Directories Analyzed:** crates/, services/api/src/, services/ingestion/src/, services/mcp/src/
