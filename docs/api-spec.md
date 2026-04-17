# rust-brain API Specification

REST API for the rust-brain code intelligence platform. All endpoints are served on port 8088 (configurable via `API_PORT` in `.env`).

**Base URL:** `http://localhost:8088`

## Endpoints Overview

### Code Intelligence Tools

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/tools/search_semantic` | Natural language code search |
| `POST` | `/tools/aggregate_search` | Cross-database aggregated search (Qdrant + Postgres + Neo4j) |
| `GET` | `/tools/get_function` | Full function details with source |
| `GET` | `/tools/get_callers` | Direct and transitive callers |
| `GET` | `/tools/get_trait_impls` | All implementations of a trait |
| `GET` | `/tools/find_usages_of_type` | Where a type is used |
| `GET` | `/tools/get_module_tree` | Module hierarchy |
| `POST` | `/tools/query_graph` | Raw Cypher queries |
| `GET` | `/tools/find_calls_with_type` | Call sites with specific type argument (turbofish) |
| `GET` | `/tools/find_trait_impls_for_type` | All trait implementations for a given type |

### Chat

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/tools/chat` | Send chat message (synchronous) |
| `GET` | `/tools/chat/stream` | SSE streaming chat responses |
| `POST` | `/tools/chat/send` | Send message to existing stream |

### Chat Sessions

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/tools/chat/sessions` | Create a new chat session |
| `GET` | `/tools/chat/sessions` | List all chat sessions |
| `GET` | `/tools/chat/sessions/:id` | Get a specific session |
| `DELETE` | `/tools/chat/sessions/:id` | Delete a session |
| `POST` | `/tools/chat/sessions/:id/fork` | Fork a session |
| `POST` | `/tools/chat/sessions/:id/abort` | Abort an active session |

### Artifacts

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/api/artifacts` | Create a new artifact |
| `GET` | `/api/artifacts` | List artifacts (with filters) |
| `GET` | `/api/artifacts/:id` | Get artifact by ID |
| `PUT` | `/api/artifacts/:id` | Update artifact status/confidence |

