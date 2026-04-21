# OpenCode + LiteLLM + MCP Integration

## Architecture Decision Record (ADR)

### Problem Statement

Rust code intelligence requires both domain knowledge (parse, typecheck, graph) and reasoning capability (analysis, suggestions, documentation). A monolithic system would require rebuilding neural networks for each codebase.

### Decision

Adopt a **hub-and-spoke architecture** where:
1. **rust-brain** is the domain-expert "spoke" (code parsing, indexing, graph)
2. **OpenCode IDE** is the user interface "spoke"
3. **Claude (via LiteLLM)** is the reasoning "hub"
4. **MCP (Model Context Protocol)** is the standardized tool interface

### Rationale

- **Separation of Concerns**: rust-brain stays focused on code analysis; Claude handles reasoning
- **Tool Reusability**: Any MCP-compatible IDE can use rust-brain tools
- **Model Flexibility**: LiteLLM allows swapping Claude ↔ GPT ↔ local models without code changes
- **Streaming First**: SSE transport enables real-time IDE interactions
- **Cost Optimization**: LiteLLM supports request caching, batch processing, cheaper model fallbacks

### Tradeoffs

| Aspect | Choice | Why |
|--------|--------|-----|
| Transport | HTTP SSE over stdio | SSE enables IDE streaming; HTTP works through firewalls |
| Model Routing | LiteLLM | Unified API for Anthropic, OpenAI, open-source; cost tracking |
| Tool Discovery | MCP `/tools` endpoint | Standard interface; IDE doesn't need hardcoded tool knowledge |
| Streaming | Per-response (not per-tool) | Simpler implementation; sufficient for UX |

---

## Service Topology

### Port Map

```
Browser/IDE                  Backend Services
    │
    ├─[8088]─────────>  Playground UI + Tool API
    │                   (static HTML + REST, 22 endpoints)
    │
    ├─[4096]─────────>  OpenCode IDE
    │                   (browser-based editor)
    │
    ├─[3001]─────────>  MCP SSE Server
    │                   (SSE tool transport)
    │
    │                   LiteLLM Proxy (EXTERNAL)
    │                   (hosted at LITELLM_BASE_URL)
    │
    └─[8088/tools]──>   Tool API (REST)
                         ├─ /tools/search_semantic
                         ├─ /tools/get_function
                         ├─ /tools/get_callers
                         ├─ /tools/chat (+ sessions CRUD)
                         └─ ... (22 total endpoints)
                              │
                              └─[8085]─────────>  Pgweb (Postgres UI)
                              └─[7474]─────────>  Neo4j Browser
                              └─[6333]─────────>  Qdrant Dashboard
```

> **Note:** LiteLLM is **not** a local Docker container. It is hosted externally (configured via `LITELLM_BASE_URL` in `.env`). OpenCode connects to it for model routing.

### Service Dependencies

```
OpenCode (port 4096)
    │
    ├─ requests auth token from OpenCode Server
    ├─ sends tool invocations to MCP SSE (port 3001)
    │
MCP SSE Server (port 3001)
    │
    ├─ receives tool invocations from OpenCode
    ├─ forwards to Claude via LiteLLM (port 4000)
    │ Claude reads/understands tools via OpenCode context
    │
LiteLLM (port 4000)
    │
    ├─ receives requests from MCP SSE
    ├─ routes to Anthropic API OR fallback model
    ├─ returns streamed responses to MCP SSE
    │
Tool API (port 8088)
    │
    ├─ implements /tools/* endpoints
    ├─ queries PostgreSQL, Neo4j, Qdrant
    ├─ returns structured tool results
    │
rust-brain Databases
    │
    ├─ PostgreSQL (8082 pgweb): raw source, metadata
    ├─ Neo4j (7474 browser): AST, call graphs, trait hierarchy
    └─ Qdrant (6333 dashboard): code embeddings
```

---

## Configuration

### Environment Variables

Add these to `.env`:

