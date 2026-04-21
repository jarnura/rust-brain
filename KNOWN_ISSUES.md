# rust-brain Known Issues and Limitations

**Last Updated:** 2026-04-21  
**Status:** Living document — updated as issues are resolved or discovered

---

## Summary

rust-brain is a production-grade Rust code intelligence platform, but like any complex system, it has known limitations. This document provides an honest assessment of what does not work yet, edge cases, and failure modes.

**Philosophy:** We document limitations transparently so users can make informed decisions and work around issues appropriately.

---

## Issue Taxonomy

| Icon | Meaning |
|------|---------|
| 🔴 | Critical — prevents core functionality |
| 🟡 | High — significantly impacts usefulness |
| 🟢 | Medium — reduces quality or reliability |
| 🔵 | Design — architectural improvement needed |
| ✅ | Resolved in recent release |

---

## Ingestion Pipeline

### ✅ ISSUE-001 [RESOLVED]: TypecheckStage Skipped

**Status:** Fixed in commit `316cb7d` (2026-03-29)

**Original Problem:** The TypecheckStage was always skipped because ParseStage cleared `state.expanded_sources` before TypecheckStage ran.

**Current State:**
- `call_sites` table now populated (99,654+ rows)
- `trait_implementations` table now populated (29,738+ rows)
- Snapshot v2 includes full call graph data (227K CALLS edges)

**Files:** See `docs/issues/ISSUE-001-typecheck-stage-skipped.md`

---

### 🟡 ISSUE-002 [OPEN]: v1/v2 Feature Mixing on Fallback

**Status:** Open — P2 priority

**Problem:** When ingesting Hyperswitch with v1 features, some crates fail expansion due to feature conflicts. The pipeline's fallback logic retries with v2 features, causing v2-only types to appear in a knowledge base intended to be v1-only.

**Affected Crates:**
| Crate | Expanded With | Result |
|-------|---------------|--------|
| `api_models` | v1 + olap + frm | v1 types |
| `router` | v1 → conflict → v2 fallback | v2 types in KB |
| `common_enums` | No feature gate | Both v1 and v2 types |

**Workaround:** None currently. The KB works but has extra v2 items.

**Proposed Fix:** Add `FEATURE_STRATEGY` environment variable

**Files:** `docs/issues/ISSUE-002-v1-v2-mixed-ingestion.md`

---

### 🟡 ISSUE-003 [OPEN]: Trait Impl Method Call Resolution Gap

**Status:** Open — P2 priority

**Problem:** When a function calls a trait method, GraphStage resolves the CALLS edge to the first matching function name rather than the specific trait impl method.

**Impact:**
- `get_callers` for trait impl methods returns empty or incomplete results
- Thousands of pub impl methods show zero callers (many should have callers)

**Workaround:** Explorer agent has a grep fallback

**Files:** `docs/issues/ISSUE-003-trait-impl-call-resolution.md`

---

### 🟡 No Incremental Ingestion

**Severity:** High

**Problem:** The pipeline always does a full re-ingestion. No file hash comparison, incremental graph updates, delta embedding, or git diff-based change detection.

**Impact:** Large codebases (100K+ LOC) can take 30+ minutes to re-ingest.

**Mitigation:** Use snapshots to distribute pre-ingested data.

---

### 🟢 Memory and Size Limitations

**File:** `services/ingestion/src/pipeline/memory_accountant.rs` and `stages.rs`

| Limit | Value | Behavior |
|-------|-------|----------|
| Pre-flight file skip | >10 MB | File skipped before expansion |
| Post-expansion skip | >2 MB | File skipped after expansion |
| Max impl blocks | 500 | Files with >500 impl blocks skipped |
| MAX_BODY_SOURCE_LEN | 50,000 bytes | Body source truncated |

---

### 🟢 Incomplete Call Site Detection

**Severity:** Medium

**Limitations in call site extraction:**
1. Method calls on `self` may not resolve callee FQN correctly
2. Trait method dispatch does not resolve to concrete implementation
3. Closure calls through function pointers are not tracked
4. Macro-generated calls rely on cargo expand output
5. Turbofish resolution may not extract concrete type args

---

### 🟢 cargo expand Fragility

**Severity:** Medium

**Issues:**
- Requires cargo-expand and nightly Rust installed
- Fails if target crate does not compile
- Produces massive output for macro-heavy crates
- No caching of expansion results
- Timeout: 3 minutes per crate (hardcoded)

---

### 🟢 Doc Comment Loss in Dual Parsing

**File:** `services/ingestion/src/parsers/mod.rs` (lines 422-437)

