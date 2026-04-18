# E2E Test Suite — 10 Queries Spanning All Endpoint Classes

> Part of [RUSA-160](/RUSA/issues/RUSA-160) — Phase 3B.1 E2E Verification Protocol  
> Target: rust-brain self-ingested data (2,285 items)

---

## Test Suite Overview

This document defines 10 comprehensive E2E test queries covering:
- Semantic search (Qdrant) — 2 tests
- Graph queries (Neo4j) — 2 tests  
- Postgres queries (items, modules) — 2 tests
- Aggregate search (cross-DB) — 2 tests
- MCP tool invocation — 1 test
- Chat endpoint — 1 test

---

## Test Data Context

**Ingested Repository**: rust-brain (self-ingested)  
**Total Items**: ~2,285 functions, structs, traits, impls  
**Key Components to Test Against**:
- `services/api/src/handlers/` — 19 handler modules
- `services/mcp/src/tools/` — 15 MCP tools
- `crates/rustbrain-common/` — Shared utilities

---

## Test 1: Semantic Search — Find Vector Store Usage

**Endpoint**: `POST /tools/search_semantic`  
**Store**: Qdrant (vector search with embedding fallback)  
**Classification**: CLASS A (lookup/query)

### Input
```json
{
  "query": "How do I search for similar code using vector embeddings?",
  "limit": 5,
  "score_threshold": 0.7
}
```

### Expected Output
```json
{
  "results": [
    {
      "fqn": "rustbrain_api::handlers::search::search_semantic_handler",
      "name": "search_semantic_handler",
      "kind": "function",
      "file_path": "services/api/src/handlers/search.rs",
      "score": 0.92,
      "snippet": "pub async fn search_semantic_handler(...) -> Result<...>",
      "docstring": "Handler for semantic code search using Qdrant vector similarity"
    }
  ],
  "query": "How do I search for similar code using vector embeddings?",
  "total": 5
}
```

### Stores Exercised
- **Qdrant**: Vector similarity search on code embeddings
- **Postgres**: Keyword fallback if Qdrant unavailable

### Success Criteria
- [ ] Returns results with similarity scores ≥ 0.7
- [ ] Results contain FQN, file_path, line numbers
- [ ] At least 1 result from `services/api/src/handlers/search.rs`

---

## Test 2: Semantic Search — Error Handling Patterns

**Endpoint**: `POST /tools/search_semantic`  
**Store**: Qdrant  
**Classification**: CLASS A

### Input
```json
{
  "query": "error handling patterns in API handlers",
  "limit": 10,
  "crate_filter": "rustbrain-api"
}
```

### Expected Output
- Results from `services/api/src/errors.rs`
- Results from `services/api/src/handlers/` error handling
- All results have `crate_name` = "rustbrain-api"

### Stores Exercised
- **Qdrant**: Filtered vector search
- **Postgres**: Crate metadata lookup

---

## Test 3: Graph Query — Find Callers of search_semantic

**Endpoint**: `GET /tools/get_callers`  
**Store**: Neo4j (CALLS relationships)  
**Classification**: CLASS B (traversal)

### Input
```http
GET /tools/get_callers?fqn=rustbrain_api::handlers::search::search_semantic_handler&depth=2
```

### Expected Output
```json
{
  "fqn": "rustbrain_api::handlers::search::search_semantic_handler",
  "callers": [
    {
      "fqn": "rustbrain_api::routes::configure_routes",
      "name": "configure_routes",
      "file_path": "services/api/src/routes.rs",
      "line": 45,
      "depth": 1
    },
    {
      "fqn": "rustbrain_api::handlers::search::aggregate_search_handler",
      "name": "aggregate_search_handler", 
      "file_path": "services/api/src/handlers/search.rs",
      "line": 200,
      "depth": 2
    }
  ],
  "depth": 2
}
```

### Stores Exercised
- **Neo4j**: CALLS edge traversal
- **Postgres**: Item metadata enrichment

### Success Criteria
- [ ] Returns at least 1 caller
- [ ] Caller includes file_path and line number
- [ ] Depth parameter respected

---

## Test 4: Graph Query — Trait Implementations

**Endpoint**: `GET /tools/get_trait_impls`  
**Store**: Neo4j (IMPLEMENTS relationships)  
**Classification**: CLASS B

### Input
```http
GET /tools/get_trait_impls?trait_name=Serialize&limit=20
```

### Expected Output
```json
{
  "trait_name": "Serialize",
  "implementations": [
    {
      "impl_fqn": "impl Serialize for FunctionDetail",
      "type_name": "FunctionDetail",
      "file_path": "services/api/src/handlers/items.rs",
      "start_line": 34
    }
  ]
}
```

### Stores Exercised
- **Neo4j**: IMPLEMENTS edge query
- **Postgres**: Impl block metadata

### Success Criteria
- [ ] Returns implementations from `services/api/src/handlers/`
- [ ] Results include impl_fqn, type_name, file_path
- [ ] Limit parameter respected

---

## Test 5: Postgres Query — Get Function Details

**Endpoint**: `GET /tools/get_function`  
**Store**: PostgreSQL (items table)  
**Classification**: CLASS A

