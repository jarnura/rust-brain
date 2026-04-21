# ADR-006: Incremental Ingestion

**Status:** Accepted  
**Date:** 2026-04-21  
**Deciders:** CTO  
**Relates to:** Gap 6 (GAP_ANALYSIS.md), RUSA-274

## Context

Full re-ingestion of Tokio (284K items) takes ~95 minutes. For active development workflows, users need sub-minute feedback on changed files. The current pipeline always processes every file through all 6 stages regardless of whether it changed.

The existing schema already stores `content_hash` (SHA-256) on `source_files` and uses FQN-based upsert logic. The stale cleanup mechanism (`cleanup_stale_items`) compares FQN sets to detect deletions. These primitives can be leveraged for incremental operation.

## Decision

Implement file-level incremental ingestion using content hash comparison, with three operating modes:

### Mode 1: Full Ingestion (existing behavior, unchanged)
Process all files through all stages. Used for first-time ingestion or when `--full` flag is passed.

### Mode 2: Incremental Ingestion (new default)
1. **Change Detection Phase** (new, before Expand):
   - Walk the source tree and compute SHA-256 of each file
   - Query `source_files` for existing `(file_path, content_hash)` pairs for this crate
   - Partition files into: `unchanged`, `modified`, `added`, `deleted`
   - Skip `unchanged` files entirely (no expand, parse, typecheck, extract)

2. **Selective Pipeline** (modified stages):
   - Expand/Parse/Typecheck/Extract run only on `modified + added` files
   - Extract stage uses existing FQN-based upsert (ON CONFLICT DO UPDATE)
   - Graph stage: MERGE for modified items (idempotent), DELETE for removed FQNs
   - Embed stage: re-embed only items whose `body_source` or `signature` changed

3. **Deletion Cascade** (enhanced stale cleanup):
   - For `deleted` files: cascade delete from Postgres (ON DELETE CASCADE handles extracted_items)
   - Delete corresponding Neo4j nodes by FQN set
   - Delete corresponding Qdrant points by deterministic UUID v5(FQN)

### Mode 3: Git-Diff Ingestion (future, not in v0.5.0)
Use `git diff --name-only HEAD~N` to identify changed files. Deferred because it requires git history access inside the container.

### Schema Changes

```sql
-- Add to source_files
ALTER TABLE source_files ADD COLUMN IF NOT EXISTS ingestion_run_id UUID;
ALTER TABLE source_files ADD COLUMN IF NOT EXISTS last_ingested_at TIMESTAMPTZ DEFAULT NOW();

-- Add ingestion_runs tracking table
CREATE TABLE IF NOT EXISTS ingestion_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    crate_name TEXT NOT NULL,
    workspace_id TEXT,
    mode TEXT NOT NULL CHECK (mode IN ('full', 'incremental')),
    started_at TIMESTAMPTZ DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    files_total INTEGER DEFAULT 0,
    files_changed INTEGER DEFAULT 0,
    files_skipped INTEGER DEFAULT 0,
    items_upserted INTEGER DEFAULT 0,
    items_deleted INTEGER DEFAULT 0,
    status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed', 'partial'))
);
```

### Qdrant Point ID Determinism

Standardize all Qdrant point IDs to `Uuid::new_v5(&NAMESPACE, fqn.as_bytes())`. The stale cleanup already uses this pattern; make it the canonical ID generation for upserts too. This enables idempotent re-embedding without orphaned vectors.

### Pipeline Runner Changes

```rust
pub struct IncrementalContext {
    pub mode: IngestionMode,
    pub changed_files: HashSet<PathBuf>,
    pub deleted_files: HashSet<PathBuf>,
    pub unchanged_files: HashSet<PathBuf>,
}

pub enum IngestionMode {
    Full,
    Incremental,
}
```

The `PipelineRunner` gains an `IncrementalContext` that each stage can inspect to skip work on unchanged files.

## Consequences

**Positive:**
- 10-100x faster re-ingestion for typical development (1-5 files changed)
- No behavioral change for first-time ingestion
- Leverages existing upsert/merge patterns (low risk of data corruption)
- Deterministic Qdrant IDs eliminate orphaned vectors

**Negative:**
- Cross-file dependencies not tracked: if `mod.rs` re-exports change, downstream items won't update unless their file also changed. Acceptable for v0.5.0; full dependency tracking is a future enhancement.
- Slightly more complex pipeline state management
- `cargo expand` still runs on changed files (can't skip individual files within a crate expansion)

**Mitigations:**
- Provide `--full` flag to force full re-ingestion when incremental results seem stale
- Add a `POST /api/ingestion/validate` endpoint that compares store counts and flags drift
- Document the cross-file limitation clearly

## Alternatives Considered

1. **AST-diff based detection**: Compare parsed ASTs to detect semantic changes vs cosmetic ones. Rejected: too complex for v0.5.0, marginal benefit over content hash.
2. **Event-sourced ingestion log**: Track every item change as an event. Rejected: over-engineered for current scale.
3. **File watcher (inotify)**: Watch filesystem for changes in real-time. Rejected: doesn't work in containerized ingestion model.