**Problem:** Doc comments preceding items are outside tree-sitter's skeleton byte range and are lost when syn parses just the extracted item source.

**Impact:** Documentation extraction incomplete for items with leading doc comments.

---

### 🟢 Feature Propagation is Hyperswitch-Specific

**Severity:** Medium

**Problem:** ~200 lines of hardcoded Hyperswitch Cargo.toml patching logic adds olap/frm features to transitive dependencies. Not configurable for other projects.

---

## Cross-Store Consistency

### 🔴 No Atomic Transactions Across Stores

**Severity:** High (Critical for data integrity)

**Problem:** Postgres, Neo4j, and Qdrant operations are not wrapped in a distributed transaction.

**Impact:** If a stage fails mid-pipeline:
- Items may exist in Postgres but not Neo4j
- Embeddings may exist in Qdrant without graph nodes
- Partial failure creates orphan records

**Mitigation:** Basic error recovery exists, but no rollback mechanism across stores.

---

### 🟢 No Cross-Store Referential Integrity

**Issues:**
- No foreign key constraints between stores
- An FQN in Postgres may not exist in Neo4j if a stage failed
- Deleted files leave stale items in all three stores
- Duplicate data stored in both Postgres AND Neo4j

---

### 🟢 No Data Lifecycle / Garbage Collection

**Missing:**
- Expiration of stale data from previous ingestion runs
- Cleanup of orphaned embeddings after code deletion
- Automatic database storage optimization

---

## Graph / Neo4j Limitations

### 🟡 CALLS Edges May Point to Placeholder Nodes

**Severity:** High

**Problem:** When GraphStage creates CALLS edges, it may reference functions that haven't been processed yet, creating placeholder nodes.

**Impact:** Caller queries may return partial information.

---

### 🟢 Known Gaps Database (GAP-001 to GAP-008)

**File:** `services/api/src/gaps.rs`

| ID | Severity | Issue | Workaround |
|----|----------|-------|------------|
| GAP-001 | Medium | USES_TYPE relationships not generated | Use semantic search |
| GAP-002 | Low | Module hierarchy incomplete | Manual inspection |
| GAP-003 | Medium | Generic type parameters not tracked | None |
| GAP-004 | Low | Doc comments may be truncated | None |
| GAP-005 | High | Macro-expanded code not indexed | None |
| GAP-006 | Medium | Cross-crate calls not fully resolved | None |
| GAP-007 | Low | Associated types not tracked | None |
| GAP-008 | Medium | Semantic search limited to functions | Use graph queries |

---

### 🟢 No Cross-Crate Dependency Analysis

**Problem:** The system ingests one crate at a time. Cross-crate call relationships are not tracked.

**Impact:** Queries only see direct calls in the target crate.

---

## API / MCP Limitations

### 🔴 No Authentication or Authorization

**Severity:** High (Security)

**Problem:** The API operates without authentication for local development.

**Missing:**
- No API key validation
- ~~No rate limiting~~ Rate limiting added in v0.4.0 (basic)
- ~~No request size limits~~ Request limits added in v0.4.0 (basic)
- Minimal input validation

**Progress in v0.4.0:** Basic rate limiting and request limits added. Cypher injection hardened with template registry. Full authentication still pending.

**Recommendation:** Add reverse proxy with auth for production deployments.

---

### 🟢 Panic Risk on Malformed Input

**Files:** Multiple API handlers and MCP tools

**Problem:** Many handlers use `.unwrap()` on serde deserialization of user-controlled input, which can panic on malformed JSON.

**Affected areas:**
- `services/api/src/handlers/tasks.rs`
- `services/api/src/handlers/execution.rs`
- `services/api/src/handlers/search.rs`
- All MCP tool files

---

### 🟢 Cypher Injection Risk

**Severity:** Medium

**Problem:** The `POST /tools/query_graph` endpoint accepts raw Cypher queries. Pattern-based filtering blocks WRITE operations but sophisticated injection may bypass it.

**Current filter:** Blocks CREATE, MERGE, DELETE, SET, REMOVE, DROP

**Progress in v0.4.0:** Cypher template registry added (RUSA-196) for safe predefined queries. `apoc.graph.fromCypher` bypass closed (RUSA-198). Comment handling hardened. Raw Cypher endpoint still available — use template registry for production workloads.

---

### 🟢 No Query Planner / Orchestration Layer

**Severity:** Medium

**Problem:** The LLM must manually choose which endpoint to call. No intelligent query planner optimizes access paths across the three stores.

---

### 🟢 Limited Context Window Management

