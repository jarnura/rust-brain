# rust-brain: Deep Gap Analysis

**Date:** 2026-03-14
**Scope:** Full architectural and implementation review
**Status:** Phase 2 (Partial) — 11,372 LOC across 2 services

---

## Your Analogy — Validated and Extended

Your analogy is **correct and precise**. Let me formalize it:

```
Traditional DB Engine                    rust-brain
─────────────────────                    ──────────
Data on disk (rows, pages)       →       Code AST in Neo4j (graph nodes/edges)
B-tree indexes for fast lookup   →       Qdrant vector indexes for semantic search
SQL query planner                →       API layer as query orchestrator
Disk → Buffer Pool → Query       →       Graph + Vector + Relational → LLM context
Application reads results        →       LLM traverses, finds, reasons over code
```

The three-database architecture maps to three distinct access patterns an LLM needs:

| Access Pattern | Database | Analogy |
|---|---|---|
| **Structural traversal** — "who calls X? what implements Y?" | Neo4j (Graph) | Index scan / join — follow relationships algorithmically |
| **Semantic discovery** — "find code that handles JSON parsing" | Qdrant (Vector) | Full-text search on steroids — find by meaning, not name |
| **Exact retrieval** — "give me the source of function X at line 42" | Postgres (Relational) | Primary key lookup — get the raw data fast |

Where traditional databases give **applications** fast algorithmic access to data on disk, rust-brain gives **LLMs** fast algorithmic access to code semantics in multi-dimensional space. The LLM becomes the "application" consuming from your "database engine."

