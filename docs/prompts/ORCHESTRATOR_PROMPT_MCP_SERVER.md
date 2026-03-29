# Orchestration Agent Prompt: Implement rust-brain MCP Server

## Mission

Build a production-grade MCP (Model Context Protocol) server for the rust-brain code intelligence platform. This MCP server is a **thin translation layer** over the existing HTTP REST API (port 8088). It exposes all 7 code intelligence tools as MCP tools so that any MCP-compatible client (Claude Code, Cursor, Windsurf, custom agents) can discover and invoke them natively.

The HTTP API remains the backbone. The MCP server is a client-facing interface that calls the HTTP API internally.

---

## Context: What Already Exists

### Existing REST API (services/api on port 8088)
The API is built with Axum in Rust. It talks to 3 databases:
- **PostgreSQL** — raw source, extracted items, call sites, trait impls
- **Neo4j** — code relationship graph (CALLS, IMPLEMENTS, DEFINES, etc.)
- **Qdrant** — 768-dim vector embeddings for semantic search (via Ollama nomic-embed-text)

### 9 Tool Endpoints Already Working

| Endpoint | Method | MCP Tool Name | Purpose |
|----------|--------|---------------|---------|
| `/tools/search_semantic` | POST | `search_code` | Natural language semantic search over codebase |
| `/tools/get_function` | GET | `get_function` | Full function details by FQN |
| `/tools/get_callers` | GET | `get_callers` | Direct + transitive call graph |
| `/tools/get_trait_impls` | GET | `get_trait_impls` | All implementations of a trait |
| `/tools/find_usages_of_type` | GET | `find_type_usages` | Where a type is used |
| `/tools/get_module_tree` | GET | `get_module_tree` | Module hierarchy for a crate |
| `/tools/query_graph` | POST | `query_graph` | Raw read-only Cypher queries |
| `/tools/find_calls_with_type` | GET | `find_calls_with_type` | Find call sites with specific type argument (turbofish) |
| `/tools/find_trait_impls_for_type` | GET | `find_trait_impls_for_type` | All trait implementations for a type |

### Infrastructure
- Docker Compose orchestrates everything: Postgres, Neo4j, Qdrant, Ollama, Prometheus, Grafana
- All services on `rustbrain-net` (172.28.0.0/16)
- Monitoring via Prometheus + Grafana already configured
- Rust 1.85, async/await with Tokio

---

## Architecture Decision

```
MCP Clients (Claude Code, Cursor, custom agents)
    │
    │  stdio / SSE transport
    ▼
┌─────────────────────────────┐
│   rustbrain-mcp (new)       │
│   Rust binary               │
│                             │
│   - MCP protocol handler    │
│   - Tool schema definitions │
│   - HTTP client → API       │
│   - Input validation        │
│   - Response formatting     │
│   - Error mapping           │
└──────────┬──────────────────┘
           │ HTTP (reqwest)
           ▼
┌─────────────────────────────┐
│   rustbrain-api (existing)  │
│   Port 8088                 │
│   Axum REST server          │
└─────────────────────────────┘
```

**Key principle**: The MCP server holds ZERO business logic. It translates MCP tool calls to HTTP requests and MCP responses back to the client. All intelligence stays in the API.

---

## Agent Delegation Plan

### Phase 1: Research Agent — MCP Protocol Specification
**Task**: Research the MCP specification and Rust MCP SDK ecosystem.

**Deliverables**:
1. Which Rust MCP SDK to use. Evaluate:
   - `mcp-rust-sdk` (official Anthropic reference)
   - `rmcp` (community)
   - Build from scratch with `tower` + JSON-RPC (if SDKs are immature)
2. Transport decisions:
   - **stdio** (primary) — for IDE integrations (Claude Code, Cursor)
   - **SSE/Streamable HTTP** (secondary) — for server-side agents, remote access
3. MCP capabilities needed:
   - `tools/list` — return all 7 tool schemas
   - `tools/call` — execute a tool, return result
   - `initialize` — handshake with capabilities
   - Progress notifications for long-running queries (semantic search)
4. Authentication pattern for MCP → HTTP API calls (API key forwarding)

**Constraints**:
- Must support MCP spec version 2025-03-26 or later
- Must handle concurrent tool calls from a single client
- Must NOT bundle database drivers — only an HTTP client

---

### Phase 2: Architect Agent — Design the MCP Service
**Task**: Design the `services/mcp/` crate structure and tool schemas.

