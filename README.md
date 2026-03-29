# rust-brain

A production-grade Rust code intelligence platform. Ingests Rust codebases and builds a queryable knowledge graph with semantic search, call graph traversal, trait resolution, and monomorphization tracking.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                         MULTI-AGENT ENVIRONMENT                              │
│  OpenCode (IDE) → LiteLLM (Model Proxy) → Claude (Orchestrator)             │
└──────────────────────────────────────────────────────────────────────────────┘
                              ▲
                              │ MCP-SSE
                              │
┌─────────────────────────────┴──────────────────────────────────────────────┐
│                      rust-brain Playground                                   │
│  Dashboard │ Search │ Call Graph │ Chat │ Cypher │ Types │ Traits │ Modules │
└──────────────────────────────┬───────────────────────────────────────────────┘
                              │ REST API
      ┌───────────────────────┴─────────────────────┐
      ▼                                             ▼
┌──────────────────────────┐          ┌──────────────────────────┐
│    MCP Server (Stdio)    │          │  MCP Server (SSE-HTTP)   │
│  Tool definitions + args │          │  WebSocket stream        │
│  Invoke endpoints        │          │  Streaming responses     │
└──────────────────────────┘          └──────────────────────────┘
      │                                             │
      └─────────────────────┬───────────────────────┘
                            ▼
      ┌─────────────────────────────────────────────┐
      │         rust-brain Services                 │
      │  ┌──────────────────────────────────────┐   │
      │  │  Tool API (Semantic Search, Graph)   │   │
      │  └──────────────────────────────────────┘   │
      │  ┌──────────────────────────────────────┐   │
      │  │  Ingestion Pipeline (Parse, Typecheck)   │
      │  └──────────────────────────────────────┘   │
      └─────────────────────────────────────────────┘
                            ▼
      ┌─────────────────────────────────────────────┐
      │         Data Layer (Docker Compose)         │
      │  ┌──────────┐ ┌──────────┐ ┌──────────┐     │
      │  │ Postgres │ │  Neo4j   │ │  Qdrant  │     │
      │  │  + Pgweb │ │ +Browser │ │+Dashboard│     │
      │  └──────────┘ └──────────┘ └──────────┘     │
      │  ┌──────────┐ ┌──────────────────────────┐  │
      │  │  Ollama  │ │  Prometheus → Grafana    │  │
      │  │ +Models  │ │  (3 dashboards)          │  │
      │  └──────────┘ └──────────────────────────┘  │
      └─────────────────────────────────────────────┘
```

## Quick Start (Snapshot)

Run rust-brain with a pre-built snapshot of the [Hyperswitch](https://github.com/juspay/hyperswitch) codebase. **No ingestion, no Ollama, no GPU required.**

```bash
git clone https://github.com/jarnura/rust-brain.git && cd rust-brain
./scripts/run-with-snapshot.sh
```

This downloads a ~1.9GB snapshot containing 161K indexed code items, restores all three databases, and starts the API + MCP server. Takes ~2-7 minutes on first run.

**Open the playground:** http://localhost:8088

**Connect your IDE** (Claude Code, Claude Desktop, Cline, OpenCode):

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

Add to `~/.claude.json` for Claude Code, `claude_desktop_config.json` for Claude Desktop, or VS Code settings for Cline.

**Requirements:** Docker >= 24.0 with Compose V2, ~8GB RAM, ~4GB disk, zstd.

## Getting Started (Full Setup)

For running your own ingestion pipeline:

```bash
cd rust-brain
cp .env.example .env

# Add your API key
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."  # Optional: for alternative models

# Start the platform (includes Ollama for embeddings)
bash scripts/start.sh
bash scripts/healthcheck.sh

# Ingest a codebase
./scripts/ingest.sh ~/projects/my-rust-repo
```

### Create Your Own Snapshot

After ingesting your codebase, create a shareable snapshot:

```bash
./scripts/create-snapshot.sh my-project abc1234
# Output: dist/rustbrain-snapshot-my-project.tar.zst
```

Share the file with teammates — they run `./scripts/run-with-snapshot.sh --local=path/to/snapshot.tar.zst`.

## Ingestion CLI

```
rustbrain-ingestion [OPTIONS]
```

| Flag | Short | Default | Env Var | Description |
|------|-------|---------|---------|-------------|
| `--crate-path <PATH>` | `-c` | `.` | — | Path to the Rust crate to ingest |
| `--database-url <URL>` | `-d` | — | `DATABASE_URL` | Postgres connection URL (required) |
| `--neo4j-url <URL>` | — | — | `NEO4J_URL` | Neo4j bolt URL |
| `--embedding-url <URL>` | — | — | `EMBEDDING_URL` | Ollama embedding endpoint |
| `--stages <STAGES>` | `-s` | all | — | Comma-separated list of stages to run |
| `--dry-run` | — | `false` | — | Parse and validate without writing to DBs |
| `--fail-fast` | — | `false` | — | Stop on first stage error |
| `--max-concurrency <N>` | — | `4` | — | Maximum parallel tasks |
| `--verbose` | `-v` | `false` | — | Enable debug logging |

```bash
# Ingest a crate with custom concurrency
rustbrain-ingestion -c ./my-crate -d postgres://... --max-concurrency 8

