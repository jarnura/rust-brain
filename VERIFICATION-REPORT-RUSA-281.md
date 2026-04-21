# Verification Report: RUSA-281 — 500K+ LOC Stress Test

**Issue**: RUSA-281 Validate 500K+ LOC stress test results across all three stores  
**Date**: 2026-04-21  
**QA Lead**: f7761dd7-9764-4e74-b52d-3540b7c62684  
**Workspace**: synthetic-500k (`1db76434-a542-44a8-846d-6354df8f0095`)  
**Test Input**: 10 synthetic crates, 524,089 LOC  

---

## Executive Summary

| Store | Status | Count | Threshold | Result |
|-------|--------|-------|-----------|--------|
| Postgres | Healthy | 72,599 items | ≥ 10,000 | ✅ PASS |
| Neo4j | Healthy | 87,473 nodes / 72,049 edges | ≥ 5,000 nodes / ≥ 1,000 edges | ✅ PASS |
| Qdrant | Unhealthy | 0 vectors | ≥ 5,000 vectors | ❌ FAIL |

**Overall Verdict**: ❌ **FAIL** — Qdrant vector store has 0 vectors due to a vector dimension mismatch bug.

---

## 1. Postgres Validation

| Metric | Value | Threshold | Result |
|--------|-------|-----------|--------|
| Total items | 72,599 | ≥ 10,000 | ✅ PASS |
| Unique FQNs | 72,599 | = total (0% duplicates) | ✅ PASS |
| Duplicate FQN % | 0.00% | ≤ 1% | ✅ PASS |
| Crate count | 10 | = 10 generated | ✅ PASS |

### Item Type Breakdown

| Type | Count | % |
|------|-------|---|
| function | 33,850 | 46.6% |
| enum | 7,393 | 10.2% |
| struct | 7,376 | 10.2% |
| trait | 7,304 | 10.1% |
| type_alias | 7,184 | 9.9% |
| const | 5,052 | 7.0% |
| static | 2,125 | 2.9% |
| impl | 2,110 | 2.9% |
| use | 155 | 0.2% |
| module | 50 | 0.07% |

---

## 2. Neo4j Validation

| Metric | Value | Threshold | Result |
|--------|-------|-----------|--------|
| Total nodes | 87,473 | ≥ 5,000 | ✅ PASS |
| Total edges | 72,049 | ≥ 1,000 | ✅ PASS |
| Crate nodes | 10 | = 10 generated | ✅ PASS |

### Node Label Breakdown

| Label | Count |
|-------|-------|
| Function | 33,850 |
| Type (generic) | 12,909 |
| Struct | 9,486 |
| Enum | 7,393 |
| Trait | 7,304 |
| TypeAlias | 7,184 |
| Const | 5,052 |
| Static | 2,125 |
| Impl | 2,110 |
| Module | 50 |
| Crate | 10 |

### Edge Type Breakdown

| Type | Count |
|------|-------|
| CONTAINS | 46,531 |
| USES_TYPE | 13,800 |
| HAS_VARIANT | 7,393 |
| FOR | 2,110 |
| HAS_FIELD | 2,110 |
| CALLS | 60 |
| DEPENDS_ON | 45 |

### Cross-Store FQN Spot Check (10 samples)

All 10 sampled FQNs from Postgres were found exactly once in Neo4j. ✅

---

## 3. Qdrant Validation

| Metric | Value | Threshold | Result |
|--------|-------|-----------|--------|
| Code vectors | 0 | ≥ 5,000 | ❌ FAIL |
| Doc vectors | 0 | ≥ 5,000 | ❌ FAIL |

**Root Cause**: Vector dimension mismatch. The ingestion pipeline creates Qdrant collections configured for **2560-dimensional** vectors (matching `EMBEDDING_DIMENSIONS=2560` in the API container), but the Ollama embedding model generates **768-dimensional** vectors. Every batch upsert fails with dimension mismatch errors.

See **Bug #6** below for details.

---

## 4. Cross-Store Consistency

| Ratio | Value | Threshold | Result |
|-------|-------|-----------|--------|
| Neo4j/Postgres | 120.48% | 30–300% | ✅ PASS |
| Qdrant/Postgres | 0.00% | 40–200% | ❌ FAIL |

