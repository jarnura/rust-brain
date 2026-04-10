# rust-brain

A code intelligence platform that gives LLMs database-like access to Rust codebases.
Ingests Rust source code and builds a queryable knowledge graph with semantic search,
call graph traversal, trait resolution, and monomorphization tracking.

## Architecture

Triple-storage design — each database optimized for its access pattern:

| Database | Role | Query | What it stores |
|----------|------|-------|----------------|
| Postgres 16 | Relational store | SQL (sqlx 0.8) | Source files, extracted items (FQN, signature, body, docs), call sites, ingestion tracking |
| Neo4j 5 | Code graph | Cypher (neo4rs 0.7) | Function/struct/trait/impl nodes, CALLS/IMPLEMENTS/CONTAINS/FOR_TYPE edges, module hierarchy |
| Qdrant 1.12 | Vector store | REST | Semantic embeddings (qwen3-embedding:4b, 2560-dim cosine) for natural language code search |

No atomic consistency across the three stores. Failed writes in one don't roll back the others.

## Crate Map

```
crates/rustbrain-common/     Shared types (Item, ItemType, CallSite, TraitImpl), errors, config, logging
services/ingestion/          6-stage pipeline: expand → parse → typecheck → extract → graph → embed
  src/parsers/                 DualParser: syn (deep analysis) + tree-sitter (fast skeleton, fallback)
  src/typecheck/resolver.rs    Call site extraction, trait impl detection via rust-analyzer subprocess
  src/pipeline/                PipelineRunner, stage definitions, context/state/config
  src/graph/                   Neo4j node/relationship creation, batch writes
  src/embedding/               Ollama HTTP client, Qdrant vector store, text representation formatting
services/api/                REST API server (Axum 0.7, 49 routes)
  src/handlers/                search, items, graph, typecheck, chat, artifacts, tasks, pg_query, health, consistency, workspace
services/mcp/                MCP protocol bridge (stdio + SSE transport on :3001, 16 tools)
```

## API Surface

**Code Intelligence (12 endpoints):**
- `POST /tools/search_semantic` — vector search via Qdrant
- `POST /tools/search_docs` — documentation search via Qdrant doc_embeddings
- `POST /tools/aggregate_search` — cross-DB fan-out (Qdrant + Postgres + Neo4j)
- `GET /tools/get_function` — full function details with source
- `GET /tools/get_callers` — direct and transitive callers from call graph
- `GET /tools/get_trait_impls` — all implementations of a trait
- `GET /tools/find_usages_of_type` — where a type is used
- `GET /tools/get_module_tree` — module hierarchy
- `POST /tools/query_graph` — raw Cypher (read-only)
- `POST /tools/pg_query` — read-only SQL queries
- `GET /tools/find_calls_with_type` — call sites with specific type arg (turbofish)
- `GET /tools/find_trait_impls_for_type` — trait impls for a concrete type

**Chat (10 endpoints):** `/tools/chat`, `/tools/chat/stream`, sessions CRUD, fork, abort

**System:** `/health`, `/metrics`, `/api/snapshot`, `/api/ingestion/progress`, `/api/consistency`, `/health/consistency`, `/playground/*`

**CRUD:** `/api/artifacts`, `/api/tasks`

**MCP Tools (16):** search_code, search_docs, get_function, get_callers, get_trait_impls, find_usages_of_type, get_module_tree, query_graph, find_calls_with_type, find_trait_impls_for_type, pg_query, aggregate_search, context_store, status_check, task_update, consistency_check

## Ingestion Pipeline

Six stages, runs containerized (32GB memory limit):

1. **Expand** — `cargo expand` to resolve macros; cached in `/tmp/rustbrain-expand-cache`
2. **Parse** — DualParser: syn for deep semantics, tree-sitter fallback if syn fails
3. **Typecheck** — rust-analyzer subprocess for call sites, trait impls, turbofish resolution
4. **Extract** — Combine parse + typecheck results → Postgres `extracted_items`
5. **Graph** — Convert to Neo4j nodes (Function, Struct, Trait, Impl, etc.) and edges (CALLS, IMPLEMENTS, FOR_TYPE, etc.)
6. **Embed** — Format items as text → Ollama embeddings → Qdrant vectors

Pipeline supports `fail_fast` (stop on first error) or graceful partial success.

## Infrastructure

17 Docker containers (~48GB RAM total). Key services:

| Service | Port | Memory | Purpose |
|---------|------|--------|---------|
| Postgres | 5432 | 6GB | Relational storage |
| Neo4j | 7474/7687 | 12GB | Graph database |
| Qdrant | 6333/6334 | 12GB | Vector search |
| Ollama | 11434 | 16GB | Embedding model (GPU optional) |
| API | 8088 | 1GB | REST server |
| MCP-SSE | 3001 | 256MB | MCP bridge |
| Grafana | 3000 | — | Dashboards |
| Prometheus | 9090 | — | Metrics |

## Tech Stack

Rust 2021, Tokio 1.37, Axum 0.7, sqlx 0.8 (compile-time checked), neo4rs 0.7,
tree-sitter 0.24 + syn 2.0, clap 4.5, rayon 1.10, reqwest 0.12, prometheus 0.13

## Coding Standards

- Conventional commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`
- Commit trailers: `Co-Authored-By: Paperclip <noreply@paperclip.ing>`
- Error handling: `anyhow` for application code, `thiserror` for library crates
- No `unsafe` code without an ADR in `docs/`
- All public APIs must have doc comments
- TDD: write failing test first, target 80%+ coverage on critical paths
- `cargo clippy --all-targets` must be clean before committing
- `cargo fmt --check` must pass

## Query Cost Hierarchy

When querying the knowledge graph, follow this token-efficient order:

```
P0: pg_query        (~50 tokens)      — use first, unlimited
P1: neo4j_query     (~200-500 tokens) — max 8 per task
P2: qdrant_search   (~300-600 tokens) — max 5 per task
P3: read_file/grep  (~4-300 tokens)   — max 1500 lines / 5 calls per task
```

## Key Documentation

- `docs/architecture.md` — full system design, graph schema, design rationale
- `docs/TESTING_GUIDE.md` — three-layer verification, CLASS A test matrix
- `docs/agent-prompts/` — 13 SDLC agent prompt designs (orchestrator, explorer, developer, etc.)
- `docs/runbook.md` — operations, health checks, troubleshooting
- `docs/getting-started.md` — setup and first run
- `docs/mcp-setup.md` — MCP server configuration for Claude/Cursor/OpenCode
- `docs/INGESTION_GUIDE.md` — ingestion walkthrough
- `docs/api-spec.md` — endpoint reference

## Quick Commands

```bash
bash scripts/start.sh              # Start all 17 containers
bash scripts/healthcheck.sh        # Verify all services healthy
./scripts/ingest.sh /path/to/crate # Run ingestion pipeline
curl http://localhost:8088/health   # API health check
docker logs rustbrain-api --tail 50 # API server logs
```