```bash
# === OpenCode Server ===
OPENCODE_PORT=4096
OPENCODE_HOST=http://opencode:4096
OPENCODE_SERVER_PASSWORD=your_server_password_here

# === LiteLLM Proxy (EXTERNAL — not a local Docker container) ===
LITELLM_BASE_URL=https://grid.ai.juspay.net
LITELLM_API_KEY=your-api-key-here

# === AI Models ===
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...  # Optional fallback

# Model routing
CHAT_MODEL=glm-latest

# === MCP Server ===
MCP_SSE_PORT=3001
```

### LiteLLM Model Configuration

LiteLLM routes requests to configured model backends with automatic fallbacks.

#### Option 1: Environment Variables (Simple)

```bash
# Single primary model
LITELLM_MODEL_LIST=claude-3.5-sonnet

# With fallbacks (try cheaper model if primary fails)
LITELLM_FALLBACK_MODELS=claude-3.5-sonnet,gpt-4o,ollama/mistral
```

#### Option 2: Config File (Advanced)

Create `configs/litellm/config.yaml`:

```yaml
model_list:
  - model_name: "claude-sonnet"
    litellm_params:
      model: "claude-3-5-sonnet-20241022"
      api_key: ${ANTHROPIC_API_KEY}
      timeout: 120
      max_tokens: 4096
      temperature: 0.7

  - model_name: "claude-haiku"
    litellm_params:
      model: "claude-3-5-haiku-20241022"
      api_key: ${ANTHROPIC_API_KEY}
      timeout: 60
      max_tokens: 2048
      temperature: 0.5

  - model_name: "gpt-4o"
    litellm_params:
      model: "gpt-4o"
      api_key: ${OPENAI_API_KEY}
      timeout: 120

# Route different use cases to different models
router_settings:
  - route_name: "code_analysis"
    models: ["claude-haiku", "claude-sonnet"]  # Try haiku first (cheaper)

  - route_name: "complex_reasoning"
    models: ["claude-sonnet", "gpt-4o"]  # Use sonnet, fallback to gpt-4o

  - route_name: "chat"
    models: ["claude-sonnet"]  # Use sonnet for interactive chat

# Cost tracking
general_settings:
  cost_tracking: true
  log_requests: true
  log_response_time: true
```

Then run LiteLLM with config:

```bash
litellm --config configs/litellm/config.yaml --port 4000
```

---

## MCP Tool Registration

The MCP server discovers and exposes rust-brain tools to Claude via the `/mcp/tools` endpoint.

### Tool Definition Format

Each tool is defined with:
- **name**: Unique identifier (snake_case)
- **description**: What the tool does (for Claude)
- **inputSchema**: JSON Schema for tool arguments
- **endpoint**: Where to invoke it (relative to tool API base)

### Available Tools

```json
{
  "tools": [
    {
      "name": "search_semantic",
      "description": "Search code semantically by natural language query",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": {
            "type": "string",
            "description": "Natural language search query (e.g., 'error handling')"
          },
          "top_k": {
            "type": "integer",
            "description": "Number of results to return (default 5)",
            "default": 5
          },
          "filter_type": {
            "type": "string",
            "enum": ["function", "type", "trait", "module"],
            "description": "Filter by kind"
          }
        },
        "required": ["query"]
      },
      "endpoint": "POST /tools/search_semantic"
    },

    {
      "name": "get_function",
      "description": "Get full function definition with signature, body, tests",
      "inputSchema": {
        "type": "object",
        "properties": {
          "fqn": {
            "type": "string",
            "description": "Fully qualified name (e.g., 'crate::module::function')"
          }
        },
        "required": ["fqn"]
      },
      "endpoint": "GET /tools/get_function?fqn={fqn}"
    },

    {
      "name": "get_callgraph",
      "description": "Get callers and callees of a function",
      "inputSchema": {
        "type": "object",
        "properties": {
          "fqn": {
            "type": "string"
          },
          "direction": {
            "type": "string",
            "enum": ["callers", "callees", "both"],
            "default": "both"
          },
          "depth": {
            "type": "integer",
            "description": "Transitive depth (1 = direct only)",
            "default": 2
          }
        },
        "required": ["fqn"]
      },
      "endpoint": "GET /tools/get_callgraph?fqn={fqn}&direction={direction}&depth={depth}"
    },

    {
      "name": "get_trait_impls",
      "description": "Find all implementations of a trait",
      "inputSchema": {
        "type": "object",
        "properties": {
          "trait_name": {
            "type": "string",
            "description": "Trait name (e.g., 'Clone', 'Iterator')"
          },
          "include_external": {
            "type": "boolean",
            "default": false
          }
        },
        "required": ["trait_name"]
      },
      "endpoint": "GET /tools/get_trait_impls?trait_name={trait_name}"
    },

    {
      "name": "find_usages",
      "description": "Find all usages of a type or function",
      "inputSchema": {
        "type": "object",
        "properties": {
          "fqn": {
            "type": "string"
          },
          "usage_type": {
            "type": "string",
            "enum": ["direct", "transitive"],
            "default": "direct"
          }
        },
        "required": ["fqn"]
      },
      "endpoint": "GET /tools/find_usages?fqn={fqn}&usage_type={usage_type}"
    },

    {
      "name": "query_cypher",
      "description": "Execute raw Neo4j Cypher query for advanced graph analysis",
      "inputSchema": {
        "type": "object",
        "properties": {
          "query": {
            "type": "string",
            "description": "Cypher query (e.g., 'MATCH (f:Function) RETURN f.fqn LIMIT 10')"
          },
          "parameters": {
            "type": "object",
            "description": "Query parameters as JSON object"
          }
        },
        "required": ["query"]
      },
      "endpoint": "POST /tools/query_cypher"
    }
  ]
}
```

