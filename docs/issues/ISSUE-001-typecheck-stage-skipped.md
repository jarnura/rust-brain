# ISSUE-001: TypecheckStage Always Skipped Due to Premature State Clear

| Field | Value |
|-------|-------|
| **ID** | ISSUE-001 |
| **Status** | **Closed** (fixed in commit `316cb7d`, 2026-03-29) |
| **Severity** | High |
| **Priority** | P1 |
| **Created** | 2026-03-29 |
| **Resolved** | 2026-03-29 |
| **Component** | Ingestion Pipeline |
| **Affects** | Lazy Monomorphization, Call Graph, Trait Resolution |

---

## Summary

The `TypecheckStage` in the ingestion pipeline is always skipped because `ParseStage` clears `state.expanded_sources` before `TypecheckStage` runs. This results in the `call_sites` and `trait_implementations` database tables remaining empty, breaking the lazy monomorphization tracking feature.

---

## Impact

### Broken Features

| Feature | Description | Status |
|---------|-------------|--------|
| **Lazy Monomorphization** | Tracks monomorphized call sites with turbofish syntax | ❌ Broken |
| **Call Graph Resolution** | Resolves generic types in function calls | ❌ Broken |
| **Trait Implementation Quality** | Analyzes and scores trait implementations | ❌ Broken |

### Database Tables Affected

| Table | Expected | Actual | Purpose |
|-------|----------|--------|---------|
| `call_sites` | Populated | **Empty (0 rows)** | Stores monomorphized call sites |
| `trait_implementations` | Populated | **Empty (0 rows)** | Stores trait impl metadata with quality scores |

### Working Features (Unaffected)

| Feature | Status |
|---------|--------|
| `extracted_items` (Postgres) | ✅ Working |
| `code_embeddings` (Qdrant) | ✅ Working |
| `graph_nodes` (Neo4j) | ✅ Working |
| Semantic search | ✅ Working |

---

## Root Cause Analysis

### Pipeline Stage Execution Order

```
┌────────────────┐    ┌────────────────┐    ┌────────────────┐
│  ExpandStage   │───▶│   ParseStage   │───▶│ TypecheckStage │
│                │    │                │    │                │
│ Populates:     │    │ Clears:        │    │ Checks:        │
│ expanded_sources│    │ expanded_sources│   │ is_empty() ────┼──▶ SKIPS!
│ source_files   │    │ source_files   │    │                │
└────────────────┘    └────────────────┘    └────────────────┘
```

### Bug Location in Code

#### File: `services/ingestion/src/pipeline/stages.rs`

---

### Step 1: ExpandStage Populates State (lines 1443-1448)

```rust
// Update state with cache file paths and source files
{
    let mut state = ctx.state.write().await;
    state.expanded_sources = Arc::new(expanded_map);  // ✅ HashMap populated
    state.source_files = all_source_files;
    state.counts.files_expanded = expanded_count;
}
```

**Result**: `state.expanded_sources` contains `{source_path -> cache_path}` mapping.

---

### Step 2: ParseStage Clears State (lines 1516-1531)

```rust
let source_files = {
    let state = ctx.state.read().await;
    let source_files = state.source_files.clone();
    let expanded_cache_paths = state.expanded_sources.clone();  // Arc clone (local copy)
    drop(state);
    
    // Clear state to free memory (expanded sources are now on disk)
    let mut state = ctx.state.write().await;
    state.expanded_sources = Arc::new(HashMap::new());  // ❌ BUG: CLEARED!
    state.source_files.clear();                          // ❌ BUG: CLEARED!
    drop(state);
    
    trim_memory();
    
    (source_files, expanded_cache_paths)  // ⚠️ Used only INTERNALLY by ParseStage
};
```

**Problems**:

1. **Line 1524**: `expanded_sources` is replaced with empty HashMap
2. **Line 1525**: `source_files` is cleared
3. The cloned `expanded_cache_paths` is used only within ParseStage's local scope
4. State is never restored after ParseStage completes

---

### Step 3: TypecheckStage Finds Empty State (lines 1905-1914)