**Severity:** Medium

**Problem:** API returns raw results with no awareness of LLM context window limits.

**Missing:**
- Token budget parameter
- Intelligent truncation
- Progressive disclosure

---

### 🟢 aggregate_search Partial Implementation

**Severity:** Medium

**Status:** Cross-database aggregated search endpoint exists but full orchestration needs validation.

---

## Performance Constraints

### 🔴 Embedding Generation Bottleneck

**Severity:** High

**Current model:** qwen3-embedding:4b (2560 dimensions)

**Requirements:**
- 16GB+ RAM recommended
- GPU highly recommended
- macOS runs on CPU only (slow)

**Throughput:** ~50-100 items/minute on CPU, ~500+/minute on GPU

---

### 🟢 Timeout Configurations

| Component | Default | Configurable |
|-----------|---------|--------------|
| HTTP Client | 600s | Yes |
| MCP HTTP | 30s | Yes |
| Execution | 7200s | Yes |
| Container Ready | 60s | Yes |
| Ollama | 60s | Yes |
| Qdrant | 30s | Yes |
| Cargo Expand | 180s | No |

---

### 🟢 No Multi-Level Caching

**Severity:** Medium

**Missing:** L1 in-memory LRU, L2 Redis cache, L3 pre-computed views.

---

## Data Quality Issues

### 🟢 Embedding Text Representation Gaps

**Current limitations:**
1. No code body in embeddings — only signatures and doc comments
2. Fixed chunking strategy splits at 500 chars
3. No relationship context in embeddings

---

### 🟢 Missing Rust Language Constructs

**Parsers don't fully handle:**
1. Procedural macros
2. Extern blocks
3. Conditional compilation (may miss or duplicate items)
4. Async traits
5. Generic Associated Types (GATs)
6. RPITIT
7. Complex const generic expressions

---

## Platform / Deployment

### 🟢 macOS GPU Not Supported

**Problem:** Docker Desktop on macOS doesn't support NVIDIA GPU passthrough. Ollama runs on CPU only.

**Workaround:** Use the snapshot system to avoid running ingestion on macOS.

---

## Test Coverage Gaps

### 🟡 Untested Critical Modules

| Module | Lines | Risk |
|--------|-------|------|
| `services/api/src/main.rs` | 1,140 | API could silently fail |
| `typecheck/resolver.rs` | 968 | Core type resolution wrong |
| `parsers/mod.rs` | 410 | DualParser fallback untested |
| `pipeline/runner.rs` | 399 | Error propagation untested |
| `embedding/ollama_client.rs` | 347 | Network handling untested |
| `embedding/qdrant_client.rs` | 556 | Vector operations untested |

---

### 🟢 Ignored Integration Tests

- `services/mcp/tests/mcp_integration.rs` — 21 ignored tests
- `services/api/tests/consistency_integration.rs` — 28 ignored tests
- `services/api/tests/api_integration.rs` — 40+ ignored tests

---

## Resolution Timeline

| Issue | Target Release | Notes |
|-------|---------------|-------|
| ISSUE-002 (v1/v2 feature mixing) | v0.5.0 | Deferred — feature propagation pre-patching added as partial mitigation |
| ISSUE-003 (trait impl calls) | v0.5.0 | Deferred |
| Incremental ingestion | v0.5.0 | Deferred from v0.4.0 |
| Query planner | v0.6.0 | |
| Auth/rate limiting | v0.5.0 | Deferred from v0.4.0 — Cypher template registry added as security hardening |
| Confidence scoring | v0.5.0 | Deferred from v0.3.0 |
| Multi-tenancy isolation | **v0.4.0** ✅ | Delivered — per-workspace Postgres schema, Neo4j labels, Qdrant collections |
| Cross-workspace leak detection | **v0.4.0** ✅ | Delivered — rustbrain-audit service with Prometheus alerting |
| SSE reconnection with backfill | **v0.4.0** ✅ | Delivered — cursor-based reconnection for MCP and chat streams |

---

## Reporting New Issues

Found a new limitation?

1. Create a file in `docs/issues/ISSUE-XXX-short-description.md`
2. Use the template from existing issue files
3. Include: reproduction steps, severity, affected files, proposed fix

---

## Related Documents

- [GAP_ANALYSIS.md](./docs/GAP_ANALYSIS.md) — Detailed architectural gap analysis
- [PROJECT_STATE.md](./PROJECT_STATE.md) — Current project status

---

*This document is co-maintained by the Community Manager and Engineering teams.*

*Co-Authored-By: Paperclip <noreply@paperclip.ing>*