### Tool Invocation Flow

```
OpenCode IDE                MCP SSE Server               Tool API
    │                          │                            │
    │─[user asks question]─────>│                            │
    │                          │                            │
    │<─────[stream start]───────│                            │
    │                          │                            │
    │    Claude decides to use tools                        │
    │<─[tool: search_semantic]──│                            │
    │<─[args: {query: ...}]─────│                            │
    │                          │─[POST /tools/search_...]──>│
    │                          │                            │ (query vector DB)
    │                          │<──[{"results": [...]}]─────│
    │<─[tool_result: {...}]─────│                            │
    │                          │                            │
    │    Claude reasons further                             │
    │<─[text: "Based on the..."]│                            │
    │<─[stop_reason: end_turn]──│                            │
    │                          │                            │
```

### Registering Custom Tools

To add a new tool:

1. **Implement endpoint** in rust-brain API (e.g., `POST /tools/custom_analysis`)
2. **Define schema** in tool registry (OpenAPI or MCP format)
3. **Register in MCP server** (`services/mcp/src/tools.rs`)
4. **Test** with `curl http://localhost:8088/mcp/tools`

---

## OpenCode IDE Configuration

### Target Repository Configuration

OpenCode needs to know which project to analyze. Set the `TARGET_REPO_PATH` environment variable to point to your Rust crate:

```bash
# In .env file
TARGET_REPO_PATH=/path/to/your/rust/crate

# Example for hyperswitch
TARGET_REPO_PATH=/home/user/projects/hyperswitch
```

This mounts your target project into the OpenCode container at `/workspace/target-repo`, allowing agents to:
- Navigate the source code filesystem
- Read files referenced by MCP tool results
- Understand project structure

**Note:** After changing `TARGET_REPO_PATH`, restart the OpenCode container:
```bash
docker compose restart opencode
```

### Starting OpenCode

OpenCode runs in Docker Compose:

```yaml
# docker-compose.yml
services:
  opencode:
    image: opencode/opencode:latest
    ports:
      - "4096:4096"
    environment:
      - AUTH_PASSWORD=${OPENCODE_SERVER_PASSWORD}
      - LOG_LEVEL=debug
      - MCP_SSE_URL=http://mcp-sse:3001
    depends_on:
      - mcp-sse
```

Access at **http://localhost:4096** in a browser.

### Configuring Tools in OpenCode

OpenCode discovers MCP tools via the SSE server at `/mcp/tools`:

```json
{
  "mcp_server": {
    "type": "sse",
    "url": "http://localhost:3001",
    "tool_discovery": "auto"
  }
}
```

Tools appear in the IDE as:
- Chat autocomplete suggestions
- `/` command palette
- Context menu on selected code

### Authentication

Set `OPENCODE_SERVER_PASSWORD` in `.env` to enable server-side authentication:

