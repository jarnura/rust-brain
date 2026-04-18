# ADR-005: Multi-Tenancy Physical Isolation for Qdrant and Neo4j

## Status

**Accepted (Phase 1)** — 2026-04-15
**Accepted (Phase 3 = Option B+, composite labels + enforced middleware)** — 2026-04-16

Supersedes the "Phase 2" note in [ADR-003](ADR-003-workspace-isolation.md).

CEO decisions (2026-04-15):
- Neo4j Enterprise budget: $0 — open-source platform cannot depend on proprietary licensing
- Phase 1 (Qdrant + Postgres) is the next-release milestone; Neo4j isolation deferred
- AGE POC approved, time-boxed to 1 sprint; if it fails, cost analysis required before Enterprise

CEO decisions (2026-04-16):
- AGE POC complete — NO-GO ([RUSA-191](/RUSA/issues/RUSA-191))
- **Phase 3 path: Option B+ approved.** Structural enforcement distinguishes it from rejected Option E.
- Enterprise cost analysis NOT triggered. Re-escalate only on mid-implementation blocker.
- Defer-to-AGE-1.8 rejected. 6-12 months unisolated is unacceptable.

### Phase 3 Guardrails (non-negotiable)

Per CEO 2026-04-16 — Option B+ correctness depends on enforcement holding:

1. **Typed `WorkspaceGraphClient` is the ONLY graph entry point.** Direct `neo4rs` usage in handlers = CI failure. Enforce via clippy lint or grep-based CI check.
2. **Cross-workspace leak tests run in CI**, not just locally. Create workspace A and B, query from A, assert zero B nodes returned. Required on every PR touching the graph layer.
3. **Audit/leak-detection job is a required deploy artifact.** Not optional. Includes runbook entry for alert response.
4. **`query_graph` user Cypher injection trust boundary documented.** Label predicate prepended server-side, users cannot bypass. Test suite for malformed user Cypher required.
5. **Container-per-workspace escape hatch documented as opt-in** in ops guide for regulated/high-isolation workloads.

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

### AGE POC Result (2026-04-16): NO-GO

[RUSA-191](/RUSA/issues/RUSA-191) completed. Apache AGE 1.6.0 fails the decision gate on three counts:

1. **Variable-length edge traversal** (`[*1..5]`) is 200x+ slower than Neo4j with no cycle deduplication — kills our most-used API feature (transitive callers).
2. **`MERGE ON CREATE SET`** is unsupported (PR #2347 unreleased) — breaks our idempotent batch ingestion pattern.
3. **Batch UNWIND+MERGE** writes 10K edges in 22 minutes vs Neo4j's sub-second — makes ingestion impractical at our scale.

Plus: no Cypher-level constraints, no parameterized queries, indexes don't integrate with the Cypher planner. Re-evaluation possible in 6-12 months when AGE 1.8+ ships these fixes.

Full report: [RUSA-191#document-poc-report](/RUSA/issues/RUSA-191#document-poc-report).

### Post-POC Path: Composite Labels with Enforced Middleware (Option B+)

Per CEO direction (no Enterprise spend without cost analysis), and Option D (containers) capping at 2-3 workspaces, the only scalable Community-compatible path is **composite labels with strict middleware enforcement** — a hardened version of Option B.

**Why this is qualitatively different from the rejected Option E:**

The board rejected "just adding workspace_id everywhere" because it relies on developers remembering to filter on every query. We address that risk with compile-time and runtime enforcement:

1. **No raw Cypher in handlers.** All Neo4j access goes through a single `WorkspaceGraphClient` type that requires a `WorkspaceContext` at construction.
2. **Workspace label injected automatically.** Every node gets a `:Workspace_<id>` label at creation. The client appends the label predicate to every MATCH/MERGE.
3. **Cypher templates only.** Handlers use named templates (`get_callers`, `get_trait_impls`, etc.) — no string concatenation. Templates carry the workspace filter as a required parameter.
4. **Audit logging.** Every Cypher execution logs the workspace context. Cross-workspace leak detection runs as a periodic job comparing query workspace vs returned node workspaces.
5. **Read-only API user remains read-only.** The `query_graph` endpoint that accepts user Cypher gets workspace label injection at the API layer before execution.

**What this is not:** It is not "just an id filter." It is enforced at the type level (cannot construct a graph client without a workspace), at the query layer (templates only, no raw strings in handlers), and at the audit layer (logged + monitored).

**Trade-offs accepted:**
- Logical isolation, not physical. A bug in the middleware could leak data — mitigated by audit logging and integration tests.
- The `query_graph` endpoint's flexibility is constrained — users cannot run arbitrary Cypher across workspaces.
- Adding a new query template requires explicit workspace parameter handling — guards against drift.

**Container-per-workspace (Option D) remains available as an opt-in for high-isolation deployments** (e.g., regulated tenants) once we have demand for it. Single-tenant deployments keep the shared Neo4j by default.

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

### Phase 3 (revised, 2026-04-16): Composite Labels with Enforced Middleware (3-4 weeks)

AGE POC failed. Per CEO direction, Enterprise is last-resort. The chosen path:

1. **Build `WorkspaceGraphClient`** in `services/api/src/neo4j.rs` — typed wrapper requiring `WorkspaceContext` at construction
2. **Workspace label injection** in `services/ingestion/src/graph/nodes.rs` — every node gets `:Workspace_<id>`
3. **Cypher template registry** — convert all handlers to use named templates with workspace parameter
4. **Audit logging + leak detection job** — periodic Cypher scan comparing returned node workspace labels against query context
5. **Update `query_graph` endpoint** — inject workspace label predicate at API layer before user Cypher executes
6. **Integration tests** — extend RUSA-190 to cover Neo4j workspace isolation

Container-per-workspace (Option D) is documented as an opt-in for regulated/high-isolation tenants but is not the default.

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
