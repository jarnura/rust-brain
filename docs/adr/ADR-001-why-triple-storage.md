# ADR-001: Triple Storage Architecture

## Status

**Accepted** — 2026-03-14

## Context

Building a code intelligence platform requires supporting multiple query patterns:

1. **Semantic search** — "Find functions that handle user authentication"
2. **Graph traversal** — "Who calls this function? What implements this trait?"
3. **Metadata queries** — "Show me all pub functions in this module with their signatures"
4. **Source retrieval** — "Get the original source code for this function"

Each pattern has different access patterns and performance requirements:

| Pattern | Access Type | Index Needs |
|---------|-------------|-------------|
| Semantic search | Vector similarity | Vector index |
| Graph traversal | Edge following | Graph index |
| Metadata queries | Structured queries | B-tree indexes |
| Source retrieval | Key-value lookup | Primary key |

We considered several approaches:

1. **Single database (Postgres)** — Store everything in Postgres with pgvector extension
2. **Graph-only (Neo4j)** — Use Neo4j for all data, including embeddings
3. **Triple storage** — Postgres + Neo4j + Qdrant, each optimized for its workload

## Decision

We will use **three specialized databases**, each handling what it does best:

### Postgres (Structured Data)

**Responsibility:** Source files, extracted items, call sites, ingestion metadata

**Why Postgres:**
- ACID transactions for data integrity during ingestion
- Strong relational queries for metadata lookups
- JSONB for flexible semi-structured data (generics, attributes)
- Mature tooling and ecosystem (pgweb, exporters, backup tools)
- Full-text search on source code and comments

### Neo4j (Graph Data)

**Responsibility:** Code relationships — calls, implementations, type hierarchies

**Why Neo4j:**
- Native graph queries with Cypher are intuitive for code relationships
- Efficient traversal for "find all callers" queries
- Pattern matching for complex relationships (trait impl chains)
- APOC library for advanced graph algorithms
- Visual exploration through Neo4j Browser

### Qdrant (Vector Data)

**Responsibility:** Semantic embeddings for natural language code search

**Why Qdrant:**
- Purpose-built for vector similarity search
- HNSW algorithm for fast approximate nearest neighbor
- Payload filtering combined with vector search
- gRPC support for high-throughput embedding queries
- Efficient storage with quantization options

## Data Distribution

```
┌─────────────────────────────────────────────────────────────┐
│                     Ingestion Pipeline                       │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
    ┌──────────┐       ┌──────────┐       ┌──────────┐
    │ Postgres │       │  Neo4j   │       │  Qdrant  │
    ├──────────┤       ├──────────┤       ├──────────┤
    │ source   │       │ nodes:   │       │ vectors: │
    │ files    │       │ - Crate  │       │ - code   │
    │          │       │ - Module │       │   embeds │
    │ items:   │       │ - Func   │       │          │
    │ - fqn    │       │ - Struct │       │ payload: │
    │ - type   │       │ - Trait  │       │ - fqn    │
    │ - sig    │       │          │       │ - type   │
    │ - loc    │       │ edges:   │       │ - crate  │
    │          │       │ - CALLS  │       │          │
    │ call     │       │ - IMPLS  │       │          │
    │ sites    │       │ - CONTAINS│      │          │
    └──────────┘       └──────────┘       └──────────┘
```

### What Goes Where

| Data | Postgres | Neo4j | Qdrant |
|------|----------|-------|--------|
| Source code (raw) | ✓ | | |
| Source code (expanded) | ✓ | | |
| Item metadata (sig, vis, loc) | ✓ | ✓ (minimal) | |
| Item FQN | ✓ (primary key) | ✓ (node property) | ✓ (payload) |
| Call relationships | ✓ (call_sites) | ✓ (CALLS edges) | |
| Type relationships | | ✓ (edges) | |
| Implementations | | ✓ (IMPL edges) | |
| Embeddings | | | ✓ (vectors) |
| Git blame | ✓ | | |
| Ingestion state | ✓ | | |

## Consequences

### Positive

1. **Query Performance** — Each database is optimized for its workload:
   - Postgres: B-tree indexes for structured queries
   - Neo4j: Native graph traversal for relationship queries
   - Qdrant: HNSW for sub-millisecond vector search

2. **Scalability** — Can scale each database independently based on workload

3. **Tooling** — Each database comes with specialized tools:
   - Postgres: pgweb, psql, mature backup/restore
   - Neo4j: Browser for visual exploration
   - Qdrant: Dashboard for vector management

4. **Fault Isolation** — If one database fails, others may still serve queries

### Negative

1. **Operational Complexity** — Three databases to monitor, backup, and maintain

2. **Data Consistency** — No cross-database transactions; must handle consistency in application layer

3. **Resource Usage** — Three database processes consume more memory:
   - Postgres: ~2GB
   - Neo4j: ~4GB
   - Qdrant: ~4GB
   - Total: ~10GB minimum

4. **Ingestion Complexity** — Pipeline must coordinate writes to three destinations

### Mitigations

1. **Operational Complexity**
   - Single `docker-compose.yml` manages all services
   - `start.sh` / `stop.sh` scripts for one-command operations
   - `healthcheck.sh` verifies all services together

2. **Data Consistency**
   - FQN as the universal join key
   - Ingestion runs are tracked in Postgres
   - Idempotent upserts allow re-ingestion

3. **Resource Usage**
   - Documented minimum requirements (18GB total with Ollama)
   - Configurable memory limits in docker-compose
   - Can run subsets for development

4. **Ingestion Complexity**
   - Single pipeline writes to all three in sequence
   - Failure tracking in ingestion_runs table
   - Retry logic per-destination

## Alternatives Considered

### Single Database (Postgres + pgvector)

**Pros:** Single operational surface, ACID transactions, simpler architecture

**Cons:**
- Graph queries are awkward (recursive CTEs are slow)
- Vector search performance inferior to specialized engines
- Would need extensions for graph operations

**Verdict:** Not chosen due to poor graph traversal performance

### Graph-Only (Neo4j)

**Pros:** Excellent for relationship queries, unified data model

**Cons:**
- Not designed for large text storage (source files)
- No native vector search (requires plugin)
- Metadata queries slower than Postgres

**Verdict:** Not chosen due to poor fit for source storage and vector search

### Document Store (MongoDB + Atlas Vector)

**Pros:** Flexible schema, can store everything together

**Cons:**
- No native graph traversal
- Vector search quality varies
- Less mature relational query support

**Verdict:** Not chosen due to lack of native graph capabilities

## References

- [PostgreSQL Documentation](https://www.postgresql.org/docs/16/)
- [Neo4j Graph Database](https://neo4j.com/docs/)
- [Qdrant Vector Database](https://qdrant.tech/documentation/)
- [pgvector Extension](https://github.com/pgvector/pgvector)