**Deliverables**:

#### 2a. Crate Structure
```
services/mcp/
├── Cargo.toml
├── Dockerfile
└── src/
    ├── main.rs           # Entry point, transport selection (stdio vs SSE)
    ├── config.rs         # Configuration (API base URL, timeouts, auth)
    ├── server.rs         # MCP server setup, capability negotiation
    ├── tools/
    │   ├── mod.rs        # Tool registry — list all tools
    │   ├── search.rs     # search_code tool
    │   ├── function.rs   # get_function tool
    │   ├── callers.rs    # get_callers tool
    │   ├── traits.rs     # get_trait_impls tool
    │   ├── usages.rs     # find_type_usages tool
    │   ├── modules.rs    # get_module_tree tool
    │   └── graph.rs      # query_graph tool
    ├── client.rs         # HTTP client wrapper for rust-brain API
    └── error.rs          # MCP error mapping from HTTP errors
```

#### 2b. MCP Tool Schemas (JSON Schema for each tool)

Design each tool's `inputSchema` with precision. The schema IS the interface contract for every agent and IDE. Example:

```json
{
  "name": "search_code",
  "description": "Search the codebase using natural language. Returns functions, structs, traits, and other items semantically matching your query. Use this to find code by intent rather than exact names. Examples: 'function that serializes data to JSON', 'error handling middleware', 'database connection pool setup'.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Natural language description of the code you're looking for. Be specific about functionality, not names."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of results to return.",
        "default": 10,
        "minimum": 1,
        "maximum": 50
      },
      "score_threshold": {
        "type": "number",
        "description": "Minimum similarity score (0.0-1.0). Higher values return more precise matches.",
        "default": 0.5
      },
      "crate_filter": {
        "type": "string",
        "description": "Optional: filter results to a specific crate name."
      }
    },
    "required": ["query"]
  }
}
```

```json
{
  "name": "get_function",
  "description": "Get complete details about a specific function, struct, enum, trait, or other item by its fully qualified name (FQN). Returns signature, documentation, source location, parameters, return type, direct callers, and callees. Use after search_code to drill into a specific result.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "fqn": {
        "type": "string",
        "description": "Fully qualified name of the item. Example: 'my_crate::module::function_name' or 'serde_json::from_str'."
      }
    },
    "required": ["fqn"]
  }
}
```

```json
{
  "name": "get_callers",
  "description": "Find all functions that call a given function, with configurable depth for transitive caller discovery. Depth 1 = direct callers only. Depth 2+ = callers of callers. Use this for impact analysis — understanding what breaks if you change a function.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "fqn": {
        "type": "string",
        "description": "Fully qualified name of the function to find callers for."
      },
      "depth": {
        "type": "integer",
        "description": "How many levels of transitive callers to include. 1=direct only, 2=callers of callers, etc.",
        "default": 1,
        "minimum": 1,
        "maximum": 10
      }
    },
    "required": ["fqn"]
  }
}
```

```json
{
  "name": "get_trait_impls",
  "description": "List all types that implement a specific trait. Returns each implementing type with its location. Use this when you need to understand the polymorphism landscape — e.g., 'which types implement Serialize?' or 'what are all the Handler implementations?'",
  "inputSchema": {
    "type": "object",
    "properties": {
      "trait_name": {
        "type": "string",
        "description": "Name of the trait. Can be a simple name like 'Serialize' or fully qualified like 'serde::Serialize'."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum implementations to return.",
        "default": 10,
        "minimum": 1,
        "maximum": 100
      }
    },
    "required": ["trait_name"]
  }
}
```

```json
{
  "name": "find_type_usages",
  "description": "Find all locations where a specific type is referenced — as function parameters, return types, struct fields, or generic arguments. Use this for dependency analysis: 'who uses this type and how?'",
  "inputSchema": {
    "type": "object",
    "properties": {
      "type_name": {
        "type": "string",
        "description": "Fully qualified name of the type to find usages for."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum usages to return.",
        "default": 10,
        "minimum": 1,
        "maximum": 100
      }
    },
    "required": ["type_name"]
  }
}
```

```json
{
  "name": "get_module_tree",
  "description": "Get the complete module hierarchy for a crate. Returns the tree structure of modules, their file paths, and optionally the items defined in each module. Use this to understand crate organization and navigate the code structure.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "crate_name": {
        "type": "string",
        "description": "Name of the crate to get the module tree for."
      }
    },
    "required": ["crate_name"]
  }
}
```