### Input
```http
GET /tools/get_function?fqn=rustbrain_api::handlers::items::get_function_handler
```

### Expected Output
```json
{
  "fqn": "rustbrain_api::handlers::items::get_function_handler",
  "name": "get_function_handler",
  "kind": "function",
  "visibility": "pub",
  "signature": "pub async fn get_function_handler(Query(params): Query<GetFunctionQuery>, State(state): State<Arc<AppState>>) -> Result<Json<FunctionDetail>, AppError>",
  "docstring": "Retrieves detailed information about a code item by its fully qualified name",
  "file_path": "services/api/src/handlers/items.rs",
  "start_line": 100,
  "end_line": 150,
  "module_path": "rustbrain_api::handlers::items",
  "crate_name": "rustbrain-api",
  "body_source": "...",
  "callers": [...],
  "callees": [...]
}
```

### Stores Exercised
- **Postgres**: Full item metadata lookup
- **Neo4j**: Call graph enrichment (callers/callees)

### Success Criteria
- [ ] Returns complete FunctionDetail
- [ ] Includes source body
- [ ] Includes docstring
- [ ] Callers and callees populated from Neo4j

---

## Test 6: Postgres Query — Module Tree

**Endpoint**: `GET /tools/get_module_tree`  
**Store**: PostgreSQL + Neo4j  
**Classification**: CLASS B

### Input
```http
GET /tools/get_module_tree?crate_name=rustbrain-api
```

### Expected Output
```json
{
  "crate_name": "rustbrain-api",
  "modules": [
    {
      "name": "handlers",
      "path": "services/api/src/handlers",
      "submodules": ["search", "items", "graph", "chat", "execution"],
      "items": 156
    },
    {
      "name": "neo4j",
      "path": "services/api/src/neo4j",
      "items": 23
    }
  ],
  "total_modules": 12
}
```

### Stores Exercised
- **Postgres**: Module hierarchy from items table
- **Neo4j**: Module containment edges

### Success Criteria
- [ ] Returns hierarchical module structure
- [ ] Includes handlers submodule with items count
- [ ] Path points to correct source directory

---

## Test 7: Aggregate Search — Cross-DB Enrichment

**Endpoint**: `POST /tools/aggregate_search`  
**Stores**: Qdrant + Postgres + Neo4j  
**Classification**: CLASS C (multi-source aggregation)

### Input
```json
{
  "query": "database connection pooling implementation",
  "limit": 5,
  "include_graph": true,
  "include_source": true
}
```

### Expected Output
```json
{
  "query": "database connection pooling implementation",
  "results": [
    {
      "fqn": "rustbrain_api::state::AppState::new",
      "name": "new",
      "kind": "function",
      "file_path": "services/api/src/state.rs",
      "score": 0.89,
      "snippet": "pub async fn new() -> Result<Self> { ... pool.connect().await ... }",
      "source_body": "...",
      "callers": [...],
      "callees": [...],
      "similar_items": [...]
    }
  ],
  "sources_queried": ["qdrant", "postgres", "neo4j"],
  "total": 5
}
```

### Stores Exercised
- **Qdrant**: Initial vector search for candidates
- **Postgres**: Source body enrichment
- **Neo4j**: Call graph and similar items

### Success Criteria
- [ ] Results contain enriched data from all three stores
- [ ] Callers/callees included when include_graph=true
- [ ] Source body included when include_source=true

---

## Test 8: Aggregate Search — Pattern Discovery

**Endpoint**: `POST /tools/aggregate_search`  
**Stores**: Cross-DB  
**Classification**: CLASS C

### Input
```json
{
  "query": "MCP tool execution with error handling",
  "limit": 10,
  "include_graph": true
}
```

### Expected Output
- Results from `services/mcp/src/tools/`
- Results from `services/api/src/handlers/`
- Related items connected via call graph

### Stores Exercised
- All three stores for comprehensive results

---

## Test 9: MCP Tool Invocation — search_code

**Endpoint**: MCP SSE transport (`POST /tools/call` via SSE)  
**Tool**: `search_code`  
**Classification**: CLASS A (tool invocation)

### Input (MCP Protocol)
```json
{
  "jsonrpc": "2.0",
  "method": "tools/call",
  "params": {
    "name": "search_code",
    "arguments": {
      "query": "GET /health endpoint handler",
      "limit": 5
    }
  },
  "id": 1
}
```

### Expected Output
```json
{
  "jsonrpc": "2.0",
  "result": {
    "content": [
      {
        "type": "text",
        "text": "Found 3 results:\n\n1. rustbrain_api::handlers::health::health_handler\n   File: services/api/src/handlers/health.rs:15\n   Handler for GET /health endpoint\n\n2. rustbrain_api::handlers::health::metrics_handler\n   File: services/api/src/handlers/health.rs:45\n   Handler for GET /metrics endpoint"
      }
    ],
    "isError": false
  },
  "id": 1
}
```

### Stores Exercised
- **Qdrant**: Via API `/tools/search_semantic`
- **Postgres**: Item metadata lookup