### Tasks

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/api/tasks` | Create a new task |
| `GET` | `/api/tasks` | List tasks (with filters) |
| `GET` | `/api/tasks/:id` | Get task by ID |
| `PUT` | `/api/tasks/:id` | Update task status |

### System

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/health` | Service health check |
| `GET` | `/health/consistency` | Cross-store consistency health |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/api/snapshot` | Snapshot metadata |
| `GET` | `/api/ingestion/progress` | Ingestion pipeline progress |
| `GET` | `/api/consistency` | Cross-store consistency check |
| `GET` | `/playground/*` | Playground static files |

### Documentation Search

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/tools/search_docs` | Semantic search over documentation files |

### Workspaces

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `POST` | `/workspaces` | Create workspace from GitHub repo |
| `GET` | `/workspaces` | List all workspaces |
| `GET` | `/workspaces/:id` | Get workspace details |
| `DELETE` | `/workspaces/:id` | Archive workspace |
| `GET` | `/workspaces/:id/files` | List workspace file tree |
| `POST` | `/workspaces/:id/execute` | Start multi-agent execution |
| `GET` | `/workspaces/:id/executions` | List executions for workspace |
| `GET` | `/workspaces/:id/stream` | SSE stream of workspace events |
| `GET` | `/workspaces/:id/diff` | Get uncommitted changes |
| `POST` | `/workspaces/:id/commit` | Commit changes |
| `POST` | `/workspaces/:id/reset` | Discard changes |

### Executions

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/executions/:id` | Get execution status |
| `GET` | `/executions/:id/events` | SSE stream of agent events |

### Validator

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/validator/runs` | List validator runs for a PR |
| `GET` | `/validator/runs/:id` | Get validator run details |

### Benchmarker

| Method | Endpoint | Purpose |
|--------|----------|---------|
| `GET` | `/benchmarker/suites` | List eval suites |
| `GET` | `/benchmarker/runs` | List bench runs |
| `POST` | `/benchmarker/runs` | Trigger a new bench run |
| `GET` | `/benchmarker/runs/:id` | Get bench run details |

---

## POST /tools/search_semantic

Natural language search over the codebase using vector embeddings.

### Request

**Content-Type:** `application/json`

```json
{
  "query": "function that serializes data to JSON",
  "limit": 10,
  "filters": {
    "item_type": ["function", "struct"],
    "crate_name": "serde",
    "visibility": "pub"
  }
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `query` | string | Yes | - | Natural language query |
| `limit` | integer | No | 10 | Maximum results to return |
| `filters` | object | No | {} | Optional filters |
| `filters.item_type` | array | No | - | Filter by item types: `function`, `struct`, `enum`, `trait`, `impl` |
| `filters.crate_name` | string | No | - | Filter by source crate |
| `filters.visibility` | string | No | - | Filter by visibility: `pub`, `pub(crate)`, `private` |

### cURL Example

```bash
curl -X POST http://localhost:8088/tools/search_semantic \
  -H "Content-Type: application/json" \
  -d '{
    "query": "function that parses JSON string into struct",
    "limit": 5
  }'
```

### Response Schema

```json
{
  "results": [
    {
      "fqn": "serde_json::from_str",
      "name": "from_str",
      "item_type": "function",
      "visibility": "pub",
      "signature": "pub fn from_str<'a, T>(s: &'a str) -> Result<T> where T: Deserialize<'a>",
      "doc_comment": "Deserialize an instance of type T from a string of JSON text.",
      "file_path": "serde_json/src/read.rs",
      "start_line": 45,
      "end_line": 62,
      "score": 0.92
    }
  ],
  "query_time_ms": 23
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Invalid query or missing required field |
| `500` | Embedding service error (check Ollama) |
| `503` | Qdrant unavailable |

---

## GET /tools/get_function

Retrieve full details for a specific function by its fully qualified name.

### Request

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `fqn` | string | Yes | Fully qualified name of the function |
| `include_body` | boolean | No | Include function body source (default: `true`) |
| `include_callers` | boolean | No | Include direct callers (default: `false`) |

### cURL Example

```bash
curl "http://localhost:8088/tools/get_function?fqn=serde_json::from_str&include_body=true"
```

### Response Schema

```json
{
  "fqn": "serde_json::from_str",
  "name": "from_str",
  "visibility": "pub",
  "signature": "pub fn from_str<'a, T>(s: &'a str) -> Result<T> where T: Deserialize<'a>",
  "doc_comment": "Deserialize an instance of type T from a string of JSON text.",
  "file_path": "serde_json/src/read.rs",
  "start_line": 45,
  "end_line": 62,
  "generic_params": ["T"],
  "attributes": [],
  "body_source": "pub fn from_str<'a, T>(s: &'a str) -> Result<T>\nwhere\n    T: Deserialize<'a>,\n{\n    from_trait(Read::new(s))\n}",
  "parameters": [
    {"name": "s", "type": "&'a str", "position": 0}
  ],
  "return_type": "Result<T>",
  "callers": [
    {
      "fqn": "my_app::config::load_config",
      "file_path": "src/config.rs",
      "line": 23
    }
  ]
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `fqn` parameter |
| `404` | Function not found |
| `500` | Database error |

---

## GET /tools/get_callers

Find all functions that call the specified function, including transitive callers.

### Request

**Query Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `fqn` | string | Yes | - | Fully qualified name of the callee |
| `depth` | integer | No | 1 | Call depth (1=direct, 2=transitive, etc.) |
| `include_monomorphized` | boolean | No | true | Include monomorphized call sites |

### cURL Example

```bash
curl "http://localhost:8088/tools/get_callers?fqn=serde_json::from_str&depth=2"
```

### Response Schema

```json
{
  "callee": "serde_json::from_str",
  "callers": [
    {
      "fqn": "my_app::config::load_config",
      "name": "load_config",
      "file_path": "src/config.rs",
      "line": 23,
      "depth": 1,
      "concrete_types": null
    },
    {
      "fqn": "my_app::main",
      "name": "main",
      "file_path": "src/main.rs",
      "line": 15,
      "depth": 2,
      "concrete_types": null
    }
  ],
  "total_count": 2,
  "max_depth": 2
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `fqn` parameter |
| `404` | Function not found |
| `500` | Graph traversal error |

---

## GET /tools/get_trait_impls

List all implementations of a specific trait.

### Request

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `trait_name` | string | Yes | Fully qualified name of the trait |
| `include_methods` | boolean | No | Include implemented methods (default: `false`) |

### cURL Example

```bash
curl "http://localhost:8088/tools/get_trait_impls?trait_name=serde::Serialize&include_methods=true"
```

### Response Schema

```json
{
  "trait": "serde::Serialize",
  "implementations": [
    {
      "impl_fqn": "my_app::models::User_impl_Serialize",
      "self_type": "my_app::models::User",
      "file_path": "src/models/user.rs",
      "start_line": 12,
      "methods": [
        {
          "name": "serialize",
          "signature": "fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer"
        }
      ]
    },
    {
      "impl_fqn": "my_app::models::Config_impl_Serialize",
      "self_type": "my_app::models::Config",
      "file_path": "src/models/config.rs",
      "start_line": 8,
      "methods": []
    }
  ],
  "total_count": 2
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `trait_name` parameter |
| `404` | Trait not found |
| `500` | Graph query error |

---

## GET /tools/find_usages_of_type

Find all locations where a specific type is used.

### Request

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `type_name` | string | Yes | Fully qualified name of the type |
| `usage_type` | string | No | Filter: `parameter`, `return`, `field`, `all` (default: `all`) |

### cURL Example

```bash
curl "http://localhost:8088/tools/find_usages_of_type?type_name=my_app::models::User"
```

### Response Schema

```json
{
  "type": "my_app::models::User",
  "usages": [
    {
      "fqn": "my_app::handlers::create_user",
      "name": "create_user",
      "item_type": "function",
      "usage_type": "return",
      "file_path": "src/handlers.rs",
      "line": 45
    },
    {
      "fqn": "my_app::handlers::get_user",
      "name": "get_user",
      "item_type": "function",
      "usage_type": "return",
      "file_path": "src/handlers.rs",
      "line": 52
    },
    {
      "fqn": "my_app::models::Order",
      "name": "user",
      "item_type": "struct_field",
      "usage_type": "field",
      "file_path": "src/models/order.rs",
      "line": 18
    }
  ],
  "total_count": 3
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `type_name` parameter |
| `404` | Type not found |
| `500` | Database error |

---

## GET /tools/get_module_tree

Get the module hierarchy for a crate.

### Request

**Query Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `crate` | string | Yes | Crate name |
| `include_items` | boolean | No | Include items in each module (default: `false`) |

### cURL Example

```bash
curl "http://localhost:8088/tools/get_module_tree?crate=serde_json&include_items=true"
```

### Response Schema

```json
{
  "crate": "serde_json",
  "root": {
    "path": "serde_json",
    "file_path": "src/lib.rs",
    "children": [
      {
        "path": "serde_json::value",
        "file_path": "src/value/mod.rs",
        "children": [
          {
            "path": "serde_json::value::raw",
            "file_path": "src/value/raw.rs",
            "children": [],
            "items": [
              {"fqn": "serde_json::value::raw::RawValue", "type": "struct"}
            ]
          }
        ],
        "items": [
          {"fqn": "serde_json::value::Value", "type": "enum"},
          {"fqn": "serde_json::value::to_value", "type": "function"}
        ]
      },
      {
        "path": "serde_json::de",
        "file_path": "src/de.rs",
        "children": [],
        "items": [
          {"fqn": "serde_json::de::from_str", "type": "function"},
          {"fqn": "serde_json::de::from_slice", "type": "function"}
        ]
      }
    ],
    "items": [
      {"fqn": "serde_json::to_string", "type": "function"},
      {"fqn": "serde_json::from_str", "type": "function"}
    ]
  }
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `crate` parameter |
| `404` | Crate not found |
| `500` | Database error |

---

## POST /tools/query_graph

Execute raw Cypher queries against the Neo4j graph database.

### Request

**Content-Type:** `application/json`

```json
{
  "query": "MATCH (f:Function)-[:CALLS]->(c:Function) WHERE f.fqn STARTS WITH $prefix RETURN f.fqn, c.fqn LIMIT 10",
  "parameters": {
    "prefix": "serde_json::"
  }
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | Cypher query string |
| `parameters` | object | No | Query parameters (recommended for safety) |

### cURL Example

```bash
curl -X POST http://localhost:8088/tools/query_graph \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (f:Function {visibility: \"pub\"}) RETURN f.fqn, f.signature LIMIT 5"
  }'
```

### Response Schema

```json
{
  "results": [
    {
      "f.fqn": "serde_json::from_str",
      "f.signature": "pub fn from_str<'a, T>(s: &'a str) -> Result<T>"
    },
    {
      "f.fqn": "serde_json::to_string",
      "f.signature": "pub fn to_string<T>(value: &T) -> Result<String>"
    }
  ],
  "query_time_ms": 12
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Invalid Cypher syntax |
| `403` | Query contains forbidden operations (WRITE operations) |
| `500` | Neo4j connection error |

---

## GET /health

Check the health status of the API service and its dependencies.

### cURL Example

```bash
curl http://localhost:8088/health
```

### Response Schema

```json
{
  "status": "healthy",
  "version": "0.1.0",
  "dependencies": {
    "postgres": "healthy",
    "neo4j": "healthy",
    "qdrant": "healthy",
    "ollama": "healthy"
  },
  "uptime_seconds": 3600
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `503` | Service unhealthy (check `dependencies` for details) |

---

## GET /tools/find_calls_with_type

Find all call sites where a specific type is used as a type argument (turbofish syntax).

### Request

**Query Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `type_name` | string | Yes | - | Name of the type to search for (e.g., `String`, `Vec`, `i32`) |
| `callee_name` | string | No | - | Optional callee function name filter (e.g., `parse`, `collect`) |
| `limit` | integer | No | 20 | Maximum results to return |

### cURL Example

```bash
curl "http://localhost:8088/tools/find_calls_with_type?type_name=String&callee_name=parse&limit=10"
```

---

## GET /tools/find_trait_impls_for_type

Find all trait implementations for a specific type (by self_type). Unlike `get_trait_impls` which finds implementations **by trait name**, this finds implementations **by the implementing type**.

### Request

**Query Parameters:**

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `type_name` | string | Yes | - | Name of the type to search for (e.g., `PaymentRequest`, `MyStruct`) |
| `limit` | integer | No | 20 | Maximum results to return |

### cURL Example

```bash
curl "http://localhost:8088/tools/find_trait_impls_for_type?type_name=PaymentRequest&limit=20"
```

---

## POST /tools/aggregate_search

Cross-database aggregated search that combines results from Qdrant (semantic), Postgres (metadata), and Neo4j (relationships) into a single rich response.

### Request

**Content-Type:** `application/json`

```json
{
  "query": "function that handles payment processing",
  "limit": 10
}
```

---

## POST /tools/chat

Send a chat message for AI-powered code exploration. Supports tool invocations against the knowledge base.

### Request

**Content-Type:** `application/json`

```json
{
  "message": "What does the process_payment function do?",
  "session_id": "optional-session-uuid"
}
```

---

## GET /tools/chat/stream

SSE (Server-Sent Events) endpoint for streaming chat responses. Connect via EventSource to receive real-time tool invocations and response text.

---

## GET /api/snapshot

Returns metadata about the currently loaded snapshot (if any).

### cURL Example

```bash
curl http://localhost:8088/api/snapshot
```

---

## GET /api/ingestion/progress

Returns the current ingestion pipeline progress and status.

### cURL Example

```bash
curl http://localhost:8088/api/ingestion/progress
```

---

## GET /metrics

Prometheus-compatible metrics endpoint.

### cURL Example

```bash
curl http://localhost:8088/metrics
```

---

## Authentication

Currently, the API operates without authentication for local development. For production deployments, consider adding:

1. **API Key Authentication** - Pass `X-API-Key` header
2. **JWT Tokens** - OAuth2/OIDC integration
3. **mTLS** - Client certificate authentication

## Rate Limiting

No rate limiting is currently implemented. Consider adding for production:

- Per-IP rate limits
- Per-API-key quotas
- Expensive query throttling (e.g., semantic search)

## Content Types

All endpoints accept and return `application/json`.

### Request Headers

```
Content-Type: application/json
Accept: application/json
```

### Response Headers

```
Content-Type: application/json
X-Request-Id: <uuid>
X-Response-Time: <ms>
```

## Pagination

For endpoints returning large result sets:

- Use `limit` and `offset` parameters
- Default limit is typically 10-50 items
- Maximum limit is 1000 items

## Error Response Format

All errors follow this structure:

```json
{
  "error": {
    "code": "FUNCTION_NOT_FOUND",
    "message": "Function with FQN 'unknown::function' not found",
    "details": {
      "fqn": "unknown::function"
    }
  },
  "request_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

## Common Error Codes

| Code | HTTP Status | Description |
|------|-------------|-------------|
| `INVALID_REQUEST` | 400 | Malformed request body or parameters |
| `MISSING_PARAMETER` | 400 | Required parameter not provided |
| `NOT_FOUND` | 404 | Requested resource not found |
| `FORBIDDEN_QUERY` | 403 | Cypher query contains forbidden operations |
| `INTERNAL_ERROR` | 500 | Internal server error |
| `SERVICE_UNAVAILABLE` | 503 | Dependency service unavailable |

---

## POST /api/artifacts

Create a new artifact in the inter-agent communication store.

### Request

**Content-Type:** `application/json`

```json
{
  "id": "art-001",
  "task_id": "task-001",
  "type": "prd",
  "producer": "planner",
  "status": "draft",
  "confidence": 1.0,
  "summary": {"title": "My PRD"},
  "payload": {"content": "..."}
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | Yes | - | Unique artifact ID |
| `task_id` | string | Yes | - | Parent task ID |
| `type` | string | Yes | - | Artifact type (e.g. `prd`, `plan`, `report`) |
| `producer` | string | Yes | - | Agent that produced this artifact |
| `status` | string | No | `draft` | `draft`, `final`, or `superseded` |
| `confidence` | float | No | `1.0` | Confidence score 0.0–1.0 |
| `summary` | object | Yes | - | Human-readable summary metadata |
| `payload` | object | Yes | - | Full artifact content |

### Response

Returns the created `Artifact` object (HTTP 200).

---

## GET /api/artifacts

List artifacts with optional filters.

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `task_id` | string | Filter by task ID |
| `type` | string | Filter by artifact type |
| `status` | string | Filter by status (`draft`, `final`, `superseded`) |
| `producer` | string | Filter by producing agent |
| `limit` | integer | Max results (default 20, max 100) |

### Example

```bash
curl "http://localhost:8088/api/artifacts?task_id=task-001&status=final"
```

---

## GET /api/artifacts/:id

Get a single artifact by ID.

### Example

```bash
curl "http://localhost:8088/api/artifacts/art-001"
```

---

## PUT /api/artifacts/:id

Update an artifact's status, superseded_by reference, or confidence.

### Request

```json
{
  "status": "final",
  "superseded_by": null,
  "confidence": 0.95
}
```

All fields are optional; only provided fields are updated.

---

## POST /api/tasks

Create a new orchestrator task.

### Request

```json
{
  "id": "task-001",
  "parent_id": null,
  "phase": "planning",
  "class": "A",
  "agent": "planner",
  "status": "pending",
  "inputs": [],
  "constraints": {},
  "acceptance": "PRD approved by CTO"
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `id` | string | Yes | - | Unique task ID |
| `parent_id` | string | No | null | Parent task ID |
| `phase` | string | Yes | - | Pipeline phase (e.g. `planning`, `implementation`) |
| `class` | string | Yes | - | Task class (e.g. `A`, `B`) |
| `agent` | string | Yes | - | Assigned agent name |
| `status` | string | No | `pending` | Initial status |
| `inputs` | array | No | `[]` | Input artifact IDs |
| `constraints` | object | No | `{}` | Task constraints |
| `acceptance` | string | No | null | Acceptance criteria |

---

## GET /api/tasks

List tasks with optional filters.

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | string | Filter by status |
| `agent` | string | Filter by agent name |
| `phase` | string | Filter by phase |
| `class` | string | Filter by class |
| `limit` | integer | Max results (default 20) |

### Example

```bash
curl "http://localhost:8088/api/tasks?agent=planner&status=pending"
```

---

## GET /api/tasks/:id

Get a single task by ID.

---

## PUT /api/tasks/:id

Update a task's status (with state transition validation), retry count, or error message.

### Request

```json
{
  "status": "in_progress",
  "retry_count": null,
  "error": null
}
```

Valid state transitions: `pending → in_progress → done | failed`. Any state can transition to `escalated`.

---

## POST /tools/search_docs

Semantic search over documentation files (not code). Queries the `doc_embeddings` Qdrant collection.

### Request

**Content-Type:** `application/json`

```json
{
  "query": "how to set up authentication",
  "limit": 10,
  "score_threshold": 0.7
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `query` | string | Yes | - | Natural language query |
| `limit` | integer | No | 10 | Maximum results to return |
| `score_threshold` | float | No | - | Minimum similarity score (0.0–1.0) |

### Response Schema

```json
{
  "query": "how to set up authentication",
  "total": 3,
  "results": [
    {
      "source_file": "docs/mcp-setup.md",
      "content_preview": "## Authentication Setup\n\nTo configure authentication...",
      "score": 0.89
    }
  ]
}
```

---

## GET /api/consistency

Cross-store consistency checker. Verifies data integrity across Postgres, Neo4j, and Qdrant.

### Query Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `crate` | string | No | - | Specific crate to check (checks all if omitted) |
| `detail` | string | No | `summary` | `summary` (counts only) or `full` (FQN sets) |

### cURL Example

```bash
curl "http://localhost:8088/api/consistency?detail=summary"
```

### Response Schema

```json
{
  "crate_name": "all",
  "timestamp": "2026-04-10T15:30:00Z",
  "store_counts": {
    "postgres": 284123,
    "neo4j": 365000,
    "qdrant": 284123
  },
  "discrepancies": null,
  "status": "consistent",
  "recommendation": "All stores are in sync."
}
```

---

## GET /health/consistency

Lightweight consistency health check for Prometheus monitoring. Returns aggregate status only.

### cURL Example

```bash
curl http://localhost:8088/health/consistency
```

### Response Schema

```json
{
  "status": "healthy",
  "postgres_count": 284123,
  "neo4j_count": 365000,
  "qdrant_count": 284123
}
```

---

## Workspace Management

Workspaces provide isolated, sandboxed environments for AI agents to read and modify code. Each workspace is backed by its own Docker volume and Postgres schema, enabling multi-tenant isolation.

### Workspace Lifecycle

```
pending → cloning → indexing → ready ⇄ error
                                    ↓
                                 archived
```

| Status | Description |
|--------|-------------|
| `pending` | Workspace record created; clone not yet started |
| `cloning` | Repository is being cloned from GitHub |
| `indexing` | Codebase is being indexed by the ingestion pipeline |
| `ready` | Fully indexed and available for queries and execution |
| `error` | An unrecoverable error occurred (clone, volume, or ingestion failure) |
| `archived` | Workspace has been deleted; all resources cleaned up |

### Workspace Data Model

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Unique workspace identifier |
| `name` | string | Human-readable name (defaults to repo slug) |
| `source_type` | string | Source type: `github` or `local` |
| `source_url` | string | GitHub HTTPS URL of the repository |
| `clone_path` | string? | Filesystem path to the cloned repo (set after clone) |
| `volume_name` | string? | Docker volume name (e.g. `rustbrain-ws-abc12345`) |
| `schema_name` | string? | Postgres schema name (e.g. `ws_abc123456789`) |
| `status` | string | Current lifecycle status (see table above) |
| `default_branch` | string? | Default branch name detected after clone |
| `github_auth_method` | string? | Auth method used: `pat`, `app`, or null (public) |
| `index_started_at` | datetime? | When indexing began |
| `index_completed_at` | datetime? | When indexing finished |
| `index_stage` | string? | Current ingestion stage name |
| `index_progress` | object? | Progress metadata from ingestion pipeline |
| `index_error` | string? | Error message if indexing failed |
| `created_at` | datetime | Timestamp of creation |
| `updated_at` | datetime | Timestamp of last update |

---

### POST /workspaces

Create a new workspace from a GitHub repository. Returns `202 Accepted` immediately; cloning and indexing happen asynchronously in the background.

The server validates that `github_url` starts with `https://github.com/` and contains at least an `owner/repo` path. If `name` is omitted, the repo slug (last path segment) is used.

### Request

**Content-Type:** `application/json`

```json
{
  "github_url": "https://github.com/juspay/hyperswitch",
  "name": "hyperswitch-main"
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `github_url` | string | Yes | - | GitHub HTTPS URL (`https://github.com/<owner>/<repo>`, `.git` suffix optional) |
| `name` | string | No | repo slug | Human-readable name for the workspace |

### cURL Example

```bash
curl -X POST http://localhost:8088/workspaces \
  -H "Content-Type: application/json" \
  -d '{
    "github_url": "https://github.com/juspay/hyperswitch",
    "name": "hyperswitch-main"
  }'
```

### Response Schema (202 Accepted)

```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "status": "cloning",
  "message": "Workspace created. Clone started in the background."
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Invalid `github_url` (must be `https://github.com/<owner>/<repo>`) or empty `name` |
| `500` | Database error creating workspace record |

---

### GET /workspaces

List all non-archived workspaces, ordered by creation time (newest first).

### cURL Example

```bash
curl http://localhost:8088/workspaces | jq .
```

### Response Schema

Returns an array of [`Workspace`](#workspace-data-model) objects.

```json
[
  {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "name": "hyperswitch-main",
    "source_type": "github",
    "source_url": "https://github.com/juspay/hyperswitch",
    "clone_path": "/tmp/rustbrain-clones/550e8400-e29b-41d4-a716-446655440000",
    "volume_name": "rustbrain-ws-550e8400e29b41d4a716446655440000",
    "schema_name": "ws_550e8400e29b",
    "status": "ready",
    "default_branch": "main",
    "github_auth_method": "pat",
    "index_started_at": "2026-04-10T10:01:00Z",
    "index_completed_at": "2026-04-10T10:05:00Z",
    "index_stage": null,
    "index_progress": null,
    "index_error": null,
    "created_at": "2026-04-10T10:00:00Z",
    "updated_at": "2026-04-10T10:05:00Z"
  }
]
```

---

### GET /workspaces/:id

Get a single workspace by ID.

### cURL Example

```bash
curl http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000 | jq .
```

### Response Schema

Returns a single [`Workspace`](#workspace-data-model) object.

### Error Codes

| Code | Description |
|------|-------------|
| `404` | Workspace not found |

---

### DELETE /workspaces/:id

Archive a workspace and clean up all associated resources. Returns `204 No Content` on success.

Cleanup is performed asynchronously in the background and includes:

1. Stop any running execution containers
2. Abort running executions in the database
3. Drop the per-workspace Postgres schema
4. Remove the Docker volume
5. Clean up the host clone directory
6. Archive the workspace record

If the workspace is already archived, returns `204` immediately.

### cURL Example

```bash
curl -X DELETE http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000
```

### Error Codes

| Code | Description |
|------|-------------|
| `404` | Workspace not found |

---

### GET /workspaces/:id/files

Return the file tree for a workspace as a recursive directory structure compatible with react-treeview. The root node represents the workspace clone directory.

Hidden entries (names starting with `.`) are excluded. Directories sort before files at each level.

### cURL Example

```bash
curl http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/files | jq .
```

### Response Schema

```json
{
  "name": "hyperswitch",
  "path": "",
  "is_dir": true,
  "children": [
    {
      "name": "src",
      "path": "src",
      "is_dir": true,
      "children": [
        {
          "name": "main.rs",
          "path": "src/main.rs",
          "is_dir": false
        }
      ]
    },
    {
      "name": "Cargo.toml",
      "path": "Cargo.toml",
      "is_dir": false
    }
  ]
}
```

> **Note:** The `children` field is omitted for file nodes (when `is_dir` is `false`).

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Workspace not yet cloned (status is not `ready`) |
| `404` | Workspace not found |
| `500` | Clone path does not exist on disk |

---

### POST /workspaces/:id/execute

Start a multi-agent execution in a workspace. Returns `202 Accepted` immediately; the orchestrator runs in the background.

The workspace must be in `ready` status and have a Docker volume attached.

### Request

**Content-Type:** `application/json`

```json
{
  "prompt": "Add unit tests for the payment processing module",
  "branch_name": "feature/payment-tests",
  "timeout_secs": 7200
}
```

### Parameters

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `prompt` | string | Yes | - | Natural-language task description (must not be empty) |
| `branch_name` | string | No | auto-generated | Git branch for commits |
| `timeout_secs` | integer | No | 7200 | Execution timeout in seconds (2 hours) |

### cURL Example

```bash
curl -X POST http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/execute \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "Add unit tests for the payment processing module",
    "branch_name": "feature/payment-tests"
  }'
```

### Response Schema (202 Accepted)

```json
{
  "id": "660e8400-e29b-41d4-a716-446655440000",
  "status": "running",
  "message": "Execution started. Stream events at GET /executions/{id}/events"
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Empty prompt, or workspace has no volume (not yet cloned) |
| `404` | Workspace not found |
| `500` | Database error creating execution |

---

### GET /workspaces/:id/executions

List all executions for a workspace, ordered by start time (newest first).

### cURL Example

```bash
curl http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/executions | jq .
```

### Response Schema

Returns an array of [`Execution`](#execution-data-model) objects.

```json
[
  {
    "id": "660e8400-e29b-41d4-a716-446655440000",
    "workspace_id": "550e8400-e29b-41d4-a716-446655440000",
    "prompt": "Add unit tests for the payment processing module",
    "branch_name": "feature/payment-tests",
    "session_id": "ses_abc123",
    "container_id": "abc123def456",
    "volume_name": "rustbrain-ws-550e8400e29b41d4a716446655440000",
    "opencode_endpoint": "http://rustbrain-exec-660e8400:4096",
    "workspace_path": "/workspace",
    "status": "completed",
    "agent_phase": "developing",
    "started_at": "2026-04-10T10:00:00Z",
    "completed_at": "2026-04-10T11:30:00Z",
    "diff_summary": {"files_changed": 3, "insertions": 45, "deletions": 12},
    "error": null,
    "timeout_config_secs": 7200,
    "container_expires_at": null
  }
]
```

---

### Execution Data Model

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Unique execution identifier |
| `workspace_id` | UUID | Parent workspace |
| `prompt` | string | The natural-language task description |
| `branch_name` | string? | Git branch for commits |
| `session_id` | string? | OpenCode session ID |
| `container_id` | string? | Docker container ID running OpenCode |
| `volume_name` | string? | Docker volume containing workspace source |
| `opencode_endpoint` | string? | OpenCode container URL |
| `workspace_path` | string? | Working directory inside the container |
| `status` | string | Execution status (see table below) |
| `agent_phase` | string? | Current agent phase (see table below) |
| `started_at` | datetime | When execution started |
| `completed_at` | datetime? | When execution finished |
| `diff_summary` | object? | Summary of code changes made |
| `error` | string? | Error message if failed |
| `timeout_config_secs` | integer | Configured timeout in seconds (default 7200) |
| `container_expires_at` | datetime? | When keep-alive expires for debugging |

### Execution Status Values

| Status | Description |
|--------|-------------|
| `running` | Agents actively working |
| `completed` | Finished successfully |
| `failed` | Terminated with error |
| `aborted` | Cancelled (by user reset or workspace archive) |
| `timeout` | Exceeded timeout limit |

### Agent Phase Values

| Phase | Description |
|-------|-------------|
| `orchestrating` | Orchestrator is dispatching sub-agents |
| `researching` | Research agent is exploring the codebase |
| `planning` | Planner agent is designing the approach |
| `developing` | Developer agent is writing code |

---

### GET /executions/:id

Get execution status and details.

### cURL Example

```bash
curl http://localhost:8088/executions/660e8400-e29b-41d4-a716-446655440000 | jq .
```

### Response Schema

Returns a single [`Execution`](#execution-data-model) object.

### Error Codes

| Code | Description |
|------|-------------|
| `404` | Execution not found |

---

### GET /executions/:id/events

SSE (Server-Sent Events) stream of agent events during execution. Polls Postgres every 500ms for new events. Terminates with a `done` event when the execution reaches a terminal state (`completed`, `failed`, `aborted`, `timeout`).

### cURL Example

```bash
curl -N http://localhost:8088/executions/660e8400-e29b-41d4-a716-446655440000/events
```

### SSE Event Format

Each event has:
- `id`: Sequential event ID (used for incremental polling)
- `event`: `agent_event` for data events, `done` for stream termination, `error` for errors
- `data`: JSON payload

### Agent Event Types

| Event Type | Description |
|------------|-------------|
| `reasoning` | Agent reasoning/thinking output |
| `tool_call` | Agent invoked a tool (read, write, search, etc.) |
| `file_edit` | Code change detected |
| `phase_change` | Agent phase transition (e.g. researching → planning) |
| `agent_dispatch` | Sub-agent spawned or transitioned |
| `error` | Error occurred during execution |
| `container_kept_alive` | Container kept alive for debugging |

### Example SSE Stream

```
event: agent_event
id: 1
data: {"id":1,"execution_id":"660e8400-...","event_type":"phase_change","content":{"phase":"researching"},"timestamp":"2026-04-10T10:00:01Z"}

event: agent_event
id: 2
data: {"id":2,"execution_id":"660e8400-...","event_type":"reasoning","content":{"text":"Let me look at the payment module..."},"timestamp":"2026-04-10T10:00:05Z"}

event: agent_event
id: 3
data: {"id":3,"execution_id":"660e8400-...","event_type":"tool_call","content":{"tool":"search_semantic","args":{"query":"payment processing"}},"timestamp":"2026-04-10T10:00:08Z"}

event: done
data: {"status":"completed"}
```

### Error Codes

| Code | Description |
|------|-------------|
| `404` | Execution not found |

---

### GET /workspaces/:id/stream

SSE stream combining workspace-level events with execution agent events. Requires an `execution_id` query parameter.

Sends a keepalive comment every 15 seconds and closes the stream automatically when the execution reaches a terminal state.

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `execution_id` | UUID | Yes | The execution whose events to stream |

### cURL Example

```bash
curl -N "http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/stream?execution_id=660e8400-e29b-41d4-a716-446655440000"
```

### SSE Event Format

Each `agent_event` carries:

```json
{
  "id": 42,
  "execution_id": "660e8400-e29b-41d4-a716-446655440000",
  "phase": "researching",
  "event_type": "reasoning",
  "content": {"text": "Analyzing the codebase..."},
  "ts": "2026-04-10T10:00:05Z"
}
```

The `phase` field is extracted from the event `content.phase` when present. A `done` event is emitted when the execution completes:

```
event: done
data: {"execution_id":"660e8400-...","status":"completed"}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Missing `execution_id` query parameter |
| `404` | Workspace or execution not found, or execution does not belong to this workspace |

---

### GET /workspaces/:id/diff

Get the unified git diff for uncommitted changes in a workspace. Runs `git diff HEAD` in the clone directory.

### cURL Example

```bash
curl http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/diff | jq .
```

### Response Schema

```json
{
  "patch": "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n fn main() {\n+    println!(\"hello\");\n }",
  "clean": false
}
```

| Field | Type | Description |
|-------|------|-------------|
| `patch` | string | Unified diff output from `git diff HEAD` (empty string if no changes) |
| `clean` | boolean | `true` when there are no staged or unstaged changes |

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Workspace not yet cloned |
| `404` | Workspace not found |
| `500` | `git diff` failed (not a git repo, etc.) |

---

### POST /workspaces/:id/commit

Stage all changes and commit in the workspace. Runs `git add -A` followed by `git commit -m <message>`. The commit author is set to `rustbrain <rustbrain@localhost>`.

Returns the short commit SHA (7 characters) on success.

### Request

**Content-Type:** `application/json`

```json
{
  "message": "Add unit tests for payment module"
}
```

### Parameters

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `message` | string | Yes | Commit message (must not be empty) |

### cURL Example

```bash
curl -X POST http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/commit \
  -H "Content-Type: application/json" \
  -d '{"message": "Add unit tests for payment module"}'
```

### Response Schema

```json
{
  "sha": "abc1234",
  "message": "Add unit tests for payment module"
}
```

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Empty message, workspace not yet cloned, or nothing to commit |
| `404` | Workspace not found |
| `500` | `git add` or `git commit` failed |

---

### POST /workspaces/:id/reset

Reset workspace to a clean state, discarding all uncommitted changes. Runs `git reset --hard HEAD` followed by `git clean -fd` to remove untracked files. Also aborts any running execution for the workspace.

### cURL Example

```bash
curl -X POST http://localhost:8088/workspaces/550e8400-e29b-41d4-a716-446655440000/reset \
  -H "Content-Type: application/json"
```

### Response Schema

```json
{
  "message": "Workspace reset to HEAD. All uncommitted changes discarded.",
  "head_sha": "abc1234"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | Human-readable confirmation |
| `head_sha` | string | The HEAD commit SHA after reset (short, 7 chars) |

### Error Codes

| Code | Description |
|------|-------------|
| `400` | Workspace not yet cloned |
| `404` | Workspace not found |
| `500` | `git reset`, `git clean`, or SHA read failed |

---

## Validator Endpoints

### GET /validator/runs

List validator runs for a specific PR.

### Query Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `repo` | string | Yes | GitHub repo in `owner/repo` form |
| `pr` | integer | Yes | Pull request number |

### Response Schema

```json
{
  "runs": [
    {
      "id": "770e8400-e29b-41d4-a716-446655440000",
      "repo": "juspay/hyperswitch",
      "pr_number": 42,
      "run_index": 1,
      "composite_score": 0.85,
      "pass": true,
      "inverted": false,
      "created_at": "2026-04-10T10:00:00Z"
    }
  ]
}
```

---

### GET /validator/runs/:id

Get full detail for a single validator run.

### Response Schema

```json
{
  "id": "770e8400-e29b-41d4-a716-446655440000",
  "repo": "juspay/hyperswitch",
  "pr_number": 42,
  "run_index": 1,
  "composite_score": 0.85,
  "pass": true,
  "inverted": false,
  "dimension_scores": {
    "correctness": 0.9,
    "completeness": 0.8,
    "code_quality": 0.85
  },
  "tokens_used": 15000,
  "cost_usd": 0.12,
  "created_at": "2026-04-10T10:00:00Z"
}
```

---

## Benchmarker Endpoints

### GET /benchmarker/suites

List all eval suites with case counts.

### Response Schema

```json
{
  "suites": [
    {
      "suite_name": "default",
      "case_count": 25
    }
  ]
}
```

---

### GET /benchmarker/runs

List bench runs with optional filters.

### Query Parameters

| Parameter | Type | Description |
|-----------|------|-------------|
| `suite` | string | Filter by suite name |
| `status` | string | Filter by status (`running`, `completed`, `failed`) |
| `limit` | integer | Max results (default 50, max 200) |

### Response Schema

```json
{
  "runs": [
    {
      "id": "880e8400-e29b-41d4-a716-446655440000",
      "suite_name": "default",
      "release_tag": "v0.3.0",
      "status": "completed",
      "total_cases": 25,
      "completed_cases": 25,
      "pass_count": 22,
      "pass_rate": 0.88,
      "mean_composite": 0.82,
      "total_cost_usd": 3.45,
      "started_at": "2026-04-10T10:00:00Z",
      "completed_at": "2026-04-10T12:00:00Z"
    }
  ]
}
```

---

### POST /benchmarker/runs

Trigger a new bench run.

### Request

**Content-Type:** `application/json`

```json
{
  "suite_name": "default",
  "release_tag": "v0.3.0"
}
```

---

### GET /benchmarker/runs/:id

Get full detail for a bench run including per-case results.

### Response Schema

```json
{
  "id": "880e8400-e29b-41d4-a716-446655440000",
  "suite_name": "default",
  "release_tag": "v0.3.0",
  "status": "completed",
  "total_cases": 25,
  "completed_cases": 25,
  "pass_count": 22,
  "pass_rate": 0.88,
  "mean_composite": 0.82,
  "total_cost_usd": 3.45,
  "started_at": "2026-04-10T10:00:00Z",
  "completed_at": "2026-04-10T12:00:00Z",
  "case_results": [
    {
      "id": "990e8400-e29b-41d4-a716-446655440000",
      "eval_case_id": "case-001",
      "validator_run_id": "770e8400-e29b-41d4-a716-446655440000",
      "run_index": 1,
      "composite": 0.85,
      "pass": true,
      "cost_usd": 0.12,
      "created_at": "2026-04-10T10:05:00Z"
    }
  ]
}
```
