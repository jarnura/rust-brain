# ADR-004: Cross-Store Consistency Strategy

## Status

**Accepted** — 2026-04-08

## Context

rust-brain uses three databases (Postgres, Neo4j, Qdrant) with no atomic guarantees across them (see [ADR-001](ADR-001-why-triple-storage.md)). The ingestion pipeline writes to each store sequentially: Postgres during Parse/Extract, Neo4j during Graph, Qdrant during Embed. A failure in any stage after a prior stage has committed leaves the stores inconsistent.

**Current failure modes:**

| Failure Point | Postgres | Neo4j | Qdrant | Impact |
|---------------|----------|-------|--------|--------|
| Parse ✓, Extract ✗ | Items with NULL `source_file_id` | Missing | Missing | Orphaned DB records |
| Extract ✓, Graph ✗ | Complete | Partial/Missing | Missing | Graph queries return incomplete results |
| Graph ✓, Embed ✗ | Complete | Complete | Partial/Missing | Semantic search unavailable for affected items |
| Mid-batch Embed ✗ | Complete | Complete | Partial | Some embeddings missing |

**Existing strengths the design can leverage:**

All three stores already use idempotent write operations:

| Store | Write Strategy | Key | Duplicate Behavior |
|-------|---------------|-----|-------------------|
| Postgres `source_files` | `ON CONFLICT DO UPDATE` | `(crate_name, module_path, file_path)` | Updates metadata |
| Postgres `extracted_items` | `ON CONFLICT DO UPDATE` | `fqn` | Updates signature, body, docs |
| Neo4j nodes | `MERGE` | `{id}` per label | Updates properties |
| Neo4j relationships | `MERGE` | `(from_id, rel_type, to_id)` | Updates properties |
| Qdrant points | HTTP PUT (upsert) | Deterministic UUID v5 from FQN | Overwrites vector + payload |

The pipeline also has checkpoint/resume logic (`pipeline_checkpoints` table) that allows restarting from the last completed stage after a crash. However, it does not verify cross-store consistency on resume.

**Constraints for v1.0:**

- No distributed transaction coordinator (2PC/XA). The operational complexity and latency penalty are not justified for a development tool.
- Re-ingestion must be safe. Idempotent writes already satisfy this.
- Users must be able to detect inconsistencies without manual database queries.

## Decision

We will implement a **two-layer consistency strategy**: idempotent re-ingestion as the recovery mechanism, and a consistency checker as the detection mechanism.

### Layer 1: Idempotent Re-Ingestion (Recovery)

The pipeline's existing idempotent writes make re-ingestion the simplest and safest recovery mechanism. No compensating transactions, no write-ahead log, no saga coordination.

**Changes required:**

1. **Stage-level verification gates.** Before each store-writing stage, verify that upstream data is complete:
   - Before Graph: assert all `extracted_items` have non-NULL `source_file_id`
   - Before Embed: sample-check that corresponding Neo4j nodes exist for items being embedded

2. **Selective stage re-run.** Allow running individual stages (Graph, Embed) without re-running the full pipeline. The checkpoint table already tracks stage completion — extend it to support `--from-stage graph` or `--from-stage embed` CLI flags.

3. **Stale data cleanup on re-ingestion.** When re-ingesting a crate, delete items from all three stores that no longer exist in source before writing new data. This prevents ghost entries from accumulating across re-ingestion runs.

**What this does NOT do:** It does not prevent inconsistency. It makes inconsistency recoverable with a single command (`./scripts/ingest.sh --from-stage graph /path/to/crate`).

### Layer 2: Consistency Checker (Detection)

A CLI subcommand and API endpoint that compares item counts and FQN sets across the three stores, reports discrepancies, and optionally triggers selective re-ingestion.

**Consistency check logic:**