```bash
OPENCODE_SERVER_PASSWORD=secure_password_123
```

Users must enter this password on first connection.

---

## Streaming Architecture (SSE)

### Why SSE Instead of Stdio?

| Aspect | Stdio MCP | SSE MCP |
|--------|-----------|---------|
| Transport | Process pipes | HTTP streaming |
| Firewall | Requires port forwarding | Works through HTTP proxy |
| IDE Support | Limited | All modern browsers |
| Streaming | Per-message | Per-response |
| Latency | Minimal | +50-100ms over HTTP |

**Decision**: Use both:
- **Stdio**: For CLI tools and local development
- **SSE**: For IDE integration and remote deployments

### SSE Message Format

Each message is JSON on a single line with format:

```
data: {"event": "...", "payload": {...}}
```

Events emitted by the server:

| Event | Payload | Meaning |
|-------|---------|---------|
| `tool` | `{name: "search_semantic"}` | Tool to invoke |
| `invoke` | `{endpoint: "POST /tools/..."}` | HTTP endpoint |
| `args` | `{...}` | Arguments for tool |
| `tool_result` | `[...]` | Result from tool |
| `text` | `"streamed text"` | Response text chunk |
| `stop_reason` | `"end_turn"` | Conversation ended |
| `usage` | `{input_tokens, output_tokens}` | Token counts |
| `error` | `{message, code}` | Error occurred |

### Example Stream

```
data: {"event": "tool", "payload": {"name": "search_semantic"}}
data: {"event": "invoke", "payload": {"endpoint": "POST /tools/search_semantic"}}
data: {"event": "args", "payload": {"query": "error handling", "top_k": 5}}
data: {"event": "tool_result", "payload": [{"fqn": "crate::error::Result", "file": "src/error.rs", "lines": 10}]}
data: {"event": "text", "payload": "Based on the search results, I found the error type used throughout..."}
data: {"event": "text", "payload": " the codebase. Let me analyze the implementations..."}
data: {"event": "stop_reason", "payload": "end_turn"}
data: {"event": "usage", "payload": {"input_tokens": 450, "output_tokens": 230}}
```

---

## Troubleshooting

### MCP Tools Not Discoverable

**Symptom**: OpenCode shows no tools

**Diagnosis**:
```bash
# 1. Check MCP server is running
curl http://localhost:3001/mcp/tools

# Expected response:
# {"tools": [{"name": "search_semantic", ...}, ...]}

# If empty or 404, MCP server is not serving tools
```

**Fix**:
1. Check `services/mcp/src/tools.rs` defines all tools
2. Verify Tool API is reachable from MCP server
3. Check docker network: `docker network ls` → should include `rustbrain`

### LiteLLM Connection Fails

**Symptom**: Chat returns 502 Bad Gateway

**Diagnosis**:
```bash
# 1. Check LiteLLM is running
curl http://localhost:4000/health

# 2. Check model is configured
curl http://localhost:4000/models

# 3. Check API key
echo $ANTHROPIC_API_KEY
```

**Fix**:
1. Verify `ANTHROPIC_API_KEY` is set: `export ANTHROPIC_API_KEY="sk-ant-..."`
2. Restart LiteLLM: `docker compose restart litellm`
3. Check LiteLLM logs: `docker compose logs litellm`

### Streaming Response Hangs

**Symptom**: Chat starts but stops mid-stream

**Diagnosis**:
```bash
# Check SSE server is receiving events
tail -f logs/mcp-sse.log

# Check connection to Claude
curl -v http://localhost:4000/chat
```

**Fix**:
1. Increase timeout in LiteLLM: `LITELLM_REQUEST_TIMEOUT=300`
2. Check network latency: `ping api.anthropic.com`
3. Review MCP SSE server logs for exceptions

### Tools Return Empty Results

**Symptom**: Tool runs but returns `[]`

**Diagnosis**:
```bash
# 1. Check ingestion completed
curl http://localhost:8088/tools/health

# 2. Check database has data
docker exec rustbrain-neo4j-1 \
  cypher-shell -u neo4j -p rustbrain_dev_2024 \
  "MATCH (n) RETURN count(*) as total"

# 3. Check tool endpoint directly
curl -X POST http://localhost:8088/tools/search_semantic \
  -H "Content-Type: application/json" \
  -d '{"query": "async"}'
```