### Success Criteria
- [ ] MCP protocol handshake successful
- [ ] Tool executes without error
- [ ] Results include FQN, file paths, line numbers
- [ ] Response format follows MCP spec

---

## Test 10: Chat Endpoint — Streaming Response

**Endpoint**: `POST /tools/chat` + `GET /tools/chat/stream`  
**Store**: Session state (Ephemeral)  
**Classification**: CLASS C (streaming/conversational)

### Input
```json
{
  "session_id": "ses_test_e2e_001",
  "message": "Explain how the semantic search handler works",
  "stream": true
}
```

### Expected Output (Initial)
```json
{
  "session_id": "ses_test_e2e_001",
  "message_id": "msg_001",
  "status": "streaming"
}
```

### Expected Output (SSE Stream)
```
event: message
data: {"role": "assistant", "content": "The semantic search handler..."}

event: tool_call
data: {"tool": "search_code", "args": {"query": "semantic search implementation"}}

event: message
data: {"role": "assistant", "content": "It uses Qdrant for vector similarity..."}

event: done
data: {"status": "completed"}
```

### Stores Exercised
- **Qdrant**: Via search_code tool invocation
- **Postgres**: Function lookups
- **Neo4j**: Relationship context

### Success Criteria
- [ ] Session created/retrieved
- [ ] SSE connection established
- [ ] Multiple event types received (message, tool_call)
- [ ] Stream terminates cleanly with "done" event

---

## Coverage Matrix

| Test | Qdrant | Postgres | Neo4j | MCP | Chat |
|------|--------|----------|-------|-----|------|
| 1. Semantic Search (Vector) | ✅ | 🔄 | ❌ | ❌ | ❌ |
| 2. Semantic Search (Filtered) | ✅ | 🔄 | ❌ | ❌ | ❌ |
| 3. Graph Callers | ❌ | 🔄 | ✅ | ❌ | ❌ |
| 4. Graph Trait Impls | ❌ | 🔄 | ✅ | ❌ | ❌ |
| 5. Postgres Get Function | 🔄 | ✅ | ✅ | ❌ | ❌ |
| 6. Postgres Module Tree | ❌ | ✅ | ✅ | ❌ | ❌ |
| 7. Aggregate Cross-DB | ✅ | ✅ | ✅ | ❌ | ❌ |
| 8. Aggregate Pattern | ✅ | ✅ | ✅ | ❌ | ❌ |
| 9. MCP Tool Invocation | ✅ | 🔄 | ❌ | ✅ | ❌ |
| 10. Chat Streaming | ✅ | 🔄 | ❌ | 🔄 | ✅ |

Legend: ✅ Primary | 🔄 Secondary | ❌ Not used

---

## Execution Instructions

### Prerequisites
```bash
# Verify all services healthy
docker ps --filter name=rustbrain --format "table {{.Names}}\t{{.Status}}" | grep healthy

# Expected: 8+ containers showing (healthy)
```

### Run Individual Tests
```bash
# Test 1: Semantic Search
curl -s -X POST http://localhost:8088/tools/search_semantic \
  -H "Content-Type: application/json" \
  -d '{"query": "vector embeddings search", "limit": 5}' | jq

# Test 3: Get Callers
curl -s "http://localhost:8088/tools/get_callers?fqn=rustbrain_api::handlers::search::search_semantic_handler&depth=2" | jq

# Test 5: Get Function
curl -s "http://localhost:8088/tools/get_function?fqn=rustbrain_api::handlers::items::get_function_handler" | jq
```

### Run via Shell Script
```bash
cd /home/jarnura/projects/rust-brain
./tests/integration/bin/run_e2e_suite.sh
```

### Run via Playwright
```bash
cd /home/jarnura/projects/rust-brain/e2e
npx playwright test e2e_suite.spec.js
```

---

## Expected Results Summary

| Metric | Target | Measurement |
|--------|--------|-------------|
| All 10 tests pass | 100% | Automated assertion count |
| Average response time | < 2s | API latency (p95) |
| Semantic search accuracy | > 80% | Manual relevance check |
| Cross-DB consistency | 100% | FQN matches across stores |
| MCP tool success rate | 100% | Zero tool errors |
| Stream stability | 100% | No SSE disconnects |

---

## Troubleshooting

### Issue: Semantic search returns empty results
**Check**: Qdrant container health and index status
```bash
curl http://localhost:6333/collections/functions | jq '.result.points_count'
```

### Issue: Graph queries return no callers
**Check**: Neo4j CALLS edges exist
```bash
curl -u neo4j:rustbrain_dev_2024 \
  -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH ()-[r:CALLS]->() RETURN count(r)"}]}' \
  http://localhost:7474/db/neo4j/tx/commit
```

### Issue: MCP tools not responding
**Check**: MCP-SSE connectivity
```bash
curl http://localhost:3001/sse
# Should return SSE headers
```

---

## Maintenance Notes

- Update expected outputs when codebase changes significantly
- Re-run suite after any handler or tool modifications
- Monitor test execution times for performance regressions
- File issues for flaky tests with environment details

---

**Document Version**: 1.0  
**Created**: 2026-04-09  
**Author**: QA Lead  
**Reviewed By**: [Pending]