```
For each crate:
  pg_items   = SELECT fqn FROM extracted_items WHERE crate_name = $crate
  neo4j_items = MATCH (n {crate_name: $crate}) RETURN n.fqn
  qdrant_items = scroll points WHERE payload.crate_name = $crate, return payload.fqn

  in_pg_not_neo4j    = pg_items - neo4j_items     → "Graph stage incomplete"
  in_pg_not_qdrant   = pg_items - qdrant_items     → "Embed stage incomplete"
  in_neo4j_not_pg    = neo4j_items - pg_items      → "Orphaned graph nodes"
  in_qdrant_not_pg   = qdrant_items - pg_items     → "Orphaned embeddings"
```

**Output format:**

```
Consistency Report for crate "rustbrain-common":
  Postgres:  142 items
  Neo4j:     142 nodes  ✓
  Qdrant:    140 points  ✗ (2 missing)
  Missing embeddings: rustbrain_common::config::DatabaseConfig, rustbrain_common::config::load_config
  Recommendation: re-run embed stage
```

**Exposure:**

- CLI: `rustbrain consistency-check [--crate <name>] [--fix]`
- API: `GET /api/consistency?crate=<name>` (returns JSON report)
- Health: `GET /health/consistency` (returns aggregate pass/fail for monitoring)

## Options Considered

### Option A: Eventual Consistency with Idempotent Re-Ingestion ✓ (Selected)

Rely on the existing idempotent write patterns. When inconsistency is detected, re-run the failed stage. No coordination overhead. No new infrastructure.

| Pros | Cons |
|------|------|
| Zero new infrastructure | Inconsistency exists until detected and re-run |
| Leverages existing idempotent writes | Requires user or operator intervention |
| Simple mental model: "just re-ingest" | No automatic self-healing |
| Already partially working today | |

### Option B: Saga Pattern with Compensating Transactions ✗

Each stage writes, and if a downstream stage fails, upstream stages execute compensating actions (delete the records they wrote).

| Pros | Cons |
|------|------|
| Automatic cleanup on failure | Complex coordination logic for three async stores |
| Prevents phantom data | Compensating deletes can fail too (double failure) |
| | Adds latency to every pipeline run |
| | Postgres `ON CONFLICT DO UPDATE` makes compensation semantics unclear — what's the "undo" of an upsert? |

**Rejected.** The compensation logic for upserts is ambiguous (you cannot restore the "previous" value without snapshots), and the failure surface doubles because compensations themselves can fail.

### Option C: Write-Ahead Log with Replay ✗

Write all intended mutations to a local WAL file before executing them against any store. On failure, replay the WAL from the point of failure.

| Pros | Cons |
|------|------|
| Durable record of intent | WAL format must serialize Cypher, SQL, and Qdrant REST — three different write protocols |
| Can replay without re-parsing source | WAL storage grows with codebase size |
| Decouples parsing from writing | Adds I/O overhead to every write |
| | Significant implementation effort for v1.0 |

**Rejected.** The engineering cost is disproportionate to the benefit. Since all stores support idempotent writes, re-running the pipeline from source achieves the same result as replaying a WAL, with zero WAL infrastructure.

### Option D: Consistency Checker CLI Tool ✓ (Selected, combined with Option A)

A standalone tool that queries all three stores and reports discrepancies.

| Pros | Cons |
|------|------|
| Non-intrusive; doesn't change the write path | Detection only, not prevention |
| Can run ad-hoc or on schedule | Requires all three stores to be reachable |
| Actionable output (tells you what to re-run) | Point-in-time snapshot; may race with active ingestion |

**Selected as complement to Option A.** Detection (D) + recovery (A) gives a complete consistency story without touching the write path.

## Implementation Plan

### Phase 1: Consistency Checker (services/api)

**Affected crates:** `services/api`

1. Add `GET /api/consistency` handler in `services/api/src/handlers/` that:
   - Queries Postgres: `SELECT crate_name, COUNT(*) FROM extracted_items GROUP BY crate_name`
   - Queries Neo4j: `MATCH (n) WHERE n.crate_name IS NOT NULL RETURN n.crate_name, count(n)`
   - Queries Qdrant: scroll each collection, group by `payload.crate_name`
   - Compares counts and optionally FQN sets (controlled by `?detail=full` parameter)