### Per-Type Consistency (Postgres vs Neo4j)

| Type | Postgres | Neo4j | Match |
|------|----------|-------|-------|
| Function | 33,850 | 33,850 | ✅ Exact |
| Enum | 7,393 | 7,393 | ✅ Exact |
| Struct | 7,376 | 9,486 | ⚠️ Neo4j has 2,110 extra |
| Impl | 2,110 | 2,110 | ✅ Exact |
| Crate | 10 | 10 | ✅ Exact |

**Note**: Neo4j has 9,486 Struct-labeled nodes vs 7,376 struct items in Postgres. The difference (2,110) equals the Impl count, suggesting possible label overlap or additional type derivation in the graph stage. The 12,909 generic `Type` nodes are graph-stage constructs with no direct Postgres counterpart.

---

## 5. Security Contract Verification

| Contract | Test | Response | Result |
|----------|------|----------|--------|
| query_graph rejects writes | `CREATE (n:Test)` | `"Only read-only queries are allowed"` / HTTP 400 | ✅ PASS |
| pg_query rejects mutations | `INSERT INTO extracted_items` | `"Mutating SQL operations are not allowed"` / HTTP 400 | ✅ PASS |

---

## 6. API Endpoint Verification

| Endpoint | Method | Status | Notes |
|----------|--------|--------|-------|
| `/health` | GET | ✅ 200 | Returns `degraded` (Qdrant unhealthy) |
| `/tools/query_graph` | POST | ✅ 200 | Returns correct Cypher results |
| `/tools/pg_query` | POST | ✅ 200 | Returns correct SQL results |
| `/tools/get_module_tree` | GET | ⚠️ 200 | Returns 0 modules (schema mismatch) |
| `/tools/search_docs` | POST | ⚠️ 200 | Returns 0 results (Qdrant empty) |

---

## 7. Bugs Found

### Bug #1: API Binary Stale — `runtime_info` Column Check
- **Severity**: High (blocks API startup)
- **File**: `services/api/src/main.rs` (line ~55)
- **Description**: The committed API binary checks for `executions.runtime_info` column which doesn't exist in the database. The source has been patched to check `volume_name` instead, but the binary was never rebuilt due to 8 other compilation errors (incomplete rate_limiter changes in AppState).
- **Workaround**: Added `runtime_info JSONB` column to `executions` table as a QA operational fix.
- **Reproduction**: Start the API binary without the `runtime_info` column → crash with `SCHEMA VALIDATION FAILED`.

### Bug #2: Neo4j Double-Label Constraint Syntax
- **Severity**: High (blocks graph stage for workspaces)
- **File**: `services/ingestion/src/pipeline/stages.rs`
- **Description**: When `--workspace-label` is provided, the graph stage generates `CREATE CONSTRAINT ... FOR (n:Crate:Workspace_1db76434a542) REQUIRE n.fqn IS UNIQUE`. The double-label syntax `Label1:Label2` inside `FOR (n:...)` is invalid in Neo4j 5.x. This causes all constraint creation to fail.
- **Impact**: Graph ingestion previously failed entirely. Current run succeeded because constraint errors were non-fatal and node insertion still worked.
- **Reproduction**: Run ingestion with `--workspace-label Workspace_xxx` against Neo4j 5.x → constraint syntax error.

### Bug #3: Schema Name Derivation Mismatch
- **Severity**: Medium (causes embed stage to find 0 items)
- **File**: `services/ingestion/src/pipeline/mod.rs` (search_path logic)
- **Description**: The ingestion binary derives the Postgres schema name from the workspace ID (first 12 hex chars → `ws_1db76434a542`), but the actual workspace schema is assigned independently by the workspace creation API (`ws_24ca3a397b28`). This causes `SET search_path` to point to a non-existent schema, falling back to `public`.
- **Impact**: Embed stage may find 0 items if it queries the wrong schema. In this test, data was in `public` due to fallback, so extract worked but the embed stage's schema-specific queries may fail.
- **Reproduction**: Create workspace → note schema name → run ingestion → observe `SET search_path` uses wrong schema.

