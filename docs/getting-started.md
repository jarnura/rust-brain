# Getting Started with rust-brain

This guide walks you through setting up rust-brain, ingesting your first Rust crate, and running your first queries.

## Prerequisites

### System Requirements

| Requirement | Minimum | Recommended |
|-------------|---------|-------------|
| RAM | 16 GB | 32 GB+ |
| CPU | 4 cores | 8+ cores |
| Disk | 20 GB free | 50+ GB SSD |
| Docker | 24.0+ | Latest |
| Docker Compose | 2.20+ | Latest |

### Software Dependencies

- **Docker**: Required for running all services
- **Docker Compose**: For orchestrating the service stack
- **Git**: For cloning repositories to analyze
- **curl/jq**: For testing API endpoints (optional)

### Verify Prerequisites

```bash
# Check Docker version
docker --version
# Expected: Docker version 24.0.0 or higher

# Check Docker Compose version
docker compose version
# Expected: Docker Compose version v2.20.0 or higher

# Check available memory
free -h
# Ensure at least 16GB total RAM
```

## macOS Setup

If running on macOS (no NVIDIA GPU), apply the macOS override **before** starting services:

```bash
# Remove GPU config that Docker Desktop on macOS doesn't support
cp docker-compose.macos.yml docker-compose.override.yml
```

Docker Compose automatically merges `docker-compose.override.yml` with the main file, stripping the NVIDIA GPU reservation from the Ollama service. Ollama will run on CPU instead.

> **Note:** On macOS, the `zstd` command is required for snapshot extraction. Install via `brew install zstd` if not present.

## Step-by-Step Setup

### 1. Clone and Configure

```bash
# Navigate to the rust-brain directory
cd /path/to/hyperswitch/rust-brain

# Create environment file from template
cp .env.example .env

# macOS only: apply GPU-free override
cp docker-compose.macos.yml docker-compose.override.yml
```

### 2. Review Configuration

Edit `.env` to customize settings (defaults work for most cases):

```bash
# Key settings (defaults shown)
POSTGRES_USER=rustbrain
POSTGRES_PASSWORD=<your-password>
POSTGRES_DB=rustbrain

NEO4J_PASSWORD=<your-password>
GRAFANA_PASSWORD=rustbrain

# AI Models
EMBEDDING_MODEL=qwen3-embedding:4b
EMBEDDING_DIMENSIONS=2560
CODE_MODEL=codellama:7b

# Performance tuning
EMBED_BATCH_SIZE=32
NEO4J_BATCH_SIZE=1000
```

### 3. Start All Services

```bash
# Start the entire stack
bash scripts/start.sh
```

This script performs the following:
1. Starts core databases (Postgres, Neo4j, Qdrant)
2. Waits for each service to become healthy
3. Initializes Qdrant collections
4. Pulls AI models via Ollama
5. Starts observability stack (Prometheus, Grafana)
6. Runs health checks

**First-time startup**: 5-10 minutes (model downloads)
**Subsequent startups**: 1-2 minutes

### 4. Verify Services

```bash
# Run automated health check
bash scripts/healthcheck.sh
```

Expected output:
```
╔══════════════════════════════════════════════════════════════╗
║           RUST-BRAIN — Health Check                          ║
╚══════════════════════════════════════════════════════════════╝

=== HTTP Endpoints ===
Postgres (pgweb)     ✓ OK
Neo4j Browser        ✓ OK
Qdrant Dashboard     ✓ OK
Ollama API           ✓ OK
Prometheus           ✓ OK
Grafana              ✓ OK
Tool API             ✓ OK
```

### 5. Access Web Interfaces

| Service | URL | Default Credentials |
|---------|-----|---------------------|
| Grafana | http://localhost:3000 | admin / rustbrain |
| Neo4j Browser | http://localhost:7474 | neo4j / <your-password> |
| Qdrant Dashboard | http://localhost:6333/dashboard | None |
| Pgweb | http://localhost:8085 | Auto-connected |
| Prometheus | http://localhost:9090 | None |

## Quick Start with Snapshot (Recommended)

