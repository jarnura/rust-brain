# rust-brain

[![CI](https://github.com/jarnura/rust-brain/actions/workflows/ci.yml/badge.svg)](https://github.com/jarnura/rust-brain/actions/workflows/ci.yml)

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

### Code Intelligence (12 endpoints)

| Endpoint | Purpose |
|----------|---------|
| `POST /tools/search_semantic` | Natural language code search |
| `POST /tools/search_docs` | Documentation semantic search |
| `POST /tools/aggregate_search` | Cross-database aggregated search |
| `GET /tools/get_function?fqn=` | Full function details with source |
| `GET /tools/get_callers?fqn=` | Direct and transitive callers |
| `GET /tools/get_trait_impls?trait_name=` | All implementations of a trait |
| `GET /tools/find_usages_of_type?type_name=` | Where a type is used |
| `GET /tools/get_module_tree?crate=` | Module hierarchy |
| `POST /tools/query_graph` | Raw Cypher queries |
| `POST /tools/pg_query` | Read-only SQL queries |
| `GET /tools/find_calls_with_type?type_name=` | Call sites with specific type argument (turbofish) |
| `GET /tools/find_trait_impls_for_type?type_name=` | All trait implementations for a given type |

### Chat (10 endpoints)

| Endpoint | Purpose |
|----------|---------|
| `POST /tools/chat` | Send chat message |
| `GET /tools/chat/stream` | SSE streaming chat |
| `POST /tools/chat/send` | Send message to stream |
| `POST /tools/chat/sessions` | Create session |
| `GET /tools/chat/sessions` | List sessions |
| `GET /tools/chat/sessions/:id` | Get session details |
| `DELETE /tools/chat/sessions/:id` | Delete session |
| `POST /tools/chat/sessions/:id/fork` | Fork a session |
| `POST /tools/chat/sessions/:id/abort` | Abort streaming session |

### System (5 endpoints)

| Endpoint | Purpose |
|----------|---------|
| `GET /health` | Service health with per-store counts |
| `GET /metrics` | Prometheus metrics |
| `GET /api/snapshot` | Snapshot info |
| `GET /api/consistency` | Cross-store consistency check |
| `GET /api/ingestion/progress` | Ingestion progress |

## Key Files

```
ORCHESTRATOR_PROMPT.md   ← Master orchestration agent prompt
docker-compose.yml       ← Infrastructure definition
.env.example             ← All configurable variables
PROJECT_STATE.md         ← Current project state
```

## Playground

The playground UI at **http://localhost:8088/playground** provides 4 pages for interactive exploration:

- **Dashboard** (`index.html`): Real-time service health, ingestion stats, quick actions
- **Query Playground** (`playground.html`): 7 query types (semantic, function, callers, trait impls, type usages, module tree, Cypher) with JSON/table view toggle
- **Audit Trail** (`audit.html`): Known issues and system audit information
- **Gap Analysis** (`gaps.html`): Feature completeness tracking

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

## CI/CD

### What Runs in CI

The CI pipeline (`.github/workflows/ci.yml`) runs automatically on all pull requests and pushes to `main`:

| Job | Command | Description |
|-----|---------|-------------|
| **fmt** | `cargo fmt --check` | Enforces code formatting |
| **clippy** | `cargo clippy --all-targets -- -D warnings` | Linting (warnings = failures) |
| **test** | `cargo test --workspace` | Unit tests only |
| **build** | `cargo build --release` | Release compilation |
| **nightly** | `cargo check --workspace` | Future compatibility check |

### What Requires Docker

Integration tests require the full docker-compose stack (Postgres, Neo4j, Qdrant, Ollama). These are **not run in CI** due to resource constraints and are marked with `#[ignore]` in the codebase.

To run integration tests locally:

```bash
# Start the infrastructure
./scripts/start.sh

# Wait for services to be healthy
./scripts/healthcheck.sh

# Run integration tests (includes ignored tests)
cargo test --workspace -- --include-ignored

# Or run specific integration test files
cargo test --test api_integration -- --include-ignored
```

**Integration test requirements:**
- Postgres (DATABASE_URL)
- Neo4j (NEO4J_URL)  
- Qdrant (QDRANT_URL)
- Ollama (EMBEDDING_URL) — optional for some tests

### Branch Protection Recommendations

For the `main` branch, configure these settings in GitHub (Settings → Branches → Add rule):

1. **Required status checks:**
   - `fmt` — Format check
   - `clippy` — Linting
   - `test` — Unit tests
   - `build` — Release build

2. **Additional settings:**
   - ✅ Require a pull request before merging
   - ✅ Require approvals (recommended: 1)
   - ✅ Require status checks to pass before merging
   - ✅ Require branches to be up to date before merging
   - ✅ Require linear history (optional, keeps history clean)

3. **Do NOT require:**
   - `nightly` job (allowed to fail, catches future breaking changes)
   - `integration` job (only runs on main branch pushes)

## Status

See [PROJECT_STATE.md](./PROJECT_STATE.md) for current phase and task status.