```rust
async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
    let start = Instant::now();
    
    info!("Starting typecheck stage");
    
    if ctx.config.dry_run {
        info!("Dry run - skipping typecheck");
        return Ok(StageResult::skipped("typecheck"));
    }
    
    let state = ctx.state.read().await;
    let source_files = state.source_files.clone();           // Empty Vec!
    let expanded_cache_paths = state.expanded_sources.clone(); // Empty HashMap!
    let parsed_items = state.parsed_items.clone();
    drop(state);
    
    if expanded_cache_paths.is_empty() {
        info!("No expanded sources to typecheck");
        return Ok(StageResult::skipped("typecheck"));  // ❌ ALWAYS TRUE!
    }
    // ... rest of typecheck logic never runs
}
```

**Result**: TypecheckStage always returns `StageResult::skipped("typecheck")`.

---

## The Misconception

The comment on line 1522-1523 reveals the original intent:

```rust
// Clear state to free memory (expanded sources are now on disk)
```

### What the Developer Thought

> "The expanded source code is large. We've written it to disk cache files. We should clear it from memory to prevent OOM."

### The Reality

The `expanded_sources` field is NOT the expanded source content:

```rust
// From pipeline/mod.rs:135
pub expanded_sources: Arc<HashMap<PathBuf, PathBuf>>,
//                              ↑            ↑
//                              │            └── Cache file path (disk location)
//                              └── Source file path (key)
```

**Memory Footprint Analysis**:

| Data | Size per Entry | For 10,000 files |
|------|----------------|------------------|
| Source path (PathBuf) | ~64 bytes | ~640 KB |
| Cache path (PathBuf) | ~64 bytes | ~640 KB |
| HashMap overhead | ~32 bytes | ~320 KB |
| **Total** | ~160 bytes | **~1.6 MB** |

**Conclusion**: The entire HashMap is ~1-2 MB for a large codebase. Clearing it saves negligible memory but breaks a critical feature.

---

## State Structure Reference

From `services/ingestion/src/pipeline/mod.rs`:

```rust
/// Mutable state accumulated during pipeline execution
#[derive(Debug, Default)]
pub struct PipelineState {
    /// Source files discovered
    pub source_files: Vec<SourceFileInfo>,

    /// Cache file paths for expanded source code by source file path.
    /// Content is read on-demand from cache files to prevent OOM.
    pub expanded_sources: Arc<HashMap<PathBuf, PathBuf>>,

    /// Parsed items by file
    pub parsed_items: HashMap<PathBuf, Vec<ParsedItemInfo>>,

    /// Extracted item IDs by FQN
    pub extracted_items: HashMap<String, Uuid>,

    /// Graph node IDs by FQN
    pub graph_nodes: HashMap<String, String>,

    /// Errors encountered
    pub errors: Vec<StageError>,

    /// Counts for each stage
    pub counts: StageCounts,

    /// Cache of expand results keyed by content hash (for incremental runs)
    pub expand_cache: HashMap<String, String>,

    /// Cross-store references for consistency tracking
    pub store_references: HashMap<String, rustbrain_common::StoreReference>,
}
```

---

## Evidence: Database Query Results

```sql
-- Verified on 2026-03-29
SELECT COUNT(*) as extracted_items_count FROM extracted_items;
-- Result: 161,258 ✅

SELECT COUNT(*) as call_sites_count FROM call_sites;
-- Result: 0 ❌

SELECT COUNT(*) as trait_implementations_count FROM trait_implementations;
-- Result: 0 ❌

SELECT COUNT(*) as with_generics FROM extracted_items WHERE generic_params != '[]';
-- Result: 14,089 (generics ARE being captured)
```

---

## Proposed Solutions

### Option A: Remove the Clear (Recommended)

**Change**: Remove line 1524 entirely.

```diff
  // Clear state to free memory (expanded sources are now on disk)
  let mut state = ctx.state.write().await;
- state.expanded_sources = Arc::new(HashMap::new());
  state.source_files.clear();
  drop(state);
```

**Pros**:
- Simple one-line fix
- Minimal memory impact (HashMap is tiny)
- Preserves all functionality

**Cons**:
- None significant

---

### Option B: Restore State After Parse

**Change**: Restore `expanded_sources` after ParseStage completes.

