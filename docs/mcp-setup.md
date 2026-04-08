# MCP Server Setup Guide

Configure MCP clients (Claude Code, Cursor, OpenCode, Cline, or any MCP-compatible tool) to use the rust-brain MCP server for Rust code intelligence.

## What is the rust-brain MCP Server?

The rust-brain MCP (Model Context Protocol) server exposes 15 tools for Rust code intelligence:

| Tool | Description |
|------|-------------|
| `search_code` | Natural language code search using vector embeddings |
| `get_function` | Get full function details with source code |
| `get_callers` | Find all callers of a function (direct and transitive) |
| `get_trait_impls` | List all implementations of a trait |
| `find_type_usages` | Find where a type is used |
| `get_module_tree` | Get module hierarchy for a crate |
| `query_graph` | Execute raw Cypher queries against Neo4j |
| `find_calls_with_type` | Find call sites with a specific type argument (turbofish) |
| `find_trait_impls_for_type` | Find all trait implementations for a type |
| `pg_query` | Execute read-only SQL queries against Postgres |
| `aggregate_search` | Cross-database fan-out search (Qdrant + Postgres + Neo4j) |
| `context_store` | Store and retrieve context for multi-turn conversations |
| `status_check` | Check ingestion and system health status |
| `task_update` | Update task progress during long-running operations |
| `consistency_check` | Verify cross-store consistency (Postgres/Neo4j/Qdrant) |

## Transport Modes

The MCP server supports two transports:

| Transport | Port | Container | Use case |
|-----------|------|-----------|----------|
| **SSE** | 3001 | `rustbrain-mcp-sse` | Remote clients (Cursor, OpenCode, Cline, Claude Code) |
| **stdio** | — | `rustbrain-mcp` | Direct pipe from host (Claude Code, Claude Desktop) |

Most clients should use the **SSE transport** at `http://localhost:3001/sse`. Use stdio only when the client supports launching a command directly.

## Prerequisites

1. **Docker and Docker Compose** installed
2. **An MCP-compatible client** (Claude Code, Cursor, OpenCode, Cline, etc.)
3. **rust-brain services running** (see Quick Start below)
4. **Rust codebase ingested** (optional — tools return empty results without data)

## Quick Start

### 1. Start All Services

```bash
cd /path/to/rust-brain
cp .env.example .env          # first time only
docker compose up -d
./scripts/healthcheck.sh      # wait for all services to report healthy
```

### 2. Verify the MCP Server

```bash
# Check the MCP SSE bridge is running
curl -s http://localhost:3001/health | jq .

# Check the REST API backing it
curl -s http://localhost:8088/health | jq .
```

Both should return `{"status": "ok", ...}`.

### 3. Ingest a Codebase (optional)

```bash
./scripts/ingest.sh /path/to/your/crate
```

See `docs/INGESTION_GUIDE.md` for details.

---

## Claude Code Configuration

Claude Code supports both SSE and stdio transports. Tool definitions are auto-discovered from the server — you do not need to list them in config.

### Option A: Project-Level Config (`.mcp.json`)

Create `.mcp.json` in your repository root:

```json
{
  "mcpServers": {
    "rust-brain": {
      "type": "sse",
      "url": "http://localhost:3001/sse"
    }
  }
}
```

### Option B: Stdio via Docker

If you prefer stdio transport (no open port needed):

```json
{
  "mcpServers": {
    "rust-brain": {
      "command": "docker",
      "args": ["exec", "-i", "rustbrain-mcp", "rustbrain-mcp"]
    }
  }
}
```

### Option C: Global Config

Add to your global Claude Code MCP settings:

**macOS/Linux:** `~/.claude/mcp.json`

```json
{
  "mcpServers": {
    "rust-brain": {
      "type": "sse",
      "url": "http://localhost:3001/sse"
    }
  }
}
```

### Verification

After configuring, restart Claude Code and run:

```
/mcp
```

You should see `rust-brain` listed with 15 available tools. Test with a query:

```
Ask: "Use the search_code tool to find functions related to parsing"
```

