# ADR-005: Multi-Tenancy Physical Isolation for Qdrant and Neo4j

## Status

**Accepted (Phase 1)** — 2026-04-15
**Proposed (Phase 2-3, pending AGE POC)** — 2026-04-15

Supersedes the "Phase 2" note in [ADR-003](ADR-003-workspace-isolation.md).

CEO decisions (2026-04-15):
- Neo4j Enterprise budget: $0 — open-source platform cannot depend on proprietary licensing
- Phase 1 (Qdrant + Postgres) is the next-release milestone; Neo4j isolation deferred
- AGE POC approved, time-boxed to 1 sprint; if it fails, cost analysis required before Enterprise

## Context

ADR-003 accepted schema-per-workspace isolation for Postgres but deferred Neo4j and Qdrant isolation to Phase 2. The current state:

| Store | Write Isolation | Read Isolation | Mechanism |
|-------|----------------|----------------|-----------|
| Postgres | Per-workspace schema (`ws_<12hex>`) | None (API reads `public`) | `search_path` on ingestion |
| Qdrant | None | None | 4 global collections, `crate_name` filter only |
| Neo4j | None | None | Single Community database, no workspace properties |

This creates cross-workspace data leakage in both search and graph queries. The board has requested physical isolation — not payload-filtered shared collections.

### Constraints

- Neo4j 5 Community Edition does **not** support multiple databases. Only `system` and `neo4j` exist.
- Current infrastructure: 48GB RAM across 17 containers. Neo4j alone consumes 12GB.
- Qdrant code_embeddings: 284K vectors at 2560 dimensions (~2.7GB HNSW index per collection).
- All Cypher queries use MERGE for idempotent writes and APOC for some graph operations.

## Qdrant: Decision

**Collection-per-workspace.** Recommended.

### Rationale

Qdrant 1.12 natively supports dynamic collection management. Creating a collection per workspace provides physical isolation with no license cost and minimal code change.

**Naming convention:** `{workspace_schema_name}_{collection_type}`
- `ws_550e8400e29b_code_embeddings`
- `ws_550e8400e29b_doc_embeddings`
- `ws_550e8400e29b_crate_docs`

**Lifecycle:** Collections created during workspace provisioning, deleted on workspace teardown.

**Memory impact:** Each 284K-vector collection requires ~2.7GB for the HNSW index. Most workspaces will be much smaller (tokio-sized crates: ~10-50K vectors, ~500MB-1.3GB). Qdrant's segment management handles this efficiently.

**Cross-workspace search:** Not a code-intelligence requirement. If needed later, fan-out across collections is a query-layer concern, not a storage concern.

### Code Changes

| File | Change |
|------|--------|
| `services/ingestion/src/embedding/qdrant_client.rs` | Replace constants (`CODE_COLLECTION`, `DOC_COLLECTION`, `CRATE_DOCS_COLLECTION`) with dynamic names derived from workspace schema name |
| `services/ingestion/src/embedding/mod.rs` | Accept workspace context in embedding functions |
| `services/api/src/handlers/search.rs` | Resolve workspace → collection name before querying |
| `services/api/src/workspace/lifecycle.rs` | Create Qdrant collections on workspace creation, delete on teardown |

## Neo4j: Evaluation

Five options evaluated. Option E (property + middleware filter) is excluded per board direction.

### Option A: Neo4j Enterprise Multi-Database

| Aspect | Assessment |
|--------|------------|
| Isolation | Physical — database-per-workspace |
| License | Enterprise Edition (commercial license required) |
| RAM | ~4GB per additional database |
| Migration | Medium — change `neo4rs` connections to specify database |
| Risk | License cost may be prohibitive; requires CEO/board budget approval |

Neo4j Enterprise provides `CREATE DATABASE ws_<id>` with full isolation. Cypher queries route to the workspace database via session config. Clean solution, but the licensing cost is the primary obstacle.

### Option B: Composite Labels (`Workspace_<id>:Function`)

| Aspect | Assessment |
|--------|------------|
| Isolation | Logical — label-based filtering |
| License | Community (free) |
| RAM | Minimal additional |
| Migration | Low — add label to all node creation, prefix all queries |
| Risk | Not true isolation. Unscoped Cypher queries leak across workspaces. Board rejected this pattern. |

Functionally equivalent to property filtering. Same weakness as Option E. **Rejected.**

### Option C: Apache AGE (Postgres Graph Extension)