Skip the full ingestion pipeline and restore a pre-built snapshot with 219K items from [hyperswitch](https://github.com/juspay/hyperswitch):

```bash
# First time — downloads ~3 GB, restores all 3 databases
./scripts/run-with-snapshot.sh
```

This takes 5-10 minutes and gives you a fully populated system with:
- 219K code items in PostgreSQL
- 302K nodes + 590K relationships in Neo4j
- 219K vector embeddings in Qdrant

**Upgrading to a new snapshot version:**
```bash
# Re-downloads and restores, replacing the existing data
./scripts/run-with-snapshot.sh --force-refresh
```

> Use `--force-refresh` whenever a new snapshot is released. Without it, the script skips download if a previous snapshot file exists locally.

After restoring, open the playground at the API service URL to browse the code graph.

**Optional: Enable Explorer filesystem access**

If you have the ingested project cloned locally, the Explorer agent can grep/rg across source files directly (in addition to using the knowledge base):

```bash
# Clone the project if you don't have it
git clone https://github.com/juspay/hyperswitch.git ~/projects/hyperswitch

# Tell rust-brain where it is
echo 'TARGET_REPO_PATH=~/projects/hyperswitch' >> .env

# Restart opencode to pick up the mount
docker compose restart opencode
```

Without `TARGET_REPO_PATH`, the Explorer still works via MCP tools (knowledge base queries) — it just can't do raw `grep` across source files.

---

## Ingestion from Source (Alternative)

Use this if you want to ingest your own crate instead of using the snapshot.

### Prepare a Rust Crate

```bash
# Use any local Rust crate
# Ensure it compiles with `cargo check`
```

### Run Ingestion

The ingestion pipeline runs as a Docker service. To ingest a crate:

```bash
# Set the target repository path
export TARGET_REPO_PATH=/path/to/your/rust/crate

# Run the ingestion service
docker-compose run --rm ingestion \
  --source-path /target \
  --crate-name my-crate
```

**Tip:** The same `TARGET_REPO_PATH` is used by OpenCode to mount your project for agent analysis. Set it in `.env` for persistence:

```bash
# Add to .env
echo "TARGET_REPO_PATH=/path/to/your/rust/crate" >> .env
```

### What Happens During Ingestion

```
1. Expand    → cargo expand (resolves macros)
2. Parse     → tree-sitter (fast skeleton) + syn (deep analysis)
3. Typecheck → rust-analyzer (type information)
4. Extract   → Functions, structs, enums, traits, impls
5. Graph     → Neo4j: nodes for items, edges for relationships
6. Embed     → Qdrant: vector embeddings for semantic search
```

### Verify Ingestion

```bash
# Check Postgres for extracted items
docker-compose exec postgres psql -U rustbrain -d rustbrain -c \
  "SELECT COUNT(*) FROM extracted_items;"

# Check Neo4j for graph nodes
curl -s http://localhost:7474/db/neo4j/tx/commit \
  -u neo4j:<your-password> \
  -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (n) RETURN count(n)"}]}' | jq .

# Check Qdrant for vectors
curl -s http://localhost:6333/collections/code_embeddings | jq '.result.points_count'
```

## First API Query Example

### Check API Health

```bash
curl http://localhost:8088/health | jq .
```

Expected response:
```json
{
  "status": "healthy",
  "timestamp": "2026-03-14T10:30:00Z",
  "version": "0.1.0",
  "dependencies": {
    "postgres": {"status": "healthy", "latency_ms": 5},
    "neo4j": {"status": "healthy", "latency_ms": 12},
    "qdrant": {"status": "healthy", "latency_ms": 3},
    "ollama": {"status": "healthy", "latency_ms": 8}
  }
}
```

### Semantic Code Search

Search for functions using natural language:

```bash
curl -X POST http://localhost:8088/tools/search_semantic \
  -H "Content-Type: application/json" \
  -d '{
    "query": "function that deserializes JSON",
    "limit": 5
  }' | jq .
```

Response:
```json
{
  "query": "function that deserializes JSON",
  "total": 5,
  "results": [
    {
      "fqn": "serde_json::from_str",
      "name": "from_str",
      "kind": "function",
      "file_path": "serde_json/src/de.rs",
      "start_line": 245,
      "end_line": 267,
      "score": 0.89,
      "docstring": "Deserialize an instance of type T from a string of JSON text"
    }
  ]
}
```

### Get Function Details

Retrieve full details for a specific function:

```bash
curl "http://localhost:8088/tools/get_function?fqn=serde_json::from_str" | jq .
```

Response:
```json
{
  "fqn": "serde_json::from_str",
  "name": "from_str",
  "kind": "function",
  "visibility": "pub",
  "signature": "pub fn from_str<'a, T>(s: &'a str) -> Result<T, Error>",
  "docstring": "Deserialize an instance of type T from a string of JSON text.",
  "file_path": "serde_json/src/de.rs",
  "start_line": 245,
  "end_line": 267,
  "callers": [
    {"fqn": "my_app::config::load", "name": "load", "file_path": "src/config.rs", "line": 42}
  ],
  "callees": [
    {"fqn": "serde_json::de::Deserializer::from_str", "name": "from_str"}
  ]
}
```

### Find Callers (Call Graph)

Find all functions that call a specific function:

```bash
curl "http://localhost:8088/tools/get_callers?fqn=serde_json::from_str&depth=2" | jq .
```

Response:
```json
{
  "fqn": "serde_json::from_str",
  "depth": 2,
  "callers": [
    {"fqn": "my_app::config::load", "name": "load", "depth": 1, "file_path": "src/config.rs", "line": 42},
    {"fqn": "my_app::main", "name": "main", "depth": 2, "file_path": "src/main.rs", "line": 15}
  ]
}
```

### Find Trait Implementations

Find all types implementing a trait:

```bash
curl "http://localhost:8088/tools/get_trait_impls?trait_name=Serialize" | jq .
```

### Query the Graph Directly

Execute raw Cypher queries:

```bash
curl -X POST http://localhost:8088/tools/query_graph \
  -H "Content-Type: application/json" \
  -d '{
    "query": "MATCH (f:Function)-[:CALLS]->(g:Function) WHERE f.name CONTAINS \"parse\" RETURN f.name, g.name LIMIT 10"
  }' | jq .
```

## Editor Playground

The Editor Playground lets you create isolated workspaces from GitHub repositories, run AI agents against them, and review the results — all through the API or the playground UI.

### Prerequisites

- The API service must be running (`bash scripts/start.sh`)
- Docker must be available (the API creates volumes and containers per workspace)
- A GitHub repository URL to work with

### Quick Workflow

The playground follows a simple lifecycle: **create workspace → run execution → review changes → commit or reset**.

```bash
# 1. Create a workspace (returns 202, cloning happens in the background)
curl -X POST http://localhost:8088/workspaces \
  -H "Content-Type: application/json" \
  -d '{"github_url": "https://github.com/juspay/hyperswitch", "name": "hyperswitch-test"}'

# Response: {"id":"<workspace-id>","status":"cloning","message":"Workspace created. Clone started in the background."}

# 2. Poll until status is "ready" (cloning + indexing takes 2-5 min)
curl http://localhost:8088/workspaces/<workspace-id> | jq .status

# 3. Browse the file tree
curl http://localhost:8088/workspaces/<workspace-id>/files | jq .

# 4. Run an execution (returns 202, agents work in the background)
curl -X POST http://localhost:8088/workspaces/<workspace-id>/execute \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Add error handling to the payment module"}'

# Response: {"id":"<execution-id>","status":"running","message":"Execution started..."}

# 5. Stream agent events in real time
curl -N "http://localhost:8088/workspaces/<workspace-id>/stream?execution_id=<execution-id>"

# Or poll execution status:
curl http://localhost:8088/executions/<execution-id> | jq .status

# 6. Review changes
curl http://localhost:8088/workspaces/<workspace-id>/diff

# 7. Commit or discard
curl -X POST http://localhost:8088/workspaces/<workspace-id>/commit \
  -H "Content-Type: application/json" \
  -d '{"message": "Add error handling to payment module"}'

# Or discard all changes:
curl -X POST http://localhost:8088/workspaces/<workspace-id>/reset

# 8. Clean up when done
curl -X DELETE http://localhost:8088/workspaces/<workspace-id>
```

### What Happens During Execution

When you `POST /workspaces/:id/execute`, the API:

1. Spawns an ephemeral OpenCode container with the workspace volume mounted
2. The orchestrator agent dispatches sub-agents through phases: researching → planning → developing
3. Agent events (reasoning, tool calls, file edits) are stored in Postgres and streamed via SSE
4. When the agent finishes (or times out), the container is stopped
5. The diff summary is recorded on the execution record

### Execution Timeouts

The default timeout is 7200 seconds (2 hours). Override with `timeout_secs`:

```bash
curl -X POST http://localhost:8088/workspaces/<workspace-id>/execute \
  -H "Content-Type: application/json" \
  -d '{"prompt": "Fix the bug", "timeout_secs": 3600}'
```

### Workspace Isolation

Each workspace gets its own:
- **Docker volume** for the cloned source code (default 10 GB)
- **Postgres schema** for extracted code items (e.g. `ws_abc123456789`)
- **Execution containers** that are isolated from other workspaces

See [Workspace Volumes](./workspace-volumes.md) for volume management details, and [Architecture](./architecture.md#editor-playground) for the full architecture diagram.

---

## Troubleshooting

### Services Won't Start

```bash
# Check for port conflicts
sudo lsof -i :5432  # Postgres
sudo lsof -i :7474  # Neo4j HTTP
sudo lsof -i :7687  # Neo4j Bolt
sudo lsof -i :6333  # Qdrant
sudo lsof -i :11434 # Ollama

# Kill conflicting process or change ports in .env
```

### Ollama Out of Memory

```bash
# Check memory usage
docker stats rustbrain-ollama

# Use smaller quantized model (edit .env)
CODE_MODEL=codellama:7b-instruct-q4_0
```

### Reset Everything

```bash
# Stop and remove all data
docker-compose down -v

# Start fresh
bash scripts/start.sh
```

## Next Steps

- Read the [API Specification](./api-spec.md) for all available endpoints
- Learn about [Ingestion](./INGESTION_GUIDE.md) in detail
- Explore the [Architecture](./architecture.md) to understand how it works
- Set up [MCP integration](./mcp-setup.md) for Claude Code or other MCP clients
- Check [Future Scope](./future-scope.md) for planned features