### Bug #4: Qdrant URL Defaults to Docker Hostname
- **Severity**: Medium (blocks embed stage outside Docker)
- **File**: `services/ingestion/src/pipeline/stages.rs` or embedding config
- **Description**: When running the ingestion binary outside Docker, `QDRANT_HOST` defaults to `http://qdrant:6333` which is unreachable from the host. Must set `QDRANT_HOST=http://localhost:6333` env var explicitly.
- **Reproduction**: Run `rustbrain-ingestion` without `QDRANT_HOST` env var from host → connection refused.

### Bug #5: NEO4J_PASSWORD Required but No CLI Flag
- **Severity**: Low (works with env var)
- **File**: `services/ingestion/src/pipeline/stages.rs`
- **Description**: The Neo4j password is required for authentication but there's no `--neo4j-password` CLI flag. Must be passed via `NEO4J_PASSWORD` environment variable, which is not documented in `--help`.
- **Reproduction**: Run ingestion without `NEO4J_PASSWORD` env var → Neo4j auth failure / panic.

### Bug #6: Vector Dimension Mismatch (Qdrant 2560 vs Ollama 768)
- **Severity**: Critical (completely blocks Qdrant population)
- **File**: `services/ingestion/src/embedding/mod.rs` (collection creation)
- **Description**: The API container creates Qdrant collections with `EMBEDDING_DIMENSIONS=2560`, but the Ollama model (`qwen3-embedding:4b`) generates 768-dimensional vectors. The ingestion binary creates collections matching the API's configured dimension (2560), but all upserts fail because the generated embeddings are 768-dim.
- **Evidence**: Qdrant collection config shows `size: 2560`, but Ollama generates `768 dimensions each`. All 45+ embed batches failed with `Failed to upsert batch embeddings to Qdrant`.
- **Reproduction**: Run ingestion with embed stage → every Qdrant upsert fails → 0 vectors stored.

### Bug #7: Pipeline Checkpoints Prevent Re-runs After Data Loss
- **Severity**: Medium (causes confusion during retries)
- **File**: `services/ingestion/src/pipeline/resilience.rs`
- **Description**: Pipeline checkpoints record that a stage completed, causing subsequent runs to skip it even if the data was cleared. Must manually delete from `pipeline_checkpoints` table before retrying.
- **Reproduction**: Run ingestion → clear data → re-run → stages skipped due to checkpoints.

---

## 8. Ingestion Performance

| Stage | Duration | Items Processed | Notes |
|-------|----------|-----------------|-------|
| Expand | 166ms | 10 crates | Cached from previous run |
| Parse/Extract | ~16s | 72,599 items | Large files (>2MB) parsed from originals |
| Graph | ~60s | 87,473 nodes, 72,049 edges | Constraint errors non-fatal |
| Embed | ~6min (killed) | 0 vectors stored | All upserts failed (Bug #6) |

**Total ingestion time** (extract + graph): ~76 seconds for 524K LOC / 72K items.

---

## 9. Test Environment

| Component | Version | Status |
|-----------|---------|--------|
| Docker Engine | 27.x | Running |
| PostgreSQL | 16 | Healthy |
| Neo4j | 5.26.22 | Healthy |
| Qdrant | Latest | Healthy (but unused due to Bug #6) |
| Ollama | Latest | Healthy (768-dim embeddings) |
| API | 0.1.0 | Degraded (Qdrant unhealthy) |
| Ingestion Binary | debug build | Functional (with workarounds) |

---

## 10. Recommendations

1. **Fix Bug #6 first** — align `EMBEDDING_DIMENSIONS` with the actual Ollama model output (768), or use an embedding model that produces 2560-dim vectors. This is the critical blocker.
2. **Fix Bug #3** — the schema name derivation must use the actual workspace schema from the database, not derive it from the workspace ID.
3. **Fix Bug #2** — use single-label constraint syntax compatible with Neo4j 5.x.
4. **Fix Bug #1** — rebuild the API binary after resolving the 8 compilation errors in the rate_limiter integration.
5. **Add CLI flags** for `--neo4j-password` and `--qdrant-host` (Bugs #4, #5).
6. **Add checkpoint reset** mechanism or auto-detect data loss (Bug #7).

---

*Report generated by QA Lead agent f7761dd7-9764-4e74-b52d-3540b7c62684 for RUSA-281.*