**Fix**:
1. Start ingestion: Check Dashboard → "Start Ingestion"
2. Verify database connection: `docker compose ps`
3. Check tool API logs: `docker compose logs api`

### OpenCode Authentication Fails

**Symptom**: "Invalid password" on first connection

**Diagnosis**:
```bash
# Check environment variable is set
echo $OPENCODE_SERVER_PASSWORD

# Check Docker got the value
docker compose exec opencode env | grep AUTH_PASSWORD
```

**Fix**:
1. Set password in `.env`: `OPENCODE_SERVER_PASSWORD=your_password`
2. Reload containers: `docker compose down && docker compose up -d opencode`
3. Try connecting with new password

### Keyboard Shortcuts Not Working in OpenCode

**Symptom**: Cmd+K doesn't open palette

**Diagnosis**:
```bash
# Browser console in OpenCode
Open DevTools → Console → type: window.keyboardHandler
```

**Fix**:
1. Click in editor area first (focus)
2. Check browser keybinding conflicts (extensions)
3. Use menu instead: "View" → "Command Palette"

---

## Monitoring & Observability

### Metrics Dashboard

Grafana dashboard available at http://localhost:3000 with:
- Tool invocation latency (by tool)
- Token usage (input/output per model)
- Error rates (tool failures, timeouts)
- Streaming response times (p50, p95, p99)

### Logging

Enable debug logging:

```bash
export RUST_LOG=debug
export LITELLM_LOG_LEVEL=debug
export MCP_SSE_LOG_LEVEL=debug

docker compose up --build
```

View logs:
```bash
docker compose logs -f mcp-sse    # MCP SSE server
docker compose logs -f api        # Tool API
docker compose logs -f litellm    # LiteLLM proxy
```

### Health Checks

```bash
# Overall system health
curl http://localhost:8088/health

# Tool API
curl http://localhost:8088/tools/health

# MCP server
curl http://localhost:3001/health

# LiteLLM
curl http://localhost:4000/health

# OpenCode
curl http://localhost:4096/health
```

---

## Performance Tuning

### Request Batching (LiteLLM)

For batch analysis (multiple files), use LiteLLM's batch API:

```python
import requests

batch = [
    {
        "custom_id": "req-1",
        "model": "claude-sonnet",
        "messages": [{"role": "user", "content": "Analyze file A"}]
    },
    {
        "custom_id": "req-2",
        "model": "claude-haiku",  # Cheaper model
        "messages": [{"role": "user", "content": "Analyze file B"}]
    }
]

resp = requests.post(
    "http://localhost:4000/batch/create",
    json={"requests": batch}
)
```

### Caching Strategy

**Tool Results**: MCP SSE server caches tool results for 5 minutes
```bash
CACHE_TTL=300  # seconds
CACHE_SIZE=1000  # max results
```

**Model Responses**: LiteLLM can cache common queries
```bash
LITELLM_CACHE=redis
REDIS_HOST=redis
REDIS_PORT=6379
```

### Concurrency Limits

```bash
# Max concurrent tool invocations
TOOL_MAX_CONCURRENT=10

# Max concurrent model requests
LITELLM_MAX_CONCURRENT=20

# Timeout for tool execution
TOOL_TIMEOUT=30  # seconds
```

---

## Security Considerations

### API Key Management

- **Never** commit `.env` with real keys
- Use `.env.local` (gitignored) for development
- Use AWS Secrets Manager / HashiCorp Vault for production
- Rotate keys monthly

### Network Security

- Firewall OpenCode/LiteLLM ports (expose only through reverse proxy)
- Use HTTPS in production (configure via reverse proxy)
- Require authentication for OpenCode IDE
- Log all API calls for audit

### Data Privacy

- Tool results contain source code — don't log to external services
- Claude can see queried code — review privacy implications
- Consider using cheaper local models (via LiteLLM) for sensitive code

---

## See Also

- [playground.md](./playground.md) — User interface guide
- [architecture.md](./architecture.md) — System design overview
- [runbook.md](./runbook.md) — Operations procedures
