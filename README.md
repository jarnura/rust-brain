# rust-brain

A production-grade Rust code intelligence platform. Ingests Rust codebases and builds a queryable knowledge graph with semantic search, call graph traversal, trait resolution, and monomorphization tracking.

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    ORCHESTRATION AGENT                               │
│            Plans · Delegates · Verifies · Documents                  │
└─────┬──────────┬──────────┬──────────┬──────────┬──────────┬───────┘
      │          │          │          │          │          │
      ▼          ▼          ▼          ▼          ▼          ▼
  ┌────────┐┌────────┐┌────────┐┌────────┐┌────────┐┌────────┐
  │ Infra  ││Pipeline││Pipeline││Pipeline││Pipeline││Service │
  │ Agents ││Expand  ││ Parse  ││Typecheck││ Graph ││ Agents │
  └────┬───┘└───┬────┘└───┬────┘└───┬────┘└───┬────┘└───┬────┘
       │         │         │         │         │         │
       ▼         └─────────┴─────────┴─────────┘         ▼
  ┌─────────────────────────────────────────────────────────┐
  │                     Docker Compose                       │
  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────────┐   │
  │  │ Postgres │ │  Neo4j   │ │  Qdrant  │ │  Ollama   │   │
  │  │  + Pgweb │ │ +Browser │ │+Dashboard│ │ + Models  │   │
  │  └──────────┘ └──────────┘ └──────────┘ └───────────┘   │
  │  ┌──────────────────────────────────────────────────┐   │
  │  │     Prometheus → Grafana (6 dashboards)          │   │
  │  └──────────────────────────────────────────────────┘   │
  └─────────────────────────────────────────────────────────┘
```

## Quick Start

```bash
cd rust-brain
cp .env.example .env
bash scripts/start.sh
bash scripts/healthcheck.sh
```

## Service Endpoints

| Service | URL | Purpose |
|---------|-----|---------|
| Grafana | http://localhost:3000 | Observability dashboards |
| Neo4j Browser | http://localhost:7474 | Graph exploration |
| Qdrant Dashboard | http://localhost:6333/dashboard | Vector DB management |
| Pgweb | http://localhost:8081 | Postgres query UI |
| Prometheus | http://localhost:9090 | Metrics & alerting |
| Ollama API | http://localhost:11434 | Embedding & LLM inference |
| Tool API | http://localhost:8088 | Agent-facing tool endpoints |

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

## Design Decisions

- **Triple storage**: Neo4j (graph traversal) + Qdrant (semantic search) + Postgres (raw data) — each DB does what it's best at
- **Dual parsing**: tree-sitter (fast skeleton) + syn (deep analysis) — speed where possible, accuracy where needed
- **Lazy monomorphization**: Store generics as-is, index concrete call sites, resolve on query — avoids compilation cost
- **Local AI**: Ollama for embeddings and code understanding — no external API dependency, full data privacy
- **Monorepo-first**: FQN scheme and graph schema support multi-repo with zero schema changes

## Status

See [PROJECT_STATE.md](./PROJECT_STATE.md) for current phase and task status.
