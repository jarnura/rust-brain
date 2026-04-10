# ADR-003: Workspace Isolation Strategy

## Status

**Accepted** — 2026-04-08

## Context

The Editor Playground feature allows users to submit GitHub repositories for AI-assisted code development. Each workspace represents an independent codebase with its own:

- Cloned repository files (read-write for the AI agent)
- Indexed code intelligence data (Postgres, Neo4j, Qdrant)
- Running execution containers (OpenCode sessions)

Multiple users may have concurrent workspaces. We need isolation guarantees to prevent:

1. Cross-workspace data leakage in queries
2. File system interference between workspaces
3. Resource contention between concurrent executions

## Decision

**Three-layer isolation:**

### Layer 1: Docker Volumes for File System Isolation

Each workspace gets a dedicated Docker volume (`rustbrain-ws-<12hex>`).

- Repository is cloned to a host temp directory, then copied into the volume via `busybox cp`
- The volume is mounted read-write into per-execution OpenCode containers
- The volume is mounted read-only into the ingestion container during indexing
- Volumes are user-controlled (no auto-TTL); teardown is explicit via `DELETE /workspaces/:id`

**Why not bind mounts?** Docker volumes are portable across hosts, managed by the Docker daemon, and don't depend on host directory permissions. Bind mounts would require coordinating UIDs between the host and containers.

### Layer 2: Postgres Schema-per-Workspace for Data Isolation

Each workspace gets its own Postgres schema (`ws_<12hex>`) containing:

- `source_files` — raw source code
- `extracted_items` — functions, structs, traits with FQN, signature, body
- `call_sites` — call relationships

The ingestion pipeline connects with `?options=--search_path=ws_<id>,public` on the `DATABASE_URL`, so all `sqlx` queries automatically scope to the workspace schema without code changes.

**Why not separate databases?** Schema isolation is lighter-weight. A single connection pool serves all workspaces. Cross-workspace queries (future: workspace comparison) remain possible. Schema `CREATE`/`DROP` is fast and doesn't require database-level permissions.

### Layer 3: Per-Execution Ephemeral Containers

Each execution (not workspace) spawns a fresh OpenCode container:

- Container name: `rustbrain-exec-<8hex>`
- Mounts the workspace volume at `/workspace:rw`
- Connects to the `rustbrain-net` Docker network for MCP/API access
- Removed on execution completion (success, failure, or timeout)

**Why per-execution, not per-workspace?** A per-workspace long-running container would accumulate state across executions (environment modifications, cached processes, temp files). Ephemeral containers guarantee a clean environment for each execution. The cost is ~2-3s container startup time, which is negligible relative to multi-minute agent execution times.

## Consequences

### Positive

- Strong isolation: file, data, and process boundaries between workspaces
- Clean execution environment: no cross-execution state bleed
- Lightweight multi-tenancy: single Postgres instance serves all workspaces
- User-controlled lifecycle: no surprise data loss from auto-cleanup

### Negative

- Volume accumulation: requires user awareness of disk usage. Future: add monitoring alerts.
- Schema proliferation: hundreds of workspaces means hundreds of schemas. Postgres handles this fine, but `pg_dump` becomes noisy.
- No cross-store atomicity: if volume creation succeeds but schema creation fails, manual cleanup is needed. The teardown flow must handle partial state.
- Orphaned resources: if the API crashes mid-creation, volumes and schemas may be left behind. A startup reconciliation sweep is recommended (future work).

### Neo4j and Qdrant Isolation (Not Yet Implemented)

The current implementation does **not** isolate Neo4j graph data or Qdrant vectors per workspace. All workspaces share the same Neo4j database and Qdrant collections. This is acceptable for MVP because:

1. Graph queries filter by `source_url` or `crate_name`, providing logical isolation
2. Qdrant search results include `crate_name` metadata for filtering
3. True per-workspace isolation in Neo4j (separate databases) and Qdrant (separate collections) is planned for Phase 2

This is a known trade-off. If workspace A and workspace B index repos with the same crate name, graph traversals may return mixed results. For MVP (Rust-only, single-user), this risk is low.

## Related

- [ADR-001: Triple Storage Architecture](ADR-001-why-triple-storage.md) — rationale for Postgres + Neo4j + Qdrant
- [Editor Playground Plan](/RUSA/issues/RUSA-42#document-plan) — full architecture plan
- [Phase 1 Engineering Plan](/RUSA/issues/RUSA-85#document-plan) — implementation breakdown