2. Add `GET /health/consistency` that returns aggregate pass/fail.

3. Add MCP tool `consistency_check` so LLM agents can verify store health.

### Phase 2: Selective Stage Re-Run (services/ingestion)

**Affected crates:** `services/ingestion`

1. Extend `PipelineConfig` with `from_stage: Option<PipelineStage>` to skip completed stages.

2. Modify `PipelineRunner::run()` to skip stages before `from_stage`, loading state from Postgres instead of re-parsing.

3. Add CLI flag `--from-stage <stage>` to `ingest` command.

### Phase 3: Verification Gates (services/ingestion)

**Affected crates:** `services/ingestion`

1. Before Graph stage: query `SELECT count(*) FROM extracted_items WHERE source_file_id IS NULL AND crate_name = $1`. Fail the run if count > 0, with a clear error message pointing to Extract stage failure.

2. Before Embed stage: sample 10 random FQNs from `extracted_items` for the crate, verify they exist as Neo4j nodes. Fail if any are missing.

### Phase 4: Stale Data Cleanup (services/ingestion)

**Affected crates:** `services/ingestion`

1. Before writing new data for a crate, collect the set of FQNs from source.

2. After all stages complete, delete entries from all three stores whose FQN is not in the current source set.

3. Use the existing cascade delete logic (`stages.rs:4783-4902`) as the foundation, extending it from crate-level to item-level granularity.

## Verification

### How to detect inconsistencies

- `GET /api/consistency?crate=<name>` — per-crate detailed report
- `GET /health/consistency` — aggregate health check (integrate with Prometheus/Grafana)
- `rustbrain consistency-check --crate <name>` — CLI for operators

### How to fix inconsistencies

- Re-run the failed stage: `./scripts/ingest.sh --from-stage graph /path/to/crate`
- Full re-ingestion (nuclear option): `./scripts/ingest.sh /path/to/crate`
- Both are safe due to idempotent writes across all three stores.

### How to prevent inconsistencies

- Verification gates (Phase 3) catch upstream failures before writing to downstream stores.
- Monitoring via `/health/consistency` alerts operators before inconsistencies impact users.

## Consequences

### Positive

- **No new infrastructure.** Recovery uses the existing pipeline; detection uses existing database connections.
- **Idempotent writes already work.** The foundation is in place — this ADR formalizes the strategy and adds detection.
- **Operator-friendly.** Consistency reports tell you exactly what's wrong and how to fix it.
- **Incremental.** Phases can ship independently. Phase 1 (detection) is useful even without Phase 2 (selective re-run).

### Negative

- **Inconsistency window.** Between a failure and re-ingestion, the stores disagree. Queries during this window may return incomplete results.
- **No automatic self-healing.** An operator or scheduled job must trigger re-ingestion. (Future work: auto-retry on consistency check failure.)
- **Re-ingestion cost.** For very large crates, re-running Graph or Embed stages may take minutes. Selective stage re-run (Phase 2) mitigates this.

### Mitigations

- **Health check integration.** `/health/consistency` wired to Prometheus alerting means operators are notified within minutes, not hours.
- **Degradation tiers.** The pipeline already skips Embed if Qdrant is unreachable and Graph if Neo4j is down (see `runner.rs:196-204`). Partial data is better than no data.
- **Idempotent safety net.** Even if an operator accidentally re-runs a stage that already succeeded, idempotent writes ensure no data corruption.

## References

- [ADR-001: Triple Storage Architecture](ADR-001-why-triple-storage.md)
- [Architecture: Design Rationale](../architecture.md)
- Pipeline checkpoint logic: `services/ingestion/src/pipeline/resilience.rs:505-699`
- Cascade delete: `services/ingestion/src/pipeline/stages.rs:4783-4902`
- Degradation tiers: `services/ingestion/src/pipeline/runner.rs:196-204`
