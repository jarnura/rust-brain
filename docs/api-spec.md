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
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/api/snapshot` | Snapshot metadata |
| `GET` | `/api/ingestion/progress` | Ingestion pipeline progress |
| `GET` | `/playground/*` | Playground static files |

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