**One refinement to your analogy:** Traditional DB engines have a query optimizer that chooses the best access path. rust-brain currently lacks this — the LLM must manually choose which tool/endpoint to call. A true "query planner for code intelligence" would be a powerful addition (see Gap #14).

---

## Architecture Summary

```
                    ┌─────────────────────────────┐
                    │         LLM / Agent          │
                    │  (the "application" layer)   │
                    └──────────┬──────────────────┘
                               │ REST API calls
                    ┌──────────▼──────────────────┐
                    │      Tool API (Axum)         │
                    │   9 endpoints, port 8080     │
                    └──┬──────────┬──────────┬────┘
                       │          │          │
              ┌────────▼───┐ ┌───▼────┐ ┌───▼────────┐
              │  Postgres  │ │ Neo4j  │ │   Qdrant   │
              │  (raw data)│ │(graph) │ │ (vectors)  │
              │  sqlx 0.8  │ │neo4rs  │ │ 768-dim    │
              └────────────┘ └────────┘ └────────────┘
                                            ▲
                    ┌───────────────────────┘
                    │ embedding generation
              ┌─────▼──────┐
              │   Ollama   │
              │ nomic-embed│
              │ codellama  │
              └────────────┘
```

**Ingestion Pipeline** (6 stages):
```
Rust Crate → cargo expand → tree-sitter + syn parse → typecheck →
  → extract to Postgres → build Neo4j graph → generate Qdrant embeddings
```

---

## Identified Gaps

### CRITICAL — Project Cannot Function Without These

---

#### Gap 1: Ingestion Service Does Not Compile (BLOCKING)

**Severity:** CRITICAL
**Files:** All of `services/ingestion/src/`

Three known API compatibility issues prevent compilation:

| Issue | Description | Files |
|---|---|---|
| INGEST-001 | `sqlx 0.8` changed `try_get()` API — `Row::try_get` signature differs | `pipeline/stages.rs`, `typecheck/resolver.rs` |
| INGEST-002 | `neo4rs 0.7+` changed `execute()` method visibility — `Graph::execute` may not exist | `graph/*.rs` (all 4 files) |
| INGEST-003 | `uuid` crate `new_v5()` signature changed | `embedding/mod.rs` |

**Impact:** The entire ingestion pipeline is non-functional. No code can be ingested into the system. The API service builds but has nothing to query.

**Fix:** Pin dependency versions in `Cargo.toml` to known-working versions, or update all call sites:
- `sqlx = "0.7"` (or update to 0.8 `Row::try_get::<Type, _>("column")` syntax)
- `neo4rs = "0.6"` (or update to `graph.run(query)` instead of `graph.execute()`)
- `uuid`: use `Uuid::new_v5(&namespace, name.as_bytes())`

---

#### Gap 2: No Unit Tests in Production Code

**Severity:** CRITICAL
**Files:** All source files in `services/ingestion/` and `services/api/`

Neither service has any Rust unit tests (`#[cfg(test)]` modules). The only tests are:
- Shell-based smoke tests (`tests/smoke/test_services.sh`)
- Shell-based integration tests (`tests/integration/`)
- Test fixture crate (`tests/fixtures/test-crate/`) — has tests but is only a sample input

For 11,372 lines of production code, zero unit tests means:
- Parser correctness is unverified (does `syn_parser` actually extract generics correctly?)
- Type resolution is unverified (does the heuristic fallback work?)
- Graph construction is unverified (are relationships created correctly?)
- Embedding text generation is unverified (are code items represented faithfully?)

**Fix:** Add `#[cfg(test)]` modules to at least:
- `parsers/syn_parser.rs` — test each item type parsing
- `parsers/tree_sitter_parser.rs` — test skeleton extraction
- `typecheck/resolver.rs` — test call site extraction, trait impl detection
- `embedding/text_representation.rs` — test text generation for each item type
- `graph/nodes.rs` — test Cypher query generation
- `graph/relationships.rs` — test relationship Cypher generation

---

#### Gap 3: No Workspace Cargo.toml — No Shared Types

**Severity:** HIGH
**Files:** `services/api/Cargo.toml`, `services/ingestion/Cargo.toml`

The two services are completely independent crates with no shared types. This means:

- The API service reimplements types that the ingestion service already defines (e.g., `ParsedItem`, `ItemType`, graph relationship types)
- No shared data contracts — if the ingestion schema changes, the API won't know
- No compile-time guarantee that the API queries match the data the ingestion produces

**Fix:** Create a workspace structure:
```
Cargo.toml (workspace)
├── crates/
│   ├── rustbrain-common/    # Shared types, DB schemas, error types
│   ├── rustbrain-ingestion/ # Ingestion pipeline
│   └── rustbrain-api/       # API service
```

---

### HIGH — Significantly Limits Usefulness

---

#### Gap 4: API Service Has No Neo4j Client

**Severity:** HIGH
**Files:** `services/api/src/main.rs`, `services/api/Cargo.toml`

The API service Cargo.toml does **not** include `neo4rs` as a dependency. The API endpoints for graph traversal (`get_callers`, `get_trait_impls`, `query_graph`) use `reqwest` HTTP calls, but there's no actual Neo4j connection in `AppState`:

```rust
struct AppState {
    config: Config,
    pg_pool: sqlx::postgres::PgPool,
    http_client: reqwest::Client,  // ← used for Qdrant/Ollama HTTP APIs
    metrics: Arc<Metrics>,
    // ← MISSING: neo4j graph client
}
```

The graph-related endpoints likely proxy through HTTP or are stubbed. This means:
- `GET /tools/get_callers` — cannot traverse call graph
- `GET /tools/get_trait_impls` — cannot query trait implementations
- `POST /tools/query_graph` — cannot execute Cypher queries
- `GET /tools/get_module_tree` — cannot traverse module hierarchy

**Fix:** Add `neo4rs` to API Cargo.toml, add `Graph` to `AppState`, implement direct Bolt protocol connections.

---

#### Gap 5: Semantic Search Doesn't Aggregate Across Databases

**Severity:** HIGH
**Files:** `services/api/src/main.rs`

The architecture doc shows the ideal flow:
```
Agent → API → Qdrant (semantic search) → Postgres (details) → Neo4j (relationships) → Agent
```

But the API currently only queries one database per endpoint. There's no cross-database query orchestration where:
1. Semantic search finds candidate FQNs in Qdrant
2. Those FQNs are enriched with source code from Postgres
3. Relationships are fetched from Neo4j
4. All three are aggregated into a rich response

**This is the core value proposition of the triple-storage pattern**, and it's not implemented.

**Fix:** Implement a `search_semantic` handler that:
1. Embeds the query via Ollama
2. Searches Qdrant for top-K FQNs
3. Fetches item details from Postgres (`extracted_items` table)
4. Fetches relationships from Neo4j (callers, trait impls)
5. Returns an aggregated, rich response

---

#### Gap 6: No Incremental Ingestion / Change Detection

**Severity:** HIGH
**Files:** `services/ingestion/src/pipeline/`

The pipeline always does a full re-ingestion. There's no:
- File hash comparison to skip unchanged files
- Incremental graph updates (only update changed nodes/edges)
- Delta embedding (only re-embed changed items)
- Git diff-based change detection

For large codebases (100K+ LOC), full re-ingestion could take 30+ minutes. Without incrementality, the system is impractical for active development.

**Fix:**
- Store file content hashes in `source_files.git_hash`
- Compare before re-processing
- Support `DELETE + INSERT` for changed items instead of full wipe
- Track last-ingested git commit per repository

---

#### Gap 7: Call Site Detection Is Incomplete

**Severity:** HIGH
**Files:** `services/ingestion/src/typecheck/resolver.rs`

The type resolver extracts call sites using `syn` AST walking, but has significant gaps:

1. **Method calls on `self`** — `self.method()` calls within impl blocks may not resolve the callee FQN correctly because the self type isn't always propagated
2. **Trait method dispatch** — `x.process()` where `x: impl Processor` doesn't resolve to the concrete implementation
3. **Closure calls** — Calls through closures/function pointers are not tracked
4. **Macro-generated calls** — Calls inside macro expansions rely on `cargo expand` output, which may not always be available
5. **Turbofish resolution** — `parse::<MyStruct>(input)` may not extract `MyStruct` as the concrete type arg

The heuristic fallback uses regex patterns, which will miss complex expressions.

**Fix:** This is fundamentally hard. Prioritize:
1. Method calls with known self type (most common case)
2. Turbofish explicit type args (easy to extract)
3. Accept imprecision for closure/trait dispatch — mark as `quality: "heuristic"`

---

### MEDIUM — Reduces Quality and Reliability

---

#### Gap 8: No Error Recovery in Pipeline Stages

**Severity:** MEDIUM
**Files:** `services/ingestion/src/pipeline/runner.rs`

The pipeline runner has `fail_fast` mode but limited recovery:
- If the Parse stage fails on one file, the entire stage result is "partial"
- There's no retry logic for transient failures (e.g., Ollama timeout during embedding)
- If the Graph stage fails mid-batch, already-inserted nodes create orphans
- No rollback mechanism across the triple storage (Postgres transaction + Neo4j + Qdrant are not atomic)

**Fix:**
- Add per-file error tracking with continuation
- Add retry with backoff for external service calls (Ollama, Qdrant)
- Implement compensating transactions for partial failures
- Consider a "validation" stage that checks consistency across all three stores

---

#### Gap 9: Embedding Quality Concerns

**Severity:** MEDIUM
**Files:** `services/ingestion/src/embedding/text_representation.rs`

The text representation for embedding is well-structured but has issues:

1. **No code body in embeddings** — Only signatures and doc comments are embedded, not function bodies. An LLM searching "function that sorts using quicksort" won't find a function whose body implements quicksort but whose doc says "sorts the array."

2. **Fixed chunking strategy** — Doc chunks split at 500 chars by paragraph boundary. This is arbitrary and may split semantic units (e.g., an example spanning multiple paragraphs).

3. **No embedding for relationships** — "How do X and Y interact?" requires relationship context in embeddings, which isn't included.

4. **Single embedding model** — `nomic-embed-text` (768-dim) is good for general text but not specifically trained on code. Code-specific models like `voyage-code-2` or fine-tuned models would perform better.

**Fix:**
- Include a truncated body preview in the text representation
- Use sliding window chunking with overlap
- Consider code-specific embedding models
- Add relationship context to embedding text ("calls: X, Y; implements: Trait")

---

#### Gap 10: Postgres and Neo4j Data Consistency

**Severity:** MEDIUM
**Files:** `scripts/init-db.sql`, `services/ingestion/src/graph/`

Data lives in three stores with no cross-store consistency guarantees:

1. **No foreign key between stores** — An FQN in Postgres may not exist in Neo4j (or vice versa) if one stage fails
2. **No deletion cascade** — If a file is removed from the codebase, its items remain in all three stores
3. **Duplicate data** — FQN, name, visibility, signature are stored in both Postgres AND Neo4j node properties
4. **No version tracking** — No way to query "what changed between ingestion run X and Y"

**Fix:**
- Add a consistency check stage at the end of the pipeline
- Implement "garbage collection" for stale items
- Use Postgres as source of truth, Neo4j/Qdrant as derived indexes
- Add an `ingestion_run_id` column to track which run created each item

---

#### Gap 11: API Has No Rate Limiting, Auth, or Input Validation

**Severity:** MEDIUM
**Files:** `services/api/src/main.rs`

The API has:
- No authentication (documented as future scope)
- No rate limiting (expensive semantic search is unbounded)
- Minimal input validation (Cypher injection possible via `query_graph` endpoint)
- No request size limits

The `query_graph` endpoint executes raw Cypher — the code claims it blocks WRITE operations but needs verification that the filtering is robust (e.g., `CALL apoc.` procedures can modify data).

**Fix:**
- Add Cypher query sanitization (whitelist read-only operations)
- Add request body size limits
- Add basic API key auth (even for local use, to prevent accidental exposure)
- Rate limit semantic search (embedding generation is expensive)

---

#### Gap 12: Missing Rust Language Constructs in Parser

**Severity:** MEDIUM
**Files:** `services/ingestion/src/parsers/syn_parser.rs`, `services/ingestion/src/parsers/tree_sitter_parser.rs`

The parsers handle common constructs but miss:

1. **Procedural macros** — `#[derive(MyMacro)]` generates code that isn't captured
2. **Extern blocks** — `extern "C" { fn ... }` is not parsed
3. **Conditional compilation** — `#[cfg(feature = "x")]` items may be missed or duplicated
4. **Async traits** — `async fn` in traits (stable since Rust 1.75) has special desugaring
5. **GATs (Generic Associated Types)** — `type Iter<'a>` in traits
6. **RPITIT** — Return Position Impl Trait In Traits
7. **Const generics beyond simple cases** — `const N: usize` is handled, but complex expressions aren't
8. **Pattern matching in function args** — `fn foo((a, b): (i32, i32))` may not extract parameter types correctly

**Fix:** Prioritize by frequency in real Rust codebases:
1. Conditional compilation (very common in libraries)
2. Async traits (increasingly common)
3. Proc macros (common but hard — requires expansion)
4. Extern blocks (common in FFI crates)

---

#### Gap 13: `cargo expand` Is Fragile

**Severity:** MEDIUM
**Files:** `services/ingestion/src/pipeline/stages.rs` (ExpandStage)

The expand stage calls `cargo expand` as a subprocess:
- Requires `cargo-expand` and nightly Rust installed
- Fails if the target crate doesn't compile
- Produces massive output for macro-heavy crates (serde, tokio)
- No caching of expansion results
- No timeout handling (large crates can take minutes)

**Fix:**
- Make expand stage optional with graceful degradation
- Cache expanded output keyed by file hash
- Add timeout (default 60s per crate)
- Fall back to raw source parsing when expand fails
- Consider selective expansion (only expand specific macros)

---

### DESIGN-LEVEL — Architectural Improvements for the Vision

---

#### Gap 14: No Query Planner / Orchestration Layer

**Severity:** DESIGN
**Impact:** Core to "database engine for code intelligence" vision

Currently, the LLM must know which endpoint to call and in what order. A true "code intelligence engine" would have a query planner:

```
LLM: "How does the error handling work in the auth module?"
     ↓
Query Planner:
  1. Semantic search → find "error handling" + "auth" items
  2. Module tree → get auth module structure
  3. Graph traverse → find error types, Result returns, ? operator usage
  4. Source fetch → get relevant function bodies
  5. Aggregate → structured context for LLM
     ↓
LLM: Receives optimized, complete context
```

This is the "query optimizer" analogy — choosing the best access path across all three databases for a given question.

**Fix:** Add a `/tools/intelligent_query` endpoint that:
- Takes a natural language question
- Decomposes it into sub-queries across the three stores
- Optimizes execution order (parallel where possible)
- Returns aggregated, ranked results

---

#### Gap 15: No Context Window Management

**Severity:** DESIGN
**Impact:** Core to LLM usability

The API returns raw results with no awareness of LLM context window limits. An LLM asking "show me all callers of function X" might get 500 results, which:
- Exceeds context window
- Includes irrelevant items
- Has no relevance ranking

**Fix:**
- Add `token_budget` parameter to all endpoints
- Implement intelligent truncation (most relevant first, then summarize the rest)
- Add "progressive disclosure" — return summary first, details on demand
- Estimate token count per result and respect the budget

---

#### Gap 16: No Cross-Crate / Dependency Analysis

**Severity:** DESIGN
**Impact:** Essential for real-world codebases

The system ingests a single crate at a time. Real codebases have deep dependency trees. When an LLM asks "what calls `serde::Serialize::serialize`?", it needs to see:
- Direct calls in the target crate
- Calls through derive macros
- Calls in workspace members
- Calls through dependency crates

**Fix:**
- Ingest the full dependency graph (at least workspace members)
- Use `cargo metadata` to discover dependencies
- Create cross-crate `:DEPENDS_ON` relationships in Neo4j
- Allow selective depth (direct deps only, transitive, workspace only)

---

#### Gap 17: No Provenance / Confidence Scoring

**Severity:** DESIGN
**Impact:** LLM trustworthiness

When the LLM receives results, it has no way to assess confidence:
- Was this item parsed with `syn` (high confidence) or `tree-sitter` heuristic?
- Is this call site analyzed or heuristic?
- How old is this data? (When was it last ingested?)
- Did the source file have compilation errors?

**Fix:**
- Add `confidence` field to all API responses
- Include `last_indexed_at` timestamp
- Include `resolution_quality` from the type resolver
- Add `parse_source` (syn vs tree-sitter) to item metadata

---

#### Gap 18: No MCP (Model Context Protocol) Integration

**Severity:** DESIGN
**Impact:** Modern LLM integration

The API uses REST endpoints designed as "tools" for LLM function calling. However, the emerging standard for LLM-to-tool communication is MCP. Without MCP:
- Each LLM client must manually configure tool schemas
- No standard discovery mechanism
- No streaming support for large results

**Fix:**
- Implement an MCP server that wraps the existing tool API
- Expose tools with proper JSON Schema descriptions
- Support streaming responses for large result sets
- Register as an MCP resource provider for code context

---

#### Gap 19: No Data Lifecycle / TTL / Garbage Collection

**Severity:** MEDIUM
**Impact:** Operational — data grows unbounded

There's no mechanism to:
- Expire stale data from previous ingestion runs
- Clean up orphaned embeddings after code deletion
- Compact or optimize database storage
- Track storage growth per crate

**Fix:**
- Add `ingestion_run_id` to all stored items
- Implement "mark and sweep" GC after each ingestion
- Add TTL-based expiration for embeddings
- Create a `/admin/gc` endpoint for manual cleanup

---

#### Gap 20: Observability Gaps

**Severity:** MEDIUM
**Files:** API and Ingestion services

While Prometheus/Grafana are set up, the metrics are basic:
- No per-endpoint latency histograms in the API
- No embedding generation latency tracking
- No Neo4j query performance metrics
- No Qdrant search quality metrics (recall, precision)
- No pipeline stage duration breakdown in Grafana dashboards
- No alerting rules configured

**Fix:**
- Add `histogram!` for each API endpoint
- Track embedding batch sizes and durations
- Add Neo4j query time metrics
- Create alerting rules for: stage failures, slow queries, service health

---

## Summary Matrix

| # | Gap | Severity | Effort | Impact |
|---|---|---|---|---|
| 1 | Ingestion doesn't compile | CRITICAL | Low | Blocking |
| 2 | No unit tests | CRITICAL | Medium | Quality |
| 3 | No workspace / shared types | HIGH | Medium | Maintainability |
| 4 | API missing Neo4j client | HIGH | Low | Functionality |
| 5 | No cross-DB aggregation | HIGH | Medium | Core value |
| 6 | No incremental ingestion | HIGH | High | Scalability |
| 7 | Incomplete call site detection | HIGH | High | Accuracy |
| 8 | No error recovery in pipeline | MEDIUM | Medium | Reliability |
| 9 | Embedding quality concerns | MEDIUM | Medium | Search quality |
| 10 | Cross-store consistency | MEDIUM | Medium | Data integrity |
| 11 | No auth / rate limiting | MEDIUM | Low | Security |
| 12 | Missing language constructs | MEDIUM | High | Coverage |
| 13 | `cargo expand` fragility | MEDIUM | Medium | Robustness |
| 14 | No query planner | DESIGN | High | Core vision |
| 15 | No context window mgmt | DESIGN | Medium | LLM usability |
| 16 | No cross-crate analysis | DESIGN | High | Real-world use |
| 17 | No confidence scoring | DESIGN | Low | Trustworthiness |
| 18 | No MCP integration | DESIGN | Medium | Modern LLM compat |
| 19 | No data lifecycle / GC | MEDIUM | Medium | Operations |
| 20 | Observability gaps | MEDIUM | Low | Operations |

---

## Recommended Priority Order

### Phase 1: Make It Work (Week 1)
1. **Fix compilation issues** (Gap 1) — pin dependencies or update call sites
2. **Add Neo4j client to API** (Gap 4) — enable graph endpoints
3. **Implement cross-DB aggregation** (Gap 5) — the core value proposition

### Phase 2: Make It Right (Week 2-3)
4. **Add unit tests** (Gap 2) — parser, type resolver, text representation
5. **Create workspace structure** (Gap 3) — shared types crate
6. **Add confidence scoring** (Gap 17) — low effort, high value
7. **Improve error recovery** (Gap 8)
8. **Fix Cypher injection risk** (Gap 11)

### Phase 3: Make It Fast (Week 3-4)
9. **Incremental ingestion** (Gap 6)
10. **Improve embedding quality** (Gap 9)
11. **Context window management** (Gap 15)
12. **Make cargo expand optional** (Gap 13)

### Phase 4: Make It Complete (Month 2+)
13. **Query planner** (Gap 14) — the "database engine optimizer" for code
14. **Cross-crate analysis** (Gap 16)
15. **MCP integration** (Gap 18)
16. **Call site detection improvements** (Gap 7)
17. **Missing language constructs** (Gap 12)
18. **Data lifecycle / GC** (Gap 19)
19. **Observability improvements** (Gap 20)
20. **Cross-store consistency** (Gap 10)

---

## Conclusion

Your analogy is sound. rust-brain is architecturally well-designed — the triple-storage pattern (Graph + Vector + Relational) is the right approach for giving LLMs "database-like" access to code intelligence. The infrastructure (Docker Compose, monitoring, observability) is production-grade.

The critical gaps are:
1. **The engine doesn't start** (compilation failures)
2. **The engine has no tests** (correctness is unverified)
3. **The core query orchestration is missing** (cross-DB aggregation is the killer feature)

Once these are addressed, the path to "a database engine where LLMs can accurately read code very fast and process it" is clear. The query planner (Gap 14) is the most exciting future piece — it would complete the analogy by adding the "query optimizer" that makes a database engine truly intelligent about access paths.