```json
{
  "name": "query_graph",
  "description": "Execute a read-only Cypher query against the code knowledge graph (Neo4j). Use this for custom traversals that the other tools don't cover. The graph has nodes: Crate, Module, Function, Struct, Enum, Trait, Impl, TypeAlias, Const, Static, Macro. Relationships: CONTAINS, DEFINES, CALLS, IMPLEMENTS, FOR_TYPE, HAS_PARAM, RETURNS, IMPORTS. WRITE operations (CREATE, DELETE, SET, MERGE, REMOVE) are forbidden.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Cypher query string. Must be read-only (MATCH/RETURN/WITH/WHERE only). Use $param_name for parameterized queries."
      },
      "parameters": {
        "type": "object",
        "description": "Parameters for the Cypher query. Keys are parameter names (without $), values are strings or numbers.",
        "additionalProperties": true
      }
    },
    "required": ["query"]
  }
}
```

```json
{
  "name": "find_calls_with_type",
  "description": "Find all call sites where a specific type is used as a type argument (turbofish syntax like func::<String>()). This queries the PostgreSQL call_sites table which stores monomorphized call information from typecheck analysis. Use this to find concrete usages of generic functions, track type instantiations, and understand how types flow through generic code.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "type_name": {
        "type": "string",
        "description": "Name of the type to search for in concrete type arguments. Examples: 'String', 'Vec', 'PaymentRequest', 'i32'."
      },
      "callee_name": {
        "type": "string",
        "description": "Optional: filter by callee function name. Use to narrow results to specific generic functions like 'parse' or 'collect'."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of results to return.",
        "default": 20,
        "minimum": 1,
        "maximum": 100
      }
    },
    "required": ["type_name"]
  }
}
```

```json
{
  "name": "find_trait_impls_for_type",
  "description": "Find all trait implementations for a specific type (by self_type). This queries the PostgreSQL trait_implementations table from typecheck analysis. Unlike get_trait_impls which finds implementations BY trait name, this finds implementations BY the implementing type. Use this to discover what traits a type implements.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "type_name": {
        "type": "string",
        "description": "Name of the type to search for. Examples: 'String', 'Vec', 'PaymentConnector', 'MyStruct'."
      },
      "limit": {
        "type": "integer",
        "description": "Maximum number of implementations to return.",
        "default": 20,
        "minimum": 1,
        "maximum": 100
      }
    },
    "required": ["type_name"]
  }
}
```

**CRITICAL**: Tool descriptions are the soul of the skill. They are the primary interface between the agent's reasoning and the tool's capability. Each description must:
1. Explain WHAT the tool does in plain language
2. Explain WHEN to use it (use cases)
3. Give concrete EXAMPLES of inputs
4. Mention LIMITATIONS or gotchas
5. Reference RELATED tools for workflow chaining

#### 2c. Error Mapping Strategy

| HTTP Status | MCP Error Code | MCP Error Message Pattern |
|-------------|---------------|---------------------------|
| 400 | InvalidParams | "Invalid parameters: {detail}" |
| 404 | InvalidParams | "Not found: {fqn/type/trait} does not exist in the knowledge graph" |
| 403 | InvalidRequest | "Forbidden: write operations are not allowed in query_graph" |
| 500 | InternalError | "Internal error: {service} is experiencing issues" |
| 503 | InternalError | "Service unavailable: {dependency} is not healthy" |
| Timeout | InternalError | "Request timed out after {n}s — try a simpler query or smaller limit" |
| Connection refused | InternalError | "Cannot reach rust-brain API at {url} — is the service running?" |

#### 2d. Configuration Design

```toml
# rustbrain-mcp.toml (or via environment variables)
[api]
base_url = "http://localhost:8088"    # RUSTBRAIN_API_URL
timeout_seconds = 30                   # RUSTBRAIN_API_TIMEOUT
api_key = ""                           # RUSTBRAIN_API_KEY (future auth)

[server]
transport = "stdio"                    # "stdio" or "sse"
name = "rustbrain"                     # MCP server name
version = "0.1.0"                      # Server version

[sse]
host = "127.0.0.1"                     # SSE bind address
port = 8089                            # SSE port
```

---

### Phase 3: Code Agent — Implement the MCP Server
**Task**: Write the complete `services/mcp/` crate.