| Aspect | Assessment |
|--------|------------|
| Isolation | Physical — inherits Postgres schema-per-workspace |
| License | Apache 2.0 (free) |
| RAM | None additional (uses Postgres) |
| Migration | High — complete rewrite of graph layer |
| Risk | AGE's openCypher support is incomplete. MERGE behavior differs. APOC not available. Maturity gap. |

Apache AGE adds graph query capabilities to Postgres. If our graph queries can be expressed in AGE's Cypher dialect, we eliminate Neo4j entirely:
- Graph data inherits the same schema isolation Postgres already provides
- Infrastructure drops from 17 → 16 containers
- 12GB RAM freed
- One fewer database to manage operationally

**Critical unknowns requiring POC validation:**
1. Does AGE support our MERGE patterns? (AGE implements openCypher 9, MERGE support is partial)
2. Can our traversal queries (transitive callers, module tree) perform acceptably?
3. Does AGE handle our graph scale (365K nodes, 366K edges per workspace)?

### Option D: Separate Neo4j Containers per Workspace

| Aspect | Assessment |
|--------|------------|
| Isolation | Physical — instance-per-workspace |
| License | Community (free) |
| RAM | ~12GB per instance |
| Migration | Medium — dynamic container orchestration, connection routing |
| Risk | Not scalable. 48GB total budget, Neo4j already uses 12GB. Max 2-3 concurrent workspaces. |

Physical isolation via separate containers. Works with Community Edition. **Not viable** beyond 2-3 workspaces due to memory consumption.

### Neo4j: Recommendation

**Phase approach — POC Apache AGE (Option C), fallback to Enterprise (Option A).**

AGE is the architecturally strongest option: it unifies graph isolation with Postgres schema isolation, eliminates a database dependency, and is free. But it carries technical risk due to Cypher dialect differences and maturity.

1. Build a time-boxed POC (1 sprint) to validate AGE against our top 10 Cypher queries.
2. If POC passes: plan the graph layer migration as a separate epic.
3. If POC fails: escalate to CEO with Neo4j Enterprise license cost estimate for budget approval.

During the POC period, Neo4j remains unscoped — no interim half-measures.

## Postgres: Complete Read Path

The write path uses `search_path` routing. The read path (API handlers) still queries `public` schema. Completing this is straightforward:

1. Add workspace resolution middleware to API
2. Set `search_path` on the connection/transaction for each request
3. All `sqlx` queries automatically scope without code changes

This is the lowest-risk, highest-value work and should be done first.

## Implementation Phases

### Phase 1: Qdrant Collections + Postgres Read Path (2-3 weeks)

- Create per-workspace Qdrant collections during workspace provisioning
- Route embedding writes and search reads to workspace-scoped collections
- Add workspace middleware to API for Postgres read-path routing
- Migrate existing data from global collections (one-time script)
- **Result:** 2 of 3 stores fully isolated

### Phase 2: Neo4j POC with Apache AGE (1-2 weeks)

- Install AGE extension in Postgres dev instance
- Port top 10 Cypher queries to AGE's openCypher dialect
- Benchmark: correctness, performance, edge cases
- Decision gate: go/no-go on full migration

### Phase 3: Neo4j Resolution (timeline depends on Phase 2 outcome)

- **If AGE viable:** Plan and execute graph layer migration (4-6 weeks)
- **If AGE not viable:** Escalate Neo4j Enterprise licensing to CEO/board

## Consequences

### Positive

- Physical isolation for Qdrant (Phase 1) — no cross-workspace vector leakage
- Complete Postgres isolation (Phase 1) — read and write paths both scoped
- Potential Neo4j elimination (Phase 3, AGE path) — simpler infra, lower cost
- Incremental delivery — each phase is independently valuable

### Negative

- Neo4j remains unisolated until Phase 3 completes
- AGE POC consumes engineering time with no guaranteed outcome
- Per-workspace Qdrant collections increase collection count (monitoring needed)
- Migration of existing global data requires careful cutover planning

## Related

- [ADR-003: Workspace Isolation Strategy](ADR-003-workspace-isolation.md) — Phase 1 decisions this supersedes
- [ADR-001: Triple Storage Architecture](ADR-001-why-triple-storage.md) — rationale for three stores (may change if AGE replaces Neo4j)
- [RUSA-180](/RUSA/issues/RUSA-180) — parent task
- [RUSA-179](/RUSA/issues/RUSA-179) — board request
