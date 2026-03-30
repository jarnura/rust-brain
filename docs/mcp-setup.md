# MCP Server Setup Guide

This guide explains how to configure Claude Code (and other MCP clients) to use the rust-brain MCP server for Rust code intelligence.

## What is the rust-brain MCP Server?

The rust-brain MCP (Model Context Protocol) server provides 9 tools for Rust code intelligence:

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

## Prerequisites

1. **Docker and Docker Compose** installed
2. **Claude Code** installed (or any MCP-compatible client)
3. **Rust codebase** to analyze (optional, for ingestion)

## Quick Start

### 1. Start the MCP Server

```bash
cd /path/to/rust-brain

# Copy environment file if not exists
cp .env.example .env

# Start all services
docker compose up -d

# Wait for services to be healthy
./scripts/healthcheck.sh
```

### 2. Verify the Server

```bash
# Check health
curl http://localhost:8088/health

# Run integration tests
./scripts/test-mcp.sh
```

### 3. Ingest Your Codebase

```bash
# Build the ingestion binary
cd target-repo && cargo build --release

# Run ingestion on your crate
./target/release/rustbrain-ingestion --crate-path /path/to/your/crate
```

## Claude Code Configuration

### Option A: Repository-Level Config (`.mcp.json`)

Create a `.mcp.json` file in your repository root:

```json
{
  "mcpServers": {
    "rust-brain": {
      "type": "http",
      "url": "http://localhost:8088",
      "tools": [
        {
          "name": "search_semantic",
          "description": "Search for Rust code using natural language queries",
          "parameters": {
            "type": "object",
            "properties": {
              "query": {
                "type": "string",
                "description": "Natural language search query"
              },
              "limit": {
                "type": "integer",
                "description": "Maximum results to return",
                "default": 10
              },
              "filters": {
                "type": "object",
                "description": "Optional filters (item_type, crate_name, visibility)"
              }
            },
            "required": ["query"]
          }
        },
        {
          "name": "get_function",
          "description": "Get full details for a specific function by its fully qualified name",
          "parameters": {
            "type": "object",
            "properties": {
              "fqn": {
                "type": "string",
                "description": "Fully qualified name of the function"
              },
              "include_body": {
                "type": "boolean",
                "description": "Include function body source",
                "default": true
              },
              "include_callers": {
                "type": "boolean",
                "description": "Include direct callers",
                "default": false
              }
            },
            "required": ["fqn"]
          }
        },
        {
          "name": "get_callers",
          "description": "Find all functions that call the specified function",
          "parameters": {
            "type": "object",
            "properties": {
              "fqn": {
                "type": "string",
                "description": "Fully qualified name of the callee"
              },
              "depth": {
                "type": "integer",
                "description": "Call depth (1=direct, 2=transitive)",
                "default": 1
              }
            },
            "required": ["fqn"]
          }
        },
        {
          "name": "get_trait_impls",
          "description": "List all implementations of a specific trait",
          "parameters": {
            "type": "object",
            "properties": {
              "trait_name": {
                "type": "string",
                "description": "Fully qualified name of the trait"
              },
              "include_methods": {
                "type": "boolean",
                "description": "Include implemented methods",
                "default": false
              }
            },
            "required": ["trait_name"]
          }
        },
        {
          "name": "find_usages_of_type",
          "description": "Find all locations where a specific type is used",
          "parameters": {
            "type": "object",
            "properties": {
              "type_name": {
                "type": "string",
                "description": "Fully qualified name of the type"
              },
              "usage_type": {
                "type": "string",
                "enum": ["parameter", "return", "field", "all"],
                "description": "Filter by usage type",
                "default": "all"
              }
            },
            "required": ["type_name"]
          }
        },
        {
          "name": "get_module_tree",
          "description": "Get the module hierarchy for a crate",
          "parameters": {
            "type": "object",
            "properties": {
              "crate": {
                "type": "string",
                "description": "Crate name"
              },
              "include_items": {
                "type": "boolean",
                "description": "Include items in each module",
                "default": false
              }
            },
            "required": ["crate"]
          }
        },
        {
          "name": "query_graph",
          "description": "Execute raw Cypher queries against the Neo4j graph database",
          "parameters": {
            "type": "object",
            "properties": {
              "query": {
                "type": "string",
                "description": "Cypher query string"
              },
              "parameters": {
                "type": "object",
                "description": "Query parameters"
              }
            },
            "required": ["query"]
          }
        },
        {
          "name": "find_calls_with_type",
          "description": "Find all call sites where a specific type is used as a type argument (turbofish syntax). Useful for finding concrete usages of generic functions like parse::<String>() or collect::<Vec<_>>().",
          "parameters": {
            "type": "object",
            "properties": {
              "type_name": {
                "type": "string",
                "description": "Name of the type to search for (e.g., 'String', 'Vec', 'i32')"
              },
              "callee_name": {
                "type": "string",
                "description": "Optional name of the callee function to filter by (e.g., 'parse', 'collect')"
              },
              "limit": {
                "type": "integer",
                "description": "Maximum number of results to return",
                "default": 20
              }
            },
            "required": ["type_name"]
          }
        },
        {
          "name": "find_trait_impls_for_type",
          "description": "Find all trait implementations for a specific type. Useful for understanding what traits a type implements, like finding all traits implemented by String or Vec.",
          "parameters": {
            "type": "object",
            "properties": {
              "type_name": {
                "type": "string",
                "description": "Name of the type to search for (e.g., 'String', 'Vec', 'MyStruct')"
              },
              "limit": {
                "type": "integer",
                "description": "Maximum number of implementations to return",
                "default": 20
              }
            },
            "required": ["type_name"]
          }
        }
      ]
    }
  }
}
```

### Option B: Global Config

For Claude Code, add to your global MCP configuration:

**macOS/Linux:** `~/.config/claude-code/mcp.json`
**Windows:** `%APPDATA%\claude-code\mcp.json`

```json
{
  "servers": {
    "rust-brain": {
      "type": "http",
      "url": "http://localhost:8088",
      "enabled": true
    }
  }
}
```

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
2. Check logs: `docker compose logs api`
3. Verify ports are free: `netstat -tlnp | grep 8080`

### Connection Refused

1. Wait for services to initialize (30-60s)
2. Run health check: `./scripts/healthcheck.sh`
3. Check service dependencies are healthy

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
┌──────────────────┐     ┌─────────────────┐
│   Claude Code    │────▶│   MCP Server    │
│   (MCP Client)   │     │   (HTTP :8088)  │
└──────────────────┘     └────────┬────────┘
                                  │
          ┌───────────────────────┼───────────────────────┐
          │                       │                       │
          ▼                       ▼                       ▼
   ┌────────────┐          ┌────────────┐          ┌────────────┐
   │  Postgres  │          │   Neo4j    │          │   Qdrant   │
   │  (Raw DB)  │          │  (Graph)   │          │ (Vectors)  │
   └────────────┘          └────────────┘          └────────────┘
                                  │
                                  ▼
                          ┌────────────┐
                          │   Ollama   │
                          │ (Embeddings)│
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