**Implementation rules**:

1. **Cargo.toml dependencies** (keep minimal):
   - The chosen MCP SDK crate (from Phase 1 research)
   - `reqwest` (HTTP client — already used in the project)
   - `serde` + `serde_json` (serialization)
   - `tokio` (async runtime)
   - `tracing` + `tracing-subscriber` (logging — match existing pattern)
   - `anyhow` (error handling — match existing pattern)
   - `clap` (CLI args for transport selection)
   - NO database drivers. NO Ollama client. NO Neo4j client.

2. **HTTP Client (`client.rs`)**:
   ```rust
   pub struct RustbrainClient {
       http: reqwest::Client,
       base_url: String,
       api_key: Option<String>,
   }

   impl RustbrainClient {
       // One method per tool, strongly typed
       pub async fn search_semantic(&self, query: &str, limit: usize, ...) -> Result<SearchResponse>;
       pub async fn get_function(&self, fqn: &str) -> Result<FunctionDetail>;
       pub async fn get_callers(&self, fqn: &str, depth: usize) -> Result<CallersResponse>;
       pub async fn get_trait_impls(&self, trait_name: &str, limit: usize) -> Result<TraitImplsResponse>;
       pub async fn find_type_usages(&self, type_name: &str, limit: usize) -> Result<UsagesResponse>;
       pub async fn get_module_tree(&self, crate_name: &str) -> Result<ModuleTreeResponse>;
       pub async fn query_graph(&self, query: &str, params: Map) -> Result<GraphResponse>;
   }
   ```
   - The response types should reuse / mirror the API's JSON shapes
   - Handle timeouts, connection errors, and HTTP error codes gracefully
   - Add `X-Request-Source: mcp` header to all requests for observability