```rust
// In ParseStage::run(), after parsing completes:

// Restore expanded_sources for TypecheckStage
{
    let mut state = ctx.state.write().await;
    state.expanded_sources = expanded_cache_paths;  // Restore the Arc
}
```

**Pros**:
- Preserves original memory management intent during parsing
- Still allows TypecheckStage to function

**Cons**:
- More complex
- Unnecessary since the memory savings are negligible

---

### Option C: Pass Data Directly Between Stages

**Change**: Modify pipeline to pass `expanded_cache_paths` directly to TypecheckStage instead of using shared state.

**Pros**:
- More explicit data flow
- No state management issues

**Cons**:
- Requires refactoring pipeline architecture
- More invasive change

---

## Recommended Fix

**Option A** is recommended:

1. Remove line 1524 in `stages.rs`
2. Optionally keep `source_files.clear()` if memory is a concern (TypecheckStage clones it before the clear anyway)

The memory savings from clearing the HashMap are negligible (~1-2 MB), but the functionality loss is critical.

---

## Implementation Checklist

- [ ] Remove or comment out line 1524 in `services/ingestion/src/pipeline/stages.rs`
- [ ] Add regression test for TypecheckStage execution
- [ ] Re-run ingestion on test codebase
- [ ] Verify `call_sites` table is populated
- [ ] Verify `trait_implementations` table is populated
- [ ] Update comments to clarify memory management

---

## Verification Steps

After applying the fix:

### 1. Run Ingestion

```bash
cd /home/jarnura/projects/rust-brain
cargo run --bin rustbrain-ingestion -- -c ./test-crate -d postgres://rustbrain:rustbrain_dev_2024@localhost:5432/rustbrain
```

### 2. Check Database Tables

```sql
-- Should have data now
SELECT COUNT(*) FROM call_sites;
SELECT COUNT(*) FROM trait_implementations;

-- Sample data
SELECT * FROM call_sites LIMIT 5;
SELECT * FROM trait_implementations LIMIT 5;
```

### 3. Verify Logs

Look for these log messages:

```
# Before fix (always appears):
INFO typecheck stage: No expanded sources to typecheck

# After fix (should appear):
INFO Typecheck stage completed: X files analyzed, Y trait impls, Z call sites
```

---

## Related Files

| File | Lines | Description |
|------|-------|-------------|
| `services/ingestion/src/pipeline/stages.rs` | 1516-1531 | ParseStage state clear (bug location) |
| `services/ingestion/src/pipeline/stages.rs` | 1905-1914 | TypecheckStage skip check |
| `services/ingestion/src/pipeline/stages.rs` | 1443-1448 | ExpandStage state population |
| `services/ingestion/src/pipeline/runner.rs` | 49-56 | Pipeline stage order |
| `services/ingestion/src/pipeline/mod.rs` | 128-157 | PipelineState definition |
| `services/ingestion/src/typecheck/mod.rs` | - | TypeResolutionService |
| `services/ingestion/src/typecheck/resolver.rs` | - | Type resolver with turbofish detection |

---

## Additional Context

### What TypecheckStage Does (When Working)

1. **Call Site Extraction**: Finds turbofish syntax like `function::<Type>()` and records:
   - Caller function FQN
   - Callee function FQN
   - Monomorphized type arguments
   - Location (file, line)

2. **Trait Implementation Analysis**: Analyzes `impl Trait for Type` blocks and records:
   - Trait FQN
   - Self type
   - Impl block FQN
   - Generic parameters
   - Quality score (completeness of implementation)

### Why This Matters for Lazy Monomorphization

The lazy monomorphization strategy is:

1. **Store generics as-is** in `extracted_items` (working)
2. **Index call sites** with concrete types in `call_sites` (broken)
3. **Resolve on query** using call site index (broken)

Without `call_sites` data, the system cannot:
- Find all call sites of `foo::<String>`
- Track which concrete types are used with generic functions
- Provide accurate "who calls this with what types" queries

---

## History

| Date | Event |
|------|-------|
| 2026-03-29 | Bug discovered during investigation of empty `call_sites` table |
| 2026-03-29 | Issue documented |
