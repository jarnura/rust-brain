# ADR-006: Neo4j Workspace Label Migration for Existing Global Nodes

## Status

**Proposed** — 2026-04-17

## Context

[RUSA-195](/RUSA/issues/RUSA-195) implements workspace label injection on all Neo4j node writes in the ingestion pipeline (Phase 3 of [RUSA-180](/RUSA/issues/RUSA-180), per [ADR-005](ADR-005-multi-tenancy-physical-isolation.md) Option B+).

The ingestion pipeline now conditionally includes a `:Workspace_<12hex>` label on every MERGE, MATCH, and UNWIND operation across `nodes.rs`, `batch.rs`, and `relationships.rs`. The `GraphStage` validates that `workspace_label` is present and refuses to run without it.

**Problem:** Existing nodes in the production Neo4j database were ingested without workspace labels. They carry only their type label (e.g., `:Function`, `:Struct`). New workspace-scoped queries will not find these nodes because MATCH predicates include the workspace label.

## Decision

**Two-phase migration with a global "default" workspace label.**

### Phase 1: Label existing nodes with a synthetic default workspace

Apply `Workspace_000000000000` (12 zero hex chars) as a label to all existing nodes that lack any `Workspace_*` label. This is the "global/default" workspace.

```cypher
// Add default workspace label to all nodes that don't have any Workspace_ label
MATCH (n)
WHERE NOT any(label IN labels(n) WHERE label STARTS WITH 'Workspace_')
SET n:Workspace_000000000000
```

This must be done for each node type label:

```cypher
// Re-run workspace-scoped unique constraints for default workspace
CREATE CONSTRAINT ws_000000000000_crate_fqn_unique IF NOT EXISTS
  FOR (n:Crate:Workspace_000000000000) REQUIRE n.fqn IS UNIQUE;
// ... repeat for all 12 node types
```

**Rationale for `Workspace_000000000000`:**
- The `Workspace_` prefix satisfies the format validation (`^Workspace_[0-9a-f]{12}$`)
- Zero-fill makes it visually distinct from real workspace IDs
- No real workspace will have a nil UUID (Postgres `ws_000000000000` schema is reserved)
- API handlers can treat `Workspace_000000000000` as a "global/fallback" context

### Phase 2: Re-ingest under real workspace labels

When a workspace is provisioned (via the workspace lifecycle API), trigger a re-ingestion of its crates. The ingestion pipeline will create new nodes with the correct `Workspace_<12hex>` label. Due to the workspace-scoped unique constraints, the old `Workspace_000000000000` nodes and new `Workspace_<realhex>` nodes coexist without FQN collisions (different label sets = different constraint scopes).

Once re-ingestion completes for a workspace, remove its nodes from the default workspace:

```cypher
// After verifying workspace A's re-ingestion is complete
MATCH (n:Workspace_000000000000)
WHERE n.fqn STARTS WITH 'workspace_a_crate_root::'
REMOVE n:Workspace_000000000000
// If node has no remaining labels, delete it
WITH n
WHERE size(labels(n)) = 1
DETACH DELETE n
```

### Rollback plan

If migration causes issues:

1. The `Workspace_000000000000` label addition is non-destructive (adds a label, doesn't change properties)
2. Remove the default label: `MATCH (n:Workspace_000000000000) REMOVE n:Workspace_000000000000`
3. Remove workspace-scoped constraints: `DROP CONSTRAINT ws_000000000000_<type>_fqn_unique` for each type
4. Revert the code to allow `workspace_label: None` (already supported — the graph stage skips if label is missing)

### Data integrity checks

Post-migration validation queries:

```cypher
// Verify every node has a Workspace_ label
MATCH (n)
WHERE NOT any(label IN labels(n) WHERE label STARTS WITH 'Workspace_')
RETURN count(n) AS unlabeled_nodes
// Expected: 0

// Verify no duplicate FQNs within a workspace
MATCH (n:Workspace_000000000000)
WITH n.fqn AS fqn, collect(n) AS nodes
WHERE size(nodes) > 1
RETURN fqn, size(nodes) AS dup_count
// Expected: 0 (enforced by constraint)
```

## Execution Plan

### Step 1: Pre-migration audit (5 min)

```cypher
// Count nodes without workspace labels
MATCH (n)
WHERE NOT any(label IN labels(n) WHERE label STARTS WITH 'Workspace_')
RETURN count(n) AS unlabeled, labels(n) AS node_labels
```

### Step 2: Apply default workspace label (5-15 min depending on data size)

```cypher
// Batch approach for large databases
CALL apoc.periodic.iterate(
  'MATCH (n) WHERE NOT any(label IN labels(n) WHERE label STARTS WITH "Workspace_") RETURN n',
  'SET n:Workspace_000000000000',
  {batchSize: 5000}
)
```

If APOC is not available, use the ingestion service's `GraphBuilder::create_workspace_constraints` with `"Workspace_000000000000"` to create constraints, then run the label SET via the API's Cypher endpoint.

### Step 3: Create workspace-scoped constraints (1 min)

Run the constraint creation for `Workspace_000000000000` — the `GraphBuilder::create_workspace_constraints("Workspace_000000000000")` method handles all 12 node types.

### Step 4: Verify (2 min)

Run the data integrity check queries above.

### Step 5: Update `init-neo4j.cypher` (code change)

Add the default workspace label setup to the initialization script so new environments start with the default workspace label and constraints pre-created.

## Constraints

- Migration must be **idempotent** — re-running must be safe
- Migration must not require downtime (Neo4j label additions are online operations)
- The `Workspace_000000000000` label must not collide with any real workspace schema name
- Rollback must be possible without data loss

## Consequences

### Positive

- All existing nodes become queryable through workspace-scoped `WorkspaceGraphClient`
- No data loss — existing nodes keep their properties and relationships
- Phased approach allows gradual migration (re-ingest one workspace at a time)
- Backward compatible — the default workspace acts as a catch-all

### Negative

- Default workspace nodes share a single label, meaning all "global" data is still co-mixed
- Re-ingestion is required for true per-workspace isolation
- The `Workspace_000000000000` namespace is a reserved convention that must be documented in the ops runbook
- Temporary increase in unique constraints (12 per default workspace + 12 per real workspace during migration)

## Related

- [ADR-005: Multi-Tenancy Physical Isolation](ADR-005-multi-tenancy-physical-isolation.md) — Option B+ decision
- [ADR-003: Workspace Isolation Strategy](ADR-003-workspace-isolation.md) — Phase 1 decisions
- [RUSA-195](/RUSA/issues/RUSA-195) — Workspace label injection implementation
- [RUSA-180](/RUSA/issues/RUSA-180) — Parent task for multi-tenant isolation