# Dry run to validate parsing only
rustbrain-ingestion -c ./my-crate --dry-run -v

# Run specific stages only
rustbrain-ingestion -c ./my-crate -s parse,extract,embed
```

## Service Endpoints

| Service | URL | Purpose |
|---------|-----|---------|
| **rust-brain Playground** | http://localhost:8088/playground | Interactive UI for code exploration |
| **OpenCode IDE** | http://localhost:4096 | AI IDE with MCP integration |
| **LiteLLM Proxy** | (external — not a local container) | Model routing & fallbacks |
| **MCP SSE Server** | ws://localhost:3001 | Streaming tool transport |
| Grafana | http://localhost:3000 | Observability dashboards |
| Neo4j Browser | http://localhost:7474 | Graph exploration |
| Qdrant Dashboard | http://localhost:6333/dashboard | Vector DB management |
| Pgweb | http://localhost:8085 | Postgres query UI |
| Prometheus | http://localhost:9090 | Metrics & alerting |
| Ollama API | http://localhost:11434 | Embedding & LLM inference |
| Tool API | http://localhost:8088/tools | REST endpoints for code tools |

## Ingestion Pipeline

```
Rust Crate → cargo expand → tree-sitter + syn → rust-analyzer → Extract → Neo4j Graph → Qdrant Embeddings
     ↓                                                            ↓
Postgres (raw source, git blame)                          Postgres (extracted items)
```

## Agent Tool API

| Endpoint | Purpose |
|----------|---------|
| `POST /tools/search_semantic` | Natural language code search |
| `GET /tools/get_function?fqn=` | Full function details with source |
| `GET /tools/get_callers?fqn=` | Direct and transitive callers |
| `GET /tools/get_trait_impls?trait_name=` | All implementations of a trait |
| `GET /tools/find_usages_of_type?type_name=` | Where a type is used |
| `GET /tools/get_module_tree?crate=` | Module hierarchy |
| `POST /tools/query_graph` | Raw Cypher queries |

## Key Files

```
ORCHESTRATOR_PROMPT.md   ← Master orchestration agent prompt
docker-compose.yml       ← Infrastructure definition
.env.example             ← All configurable variables
PROJECT_STATE.md         ← Current project state
```

## Playground

The unified playground UI provides interactive exploration of Rust codebases with multiple views:

- **Dashboard**: Overview of ingestion status, metrics, recent searches
- **Search**: Semantic code search with full-text fallback
- **Call Graph**: Interactive dependency graph visualization
- **Chat**: AI-powered code exploration with streaming responses
- **Cypher**: Raw Neo4j query interface for graph analysis
- **Types**: Browse types, structs, enums, and their implementations
- **Traits**: Trait definitions, implementations, and bounds
- **Modules**: Module hierarchy and export structure
- **Audit**: Code quality metrics, dependency analysis
- **Gaps**: Missing implementations, trait coverage analysis

**Keyboard Shortcuts:**
- `Cmd+K` / `Ctrl+K` — Command palette
- `Cmd+1` to `Cmd+9` — Jump to panel (1=Dashboard, 2=Search, etc.)
- `Cmd+/` — Toggle chat sidebar
- `Cmd+B` — Toggle detail panel
- `Escape` — Close overlays

See [docs/playground.md](./docs/playground.md) for detailed documentation.

## OpenCode Integration

rust-brain integrates with OpenCode IDE and LiteLLM for multi-model AI assistance:

- **OpenCode**: Browser-based IDE with MCP tool support
- **LiteLLM**: Model router with fallback chains and cost optimization
- **MCP (Model Context Protocol)**: Standardized tool interface with SSE streaming
- **Claude**: Deep code intelligence and reasoning

See [docs/opencode-integration.md](./docs/opencode-integration.md) for architecture and setup.

## Design Decisions

- **Triple storage**: Neo4j (graph traversal) + Qdrant (semantic search) + Postgres (raw data) — each DB does what it's best at
- **Dual parsing**: tree-sitter (fast skeleton) + syn (deep analysis) — speed where possible, accuracy where needed
- **Lazy monomorphization**: Store generics as-is, index concrete call sites, resolve on query — avoids compilation cost
- **Local embeddings**: Ollama for code embeddings (qwen3-embedding:4b, 2560 dims) — full data privacy, no external API dependency
- **Monorepo-first**: FQN scheme and graph schema support multi-repo with zero schema changes
- **Streaming responses**: SSE-based MCP transport for real-time tool invocations in IDEs
- **Model flexibility**: LiteLLM routing supports Anthropic, OpenAI, local models with transparent fallbacks

## Status

See [PROJECT_STATE.md](./PROJECT_STATE.md) for current phase and task status.