3. **Tool Handlers (tools/*.rs)**:
   Each tool handler:
   - Parses the MCP `arguments` JSON into the tool's input struct
   - Validates required fields, applies defaults
   - Calls `RustbrainClient` method
   - Formats the response as MCP `content` (text type with formatted output)
   - Returns structured text, NOT raw JSON — agents read text better than JSON blobs

   Example response formatting for `search_code`:
   ```
   Found 3 results for "JSON serialization":

   1. serde_json::to_string (function) — score: 0.94
      Serialize the given data structure as a String of JSON.
      Location: serde_json/src/ser.rs:45-62

   2. serde_json::to_value (function) — score: 0.89
      Convert a T into serde_json::Value.
      Location: serde_json/src/value/mod.rs:120-135

   3. serde::Serialize (trait) — score: 0.82
      A data structure that can be serialized into any format.
      Location: serde/src/ser.rs:200-250
   ```

4. **main.rs entry point**:
   ```
   rustbrain-mcp [--transport stdio|sse] [--api-url URL] [--port PORT]
   ```
   - Default transport: stdio
   - Default API URL: http://localhost:8088
   - Read config from env vars, CLI args, or config file (in that priority order)

5. **Dockerfile**: Multi-stage build matching the existing pattern in `services/api/Dockerfile`

6. **Docker Compose addition**: Add the MCP service to `docker-compose.yml`:
   ```yaml
   mcp:
     build:
       context: ./services/mcp
       dockerfile: Dockerfile
     container_name: rustbrain-mcp
     ports:
       - "${MCP_SSE_PORT:-8089}:8089"
     environment:
       RUSTBRAIN_API_URL: http://api:8080
       RUSTBRAIN_MCP_TRANSPORT: sse
       RUST_LOG: info,rustbrain_mcp=debug
     depends_on:
       api:
         condition: service_healthy
     networks: [rustbrain-net]
     profiles:
       - mcp
   ```

---

### Phase 4: Manager Agent — Integration, Testing & Documentation
**Task**: Ensure the MCP server works end-to-end and is ready for team adoption.

**Deliverables**:

#### 4a. Integration Tests
Create `tests/integration/test_mcp.sh`:
- Start the MCP server in SSE mode
- Call `initialize` and verify capabilities
- Call `tools/list` and verify all 9 tools are present
- Call each tool with valid inputs and verify response structure
- Call with invalid inputs and verify error responses
- Test timeout handling

#### 4b. Claude Code Configuration Example
Create `docs/mcp-setup.md` with:

```json
// Add to ~/.claude/claude_desktop_config.json (or .mcp.json in repo root)
{
  "mcpServers": {
    "rustbrain": {
      "command": "rustbrain-mcp",
      "args": ["--api-url", "http://localhost:8088"],
      "env": {
        "RUST_LOG": "info"
      }
    }
  }
}
```

Also provide the `.mcp.json` file for repository-level config:
```json
{
  "mcpServers": {
    "rustbrain": {
      "command": "docker",
      "args": ["exec", "-i", "rustbrain-mcp", "rustbrain-mcp", "--transport", "stdio"],
      "env": {}
    }
  }
}
```

#### 4c. Smoke Test Script
Create `scripts/test-mcp.sh`:
```bash
#!/bin/bash
# Test MCP server responds to initialize and tools/list via stdio
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}' | rustbrain-mcp | jq .
```

#### 4d. Add to Prometheus monitoring
- The MCP server should expose `/metrics` on its SSE port
- Add scrape target to `configs/prometheus/prometheus.yml`
- Track: tool call count, latency per tool, error rate

---

## Quality Gates (Must Pass Before Merge)

1. `cargo build --release` succeeds for `services/mcp/`
2. `cargo clippy -- -D warnings` passes
3. `cargo test` for unit tests in the crate
4. Integration test script passes against running stack
5. Claude Code can discover and call all 9 tools via stdio transport
6. Tool descriptions produce correct tool selection by the LLM (test with sample queries)
7. Error messages are actionable (the agent reading the error knows what to do next)
8. No database drivers in `Cargo.toml` — only HTTP client
9. Response times: MCP overhead < 50ms above raw API latency

---

## Non-Goals (Explicitly Out of Scope)

- **No authentication implementation** — defer to future phase (see future-scope.md)
- **No caching in the MCP layer** — the API layer handles this
- **No resources or prompts MCP capabilities** — tools only for now
- **No database access** — HTTP API is the only backend
- **No WebSocket transport** — stdio + SSE covers all use cases

---

## Success Criteria

When this is done, a developer on the team can:
1. Install `rustbrain-mcp` (cargo install or Docker)
2. Add 3 lines to their Claude Code / Cursor MCP config
3. Ask "find the function that handles authentication" and get results from the knowledge graph
4. Ask "what calls `my_crate::auth::verify_token`?" and get the call chain
5. Ask "show me all implementations of the Handler trait" and get the full list
6. All of this works without the developer knowing anything about Neo4j, Qdrant, or Postgres

The MCP server makes the knowledge graph invisible. The agent just has tools.

---

## Skill Files to Create After MCP Server

Once the MCP server is operational, create these skill definitions for each agent type. These are the "souls" — the combination of tools + prompt that define each agent's capability:

### research_agent.skill
```yaml
name: research_agent
description: Investigates codebase to answer questions about structure, patterns, and dependencies
mcp_server: rustbrain
tools_allowed:
  - search_code
  - get_function
  - get_callers
  - get_trait_impls
  - get_module_tree
  - find_calls_with_type
  - find_trait_impls_for_type
tools_denied:
  - query_graph  # Too powerful for research, use structured tools
max_iterations: 20
```

### architect_agent.skill
```yaml
name: architect_agent
description: Analyzes codebase architecture, plans changes, performs impact analysis
mcp_server: rustbrain
tools_allowed:
  - search_code
  - get_function
  - get_callers
  - find_type_usages
  - get_module_tree
  - find_calls_with_type
  - find_trait_impls_for_type
  - query_graph  # Architect needs custom traversals
max_iterations: 30
```

### code_agent.skill
```yaml
name: code_agent
description: Reads existing code to understand context before writing changes
mcp_server: rustbrain
tools_allowed:
  - get_function
  - get_callers
  - get_trait_impls
  - search_code
  - find_calls_with_type
  - find_trait_impls_for_type
max_iterations: 15
```

---

## Delegation Summary

| Phase | Agent | Deliverable | Depends On |
|-------|-------|------------|------------|
| 1 | Research | MCP SDK evaluation, transport decision | Nothing |
| 2 | Architect | Crate design, tool schemas, error mapping | Phase 1 |
| 3 | Code | Full implementation of services/mcp/ | Phase 2 |
| 4 | Manager | Tests, docs, CI integration, monitoring | Phase 3 |

**Estimated total**: This is a focused build — the MCP server is a thin proxy with well-defined inputs and outputs. The complexity is in getting the tool descriptions right (Phase 2), not in the code (Phase 3).