---

## Cursor Configuration

[Cursor](https://cursor.sh/) supports MCP servers via SSE transport.

**Prerequisites:** Cursor 0.45+ with MCP support enabled in Settings > Features > MCP.

### Option A: Project-Level Config

Create `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "rust-brain": {
      "url": "http://localhost:3001/sse",
      "transport": "sse"
    }
  }
}
```

### Option B: Global Config

**macOS/Linux:** `~/.cursor/mcp.json`
**Windows:** `%APPDATA%\Cursor\User\globalStorage\mcp.json`

```json
{
  "mcpServers": {
    "rust-brain": {
      "url": "http://localhost:3001/sse",
      "transport": "sse"
    }
  }
}
```

### Verification

1. Open Cursor and go to **Settings > MCP**
2. Verify `rust-brain` shows a green status indicator
3. Open a Rust project and use AI chat (`Cmd+L` / `Ctrl+L`)
4. Ask: "Use the search_code tool to find functions related to parsing"

If the status is red, check that `rustbrain-mcp-sse` is running:

```bash
curl -s http://localhost:3001/health | jq .
```

---

## OpenCode Configuration

[OpenCode](https://github.com/opencode-ai/opencode) is a terminal-based AI IDE with native MCP support. The rust-brain Docker stack includes an OpenCode container pre-configured with the MCP server.

**Prerequisites:** OpenCode 0.1+ (included in `docker compose up -d`, available at `http://localhost:4096`).

### Option A: Project-Level Config

Create `opencode.json` in your project root:

```json
{
  "mcp": {
    "servers": {
      "rust-brain": {
        "type": "sse",
        "url": "http://localhost:3001/sse"
      }
    }
  }
}
```

### Option B: Global Config

**macOS/Linux:** `~/.config/opencode/config.json`
**Windows:** `%APPDATA%\opencode\config.json`

```json
{
  "mcp": {
    "servers": {
      "rust-brain": {
        "type": "sse",
        "url": "http://localhost:3001/sse"
      }
    }
  }
}
```

### OpenCode Tool Naming

OpenCode prefixes MCP tool names with the server key. For a server named `rust-brain`, tools become `mcp_rust-brain_<tool>`:

| MCP Tool | OpenCode Tool Name |
|----------|-------------------|
| `search_code` | `mcp_rust-brain_search_code` |
| `get_function` | `mcp_rust-brain_get_function` |
| `get_callers` | `mcp_rust-brain_get_callers` |
| `get_trait_impls` | `mcp_rust-brain_get_trait_impls` |
| `find_type_usages` | `mcp_rust-brain_find_type_usages` |
| `get_module_tree` | `mcp_rust-brain_get_module_tree` |
| `query_graph` | `mcp_rust-brain_query_graph` |
| `find_calls_with_type` | `mcp_rust-brain_find_calls_with_type` |
| `find_trait_impls_for_type` | `mcp_rust-brain_find_trait_impls_for_type` |
| `pg_query` | `mcp_rust-brain_pg_query` |
| `aggregate_search` | `mcp_rust-brain_aggregate_search` |
| `context_store` | `mcp_rust-brain_context_store` |
| `status_check` | `mcp_rust-brain_status_check` |
| `task_update` | `mcp_rust-brain_task_update` |
| `consistency_check` | `mcp_rust-brain_consistency_check` |

### Verification

1. Start the stack: `docker compose up -d && ./scripts/healthcheck.sh`
2. Open OpenCode at `http://localhost:4096`
3. Run `/tools` in the OpenCode chat — you should see all 15 `mcp_rust-brain_*` tools listed
4. Test: ask "Use search_code to find functions related to error handling"

---

## Cline Configuration

[Cline](https://github.com/cline/cline) is a VS Code extension for autonomous coding with MCP support.

**Prerequisites:** Cline extension installed in VS Code.

### VS Code Settings

Add to your VS Code `settings.json`:

**macOS/Linux:** `~/.config/Code/User/settings.json`
**Windows:** `%APPDATA%\Code\User\settings.json`

```json
{
  "cline.mcpServers": {
    "rust-brain": {
      "type": "sse",
      "url": "http://localhost:3001/sse"
    }
  }
}
```

### Verification

1. Open VS Code with the Cline extension
2. Open the Cline sidebar (`Cmd+Shift+P` > "Cline: Open")
3. Check the MCP server status in the Cline settings panel — `rust-brain` should show as connected
4. Test: ask "Use search_code to find functions related to parsing"

---

## Generic MCP Client

Any MCP-compatible client can connect to rust-brain. The server implements the [Model Context Protocol](https://modelcontextprotocol.io/) specification.

### SSE Transport (recommended)

Connect to the SSE endpoint. The server sends an `endpoint` event on connection with the URL for posting JSON-RPC messages.

| Parameter | Value |
|-----------|-------|
| **SSE URL** | `http://localhost:3001/sse` |
| **Message URL** | `http://localhost:3001/message?sessionId=<id>` (provided by server) |
| **Health check** | `GET http://localhost:3001/health` |
| **Protocol** | JSON-RPC 2.0 over SSE |
| **Keep-alive** | 15-second ping interval |

Connection flow:

1. `GET /sse` — opens an SSE stream; first event is `endpoint` with the POST URL
2. Client sends JSON-RPC requests via `POST /message?sessionId=<id>`
3. Server pushes responses as `message` events on the SSE stream

### Stdio Transport

For clients that support launching a subprocess:

```bash
docker exec -i rustbrain-mcp rustbrain-mcp
```

The server reads JSON-RPC from stdin and writes responses to stdout. Logs go to stderr.

### Manual Test

Verify the SSE endpoint is reachable from any client:

```bash
# Open SSE connection (will print the endpoint event)
curl -N http://localhost:3001/sse

# Expected output:
# event: endpoint
# data: /message?sessionId=<generated-id>
```

### Tool Discovery

Clients discover tools via the MCP `tools/list` JSON-RPC method. No manual tool definitions are needed — the server advertises all 15 tools with their schemas automatically.

## Environment Variables

Configure the MCP server via environment variables in `.env`:

| Variable | Default | Description |
|----------|---------|-------------|
| `API_PORT` | `8088` | Host port mapped to the API container |
| `DATABASE_URL` | - | PostgreSQL connection string |
| `NEO4J_URI` | `bolt://neo4j:7687` | Neo4j connection URI |
| `NEO4J_USER` | `neo4j` | Neo4j username |
| `NEO4J_PASSWORD` | - | Neo4j password |
| `QDRANT_HOST` | `http://qdrant:6333` | Qdrant REST API URL |
| `OLLAMA_HOST` | `http://ollama:11434` | Ollama API URL |
| `EMBEDDING_MODEL` | `qwen3-embedding:4b` | Embedding model name |
| `EMBEDDING_DIMENSIONS` | `2560` | Embedding vector dimensions |
| `MCP_SSE_PORT` | `3001` | MCP SSE server port |

## Tool Usage Examples

### search_semantic

```bash
curl -X POST http://localhost:8088/tools/search_semantic \
  -H "Content-Type: application/json" \
  -d '{
    "query": "function that parses JSON string into struct",
    "limit": 5
  }'
```

### get_function

```bash
curl "http://localhost:8088/tools/get_function?fqn=serde_json::from_str&include_body=true"
```

### get_callers

```bash
curl "http://localhost:8088/tools/get_callers?fqn=my_app::process_data&depth=2"
```

### get_trait_impls

```bash
curl "http://localhost:8088/tools/get_trait_impls?trait_name=serde::Serialize&include_methods=true"
```

### find_usages_of_type

```bash
curl "http://localhost:8088/tools/find_usages_of_type?type_name=my_app::models::User"
```

### get_module_tree

```bash
curl "http://localhost:8088/tools/get_module_tree?crate=serde_json&include_items=true"
```

### query_graph

```bash
curl -X POST http://localhost:8088/tools/query_graph \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (f:Function)-[:CALLS]->(c:Function) WHERE f.fqn STARTS WITH $prefix RETURN f.fqn, c.fqn LIMIT 10",
    "parameters": {"prefix": "serde_json::"}
  }'
```

### find_calls_with_type

Find all call sites where a specific type is used as a type argument (turbofish syntax):

```bash
curl "http://localhost:8088/tools/find_calls_with_type?type_name=String&limit=10"
```

Filter by callee function:

```bash
curl "http://localhost:8088/tools/find_calls_with_type?type_name=PaymentRequest&callee_name=parse&limit=20"
```

**Use cases:**
- Find where `String` is used as a generic type argument: `parse::<String>()`
- Find concrete instantiations of generic functions
- Track monomorphized call sites

### find_trait_impls_for_type

Find all trait implementations for a specific type:

```bash
curl "http://localhost:8088/tools/find_trait_impls_for_type?type_name=PaymentRequest&limit=20"
```

**Use cases:**
- Find all traits implemented by a type
- Understand polymorphism landscape for a type
- Discover trait implementations from typecheck analysis

## Monitoring

The MCP server exposes Prometheus metrics at `/metrics`:

```bash
curl http://localhost:8088/metrics
```

Key metrics:
- `rustbrain_api_requests_total` - Total API requests by endpoint
- `rustbrain_api_request_duration_seconds` - Request latency histogram
- `rustbrain_api_errors_total` - Total errors by endpoint

View in Grafana at http://localhost:3000

## Troubleshooting

### Server Not Starting

1. Check Docker is running: `docker ps`
2. Check MCP SSE logs: `docker compose logs mcp-sse`
3. Check API logs: `docker compose logs api`
4. Verify ports are free: `netstat -tlnp | grep -E '3001|8088'`

### Connection Refused

1. Wait for services to initialize (30-60s)
2. Run health check: `./scripts/healthcheck.sh`
3. Verify MCP SSE bridge specifically: `curl http://localhost:3001/health`
4. Check service dependencies are healthy

### Empty Search Results

1. Verify codebase was ingested
2. Check Qdrant has vectors: `curl http://localhost:6333/collections/rust_functions`
3. Verify Ollama is running: `curl http://localhost:11434/api/tags`

### Neo4j Query Errors

1. Check Neo4j is healthy: `curl http://localhost:7474`
2. Verify data was ingested
3. Check query syntax with Neo4j Browser

## Architecture

```
┌──────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  Claude Code /   │     │  MCP SSE Bridge  │     │   REST API      │
│  Cursor / Cline  │────▶│  (SSE :3001)     │────▶│   (HTTP :8088)  │
│  OpenCode / etc  │     └──────────────────┘     └────────┬────────┘
└──────────────────┘                                       │
        │ (stdio)        ┌──────────────────┐              │
        └───────────────▶│  MCP Stdio       │──────────────┘
                         │  (docker exec)   │              │
                         └──────────────────┘    ┌─────────┼─────────┐
                                                 │         │         │
                                                 ▼         ▼         ▼
                                          ┌──────────┐ ┌────────┐ ┌────────┐
                                          │ Postgres │ │ Neo4j  │ │ Qdrant │
                                          │  :5432   │ │ :7687  │ │ :6333  │
                                          └──────────┘ └────────┘ └────────┘
                                                                      │
                                                                      ▼
                                                               ┌────────────┐
                                                               │   Ollama   │
                                                               │  :11434    │
                                                               └────────────┘
```

## Security Considerations

For production deployments:

1. **Add Authentication** - Implement API key or JWT authentication
2. **Enable TLS** - Use HTTPS with valid certificates
3. **Network Isolation** - Run services in a private network
4. **Rate Limiting** - Add rate limits for expensive queries
5. **Input Validation** - Validate all inputs (especially Cypher queries)

## Support

- Documentation: `/docs`
- API Spec: `/docs/api-spec.md`
- Runbook: `/docs/runbook.md`
- Issues: Check logs with `docker compose logs api`
