# Coverage Report — rust-brain

**Generated:** 2026-04-08  
**Tool:** `cargo llvm-cov --workspace` + `cargo test -p rustbrain-mcp` (separate crate)  
**Rust:** rustc 1.94.1

---

## Executive Summary

| Metric | Value |
|--------|-------|
| Overall line coverage (workspace) | **48.82%** |
| Overall region coverage | **50.84%** |
| Overall function coverage | **51.69%** |
| Unit tests passing | **1,194** (workspace: 866, MCP: 329) |
| Unit tests failing | **0** |
| Integration tests (ignored, need Docker) | **57** (API: 38, MCP: 19) |
| Crates in workspace | **5** (common, api, ingestion, validator, benchmarker) |
| MCP crate (separate) | **1** |

**Verdict:** Below the 80% target for critical paths. The 48.82% line coverage reflects heavy concentration in serialization/unit tests, while handler execution paths, pipeline runners, and service entrypoints remain largely untested.

---

## Per-Crate Coverage Breakdown

### rustbrain-common — 65.5% lines

| File | Lines | Missed | Coverage |
|------|-------|--------|----------|
| config.rs | 45 | 17 | 62.2% |
| logging.rs | 50 | 50 | **0.0%** |
| types.rs | 131 | 11 | 91.6% |

**Gap:** `logging.rs` has 0% coverage — no unit tests exercise the logging initialization path.

---

### services/api — 41.9% lines

| File | Lines | Missed | Coverage |
|------|-------|--------|----------|
| config.rs | 89 | 63 | 29.2% |
| docker.rs | 297 | 95 | 68.0% |
| errors.rs | 85 | 2 | 97.7% |
| execution/models.rs | 308 | 255 | **17.2%** |
| execution/runner.rs | 247 | 178 | **27.9%** |
| execution/sweeper.rs | 33 | 26 | **21.2%** |
| gaps.rs | 783 | 477 | 39.1% |
| github.rs | 315 | 72 | 77.1% |
| main.rs | 180 | 180 | **0.0%** |
| neo4j.rs | 270 | 162 | 40.0% |
| opencode.rs | 189 | 189 | **0.0%** |
| state.rs | 48 | 3 | 93.8% |
| handlers/artifacts.rs | 216 | 150 | 30.6% |
| handlers/benchmarker.rs | 293 | 195 | 33.5% |
| handlers/chat.rs | 171 | 171 | **0.0%** |
| handlers/execution.rs | 125 | 103 | **17.6%** |
| handlers/graph.rs | 386 | 264 | 31.6% |
| handlers/graph_templates.rs | 473 | 0 | **100.0%** |
| handlers/health.rs | 226 | 205 | **9.3%** |
| handlers/ingestion.rs | 135 | 31 | 77.0% |
| handlers/items.rs | 172 | 172 | **0.0%** |
| handlers/mod.rs | 15 | 0 | 100.0% |
| handlers/pg_query.rs | 254 | 125 | 50.8% |
| handlers/playground.rs | 8 | 8 | **0.0%** |
| handlers/search.rs | 543 | 302 | 44.4% |
| handlers/tasks.rs | 295 | 172 | 41.7% |
| handlers/typecheck.rs | 196 | 142 | **27.6%** |
| handlers/validator.rs | 132 | 74 | 43.9% |
| handlers/workspace.rs | 488 | 284 | 41.8% |
| handlers/workspace_commit.rs | 123 | 57 | 53.7% |
| handlers/workspace_diff.rs | 74 | 26 | 64.9% |
| handlers/workspace_reset.rs | 125 | 68 | 45.6% |
| handlers/workspace_stream.rs | 106 | 37 | 65.1% |
| workspace/lifecycle.rs | 131 | 121 | **7.6%** |
| workspace/manager.rs | 24 | 21 | **12.5%** |
| workspace/models.rs | 128 | 60 | 53.1% |
| workspace/schema.rs | 92 | 28 | 69.6% |

**Critical gaps (0% coverage):**
- `handlers/chat.rs` — OpenCode streaming, session management, message routing
- `handlers/items.rs` — Code item retrieval endpoints
- `opencode.rs` — OpenCode client integration
- `main.rs` — Server initialization, router, middleware

**Critical gaps (<20% coverage):**
- `workspace/lifecycle.rs` (7.6%) — Workspace state machine transitions
- `handlers/health.rs` (9.3%) — Health/readiness checks
- `workspace/manager.rs` (12.5%) — Workspace lifecycle management
- `execution/models.rs` (17.2%) — Execution data models
- `handlers/execution.rs` (17.6%) — Execution handler endpoints
- `execution/sweeper.rs` (21.2%) — Stale workspace cleanup

---

### services/ingestion — 51.2% lines

| File | Lines | Missed | Coverage |
|------|-------|--------|----------|
| derive_detector.rs | 281 | 21 | 92.5% |
| embedding/mod.rs | 627 | 261 | 58.4% |
| embedding/ollama_client.rs | 207 | 133 | 35.8% |
| embedding/qdrant_client.rs | 383 | 234 | 38.9% |
| embedding/text_representation.rs | 732 | 195 | 73.4% |
| graph/batch.rs | 413 | 392 | **5.1%** |
| graph/mod.rs | 263 | 263 | **0.0%** |
| graph/nodes.rs | 578 | 449 | **22.3%** |
| graph/relationships.rs | 484 | 293 | 39.5% |
| main.rs | 120 | 120 | **0.0%** |
| monitoring/audit.rs | 176 | 80 | 54.5% |
| monitoring/health.rs | 256 | 49 | 80.9% |
| monitoring/metrics.rs | 56 | 7 | 87.5% |
| monitoring/monitor.rs | 195 | 24 | 87.7% |
| monitoring/progress.rs | 143 | 13 | 90.9% |
| monitoring/stuck_detector.rs | 199 | 18 | 90.9% |
| parsers/mod.rs | 218 | 18 | 91.7% |
| parsers/syn_parser.rs | 1,141 | 60 | 94.7% |
| parsers/tree_sitter_parser.rs | 334 | 49 | 85.3% |
| pipeline/circuit_breaker.rs | 290 | 39 | 86.6% |
| pipeline/memory_accountant.rs | 151 | 15 | 90.1% |
| pipeline/mod.rs | 163 | 13 | 92.0% |
| pipeline/resilience.rs | 680 | 313 | 54.0% |
| pipeline/runner.rs | 393 | 337 | **14.3%** |
| pipeline/stages.rs | 2,855 | 1,958 | **31.4%** |
| pipeline/streaming_runner.rs | 556 | 511 | **8.1%** |
| typecheck/mod.rs | 204 | 196 | **3.9%** |
| typecheck/resolver.rs | 795 | 253 | 68.2% |

**Critical gaps (0% coverage):**
- `graph/mod.rs` — Neo4j connection, index creation, query execution (requires live DB)
- `main.rs` — CLI entrypoint

**Critical gaps (<15% coverage):**
- `typecheck/mod.rs` (3.9%) — Typecheck pipeline stage (requires rust-analyzer)
- `pipeline/streaming_runner.rs` (8.1%) — Streaming pipeline execution
- `pipeline/runner.rs` (14.3%) — Pipeline orchestration, stage execution
- `graph/batch.rs` (5.1%) — Batch Neo4j write operations

---

### services/validator — 63.0% lines

| File | Lines | Missed | Coverage |
|------|-------|--------|----------|
| comparator.rs | 376 | 10 | 97.3% |
| executor.rs | 102 | 35 | 65.7% |
| extractor.rs | 109 | 10 | 90.8% |
| github.rs | 328 | 133 | 59.4% |
| judge.rs | 282 | 83 | 70.6% |
| main.rs | 170 | 170 | **0.0%** |
| opencode.rs | 189 | 189 | **0.0%** |
| preparator.rs | 105 | 47 | 55.2% |
| scorer.rs | 154 | 2 | 98.7% |
| storage.rs | 211 | 71 | 66.4% |

**Critical gaps:** `main.rs` (0%), `opencode.rs` (0%)

---

### services/benchmarker — 43.8% lines

| File | Lines | Missed | Coverage |
|------|-------|--------|----------|
| ci.rs | 225 | 59 | 73.8% |
| main.rs | 183 | 183 | **0.0%** |
| registry.rs | 204 | 66 | 67.7% |
| reporter.rs | 289 | 124 | 57.1% |
| run_manager.rs | 517 | 365 | **29.4%** |

**Critical gaps:** `main.rs` (0%), `run_manager.rs` (29.4% — core benchmark execution logic)

---

### services/mcp (separate crate — not in llvm-cov)

| Metric | Value |
|--------|-------|
| Unit tests | 329 passed |
| Integration tests | 19 (ignored, need Docker) |
| Line coverage | Not measured (outside workspace) |

Files with confirmed unit tests: server.rs, client.rs, config.rs, error.rs, all 14 tool modules  
Files with **no** unit tests: `main.rs`, `sse_transport.rs`

---

## Test Results Summary

### Workspace — `cargo test --workspace`

| Crate | Passed | Failed | Ignored |
|-------|--------|--------|---------|
| rustbrain-api (unit) | 217 | 0 | 0 |
| rustbrain-api (integration) | 0 | 0 | 38 |
| rustbrain-benchmarker | 46 | 0 | 0 |
| rustbrain-common (unit) | 7 | 0 | 0 |
| rustbrain-common (logging) | 33 | 0 | 0 |
| rustbrain-ingestion (lib) | 241 | 0 | 2 |
| rustbrain-ingestion (bin) | 241 | 0 | 2 |
| rustbrain-validator | 79 | 0 | 0 |
| **Workspace total** | **864** | **0** | **42** |

### MCP Crate — `cargo test --manifest-path services/mcp/Cargo.toml`

| Crate | Passed | Failed | Ignored |
|-------|--------|--------|---------|
| rustbrain-mcp (unit) | 329 | 0 | 0 |
| rustbrain-mcp (integration) | 0 | 0 | 19 |

### Lint & Format

| Check | Result |
|-------|--------|
| `cargo clippy --all-targets` | Clean (0 errors, 0 warnings) |
| `cargo fmt --check` | 2 formatting diffs (docker.rs:128, execution/runner.rs:19) |

---

## Integration Test Coverage (Docker Required)

All 57 integration tests are marked `#[ignore]` and require `docker-compose up`.

### REST API Endpoints — 38 integration tests

| Endpoint | Covered | Test Functions |
|----------|---------|----------------|
| GET /health | ✅ | test_health_returns_healthy |
| GET /metrics | ✅ | test_metrics_endpoint |
| GET /api/snapshot | ✅ | test_snapshot_endpoint |
| POST /tools/search_semantic | ✅ | test_search_semantic_happy_path, test_search_semantic_missing_query |
| POST /tools/aggregate_search | ✅ | test_aggregate_search_happy_path, test_aggregate_search_missing_query |
| GET /tools/get_function | ✅ | test_get_function_not_found, test_get_function_missing_fqn_param |
| GET /tools/get_callers | ✅ | test_get_callers_unknown_fqn_returns_empty, test_get_callers_missing_fqn_param |
| GET /tools/get_trait_impls | ✅ | test_get_trait_impls_happy_path, test_get_trait_impls_missing_param |
| GET /tools/find_usages_of_type | ✅ | test_find_usages_of_type_happy_path, test_find_usages_of_type_missing_param |
| GET /tools/get_module_tree | ✅ | test_get_module_tree_happy_path, test_get_module_tree_missing_param |
| POST /tools/query_graph | ✅ | test_query_graph_read_only_happy_path, test_query_graph_rejects_write_cypher, test_query_graph_both_fields_missing |
| GET /tools/find_calls_with_type | ✅ | test_find_calls_with_type_happy_path, test_find_calls_with_type_missing_param |
| GET /tools/find_trait_impls_for_type | ✅ | test_find_trait_impls_for_type_happy_path, test_find_trait_impls_for_type_missing_param |
| POST /tools/pg_query | ✅ | test_pg_query_select_happy_path, test_pg_query_rejects_insert, test_pg_query_rejects_drop, test_pg_query_rejects_system_tables |
| GET /api/ingestion/progress | ✅ | test_ingestion_progress |
| POST /api/artifacts (+ CRUD) | ✅ | test_artifacts_crud_lifecycle |
| POST /api/tasks (+ CRUD) | ✅ | test_tasks_crud_lifecycle |
| POST /tools/chat | ✅ | test_chat_happy_path, test_chat_missing_message |
| GET /tools/chat/stream | ✅ | test_chat_stream_is_sse, test_chat_stream_post_returns_405 |
| POST /tools/chat/sessions | ✅ | test_chat_sessions_lifecycle |
| GET /tools/chat/sessions | ✅ | test_chat_sessions_lifecycle |
| DELETE /tools/chat/sessions/:id | ✅ | test_chat_sessions_lifecycle |
| **POST /tools/chat/send** | **❌** | No integration test |
| POST /tools/chat/sessions/:id/fork | ✅ | test_chat_sessions_lifecycle |
| POST /tools/chat/sessions/:id/abort | ✅ | test_chat_sessions_lifecycle |

### MCP Tools — 19 integration tests

All 14 MCP tools covered:
search_code, get_function, get_callers, get_trait_impls, find_type_usages, get_module_tree, query_graph, find_calls_with_type, find_trait_impls_for_type, pg_query, context_store, status_check, task_update, aggregate_search

Plus: initialize/list_tools, health check, unknown tool handling, write-rejection for query_graph and pg_query.

---

## Top 10 Untested Critical Paths

| # | Path | Lines | Coverage | Risk |
|---|------|-------|----------|------|
| 1 | `services/api/src/handlers/chat.rs` | 171 | 0.0% | **Critical** — OpenCode streaming, session management, message routing. All chat functionality is untested at unit level. |
| 2 | `services/ingestion/src/pipeline/streaming_runner.rs` | 556 | 8.1% | **Critical** — Streaming pipeline execution is the primary ingestion mode. Only 45 of 556 lines covered. |
| 3 | `services/ingestion/src/graph/mod.rs` | 263 | 0.0% | **High** — Neo4j connection, query execution, index creation. Requires live DB but should have mock tests. |
| 4 | `services/api/src/opencode.rs` | 189 | 0.0% | **High** — OpenCode client integration used by workspace execution. Zero test coverage. |
| 5 | `services/ingestion/src/typecheck/mod.rs` | 204 | 3.9% | **High** — Typecheck pipeline stage (rust-analyzer integration). Core feature with near-zero coverage. |
| 6 | `services/api/src/workspace/lifecycle.rs` | 131 | 7.6% | **High** — Workspace state machine. Only 10 of 131 lines covered despite unit tests existing for transitions. |
| 7 | `services/ingestion/src/pipeline/runner.rs` | 393 | 14.3% | **High** — Pipeline orchestration. Config tests exist but actual `run()` logic is untested. |
| 8 | `services/api/src/handlers/items.rs` | 172 | 0.0% | **Medium** — Code item retrieval endpoints. Zero coverage. |
| 9 | `services/mcp/src/sse_transport.rs` | 231 | N/A | **Medium** — Core SSE session management for MCP transport. Outside workspace coverage, confirmed no tests. |
| 10 | `services/ingestion/src/graph/batch.rs` | 413 | 5.1% | **Medium** — Batch Neo4j write operations. Essential for ingestion throughput. |

---

## Modules at 0% Coverage

| Module | File | Lines | Reason |
|--------|------|-------|--------|
| API chat handler | handlers/chat.rs | 171 | No unit tests |
| API items handler | handlers/items.rs | 172 | No unit tests |
| API OpenCode client | opencode.rs | 189 | No unit tests |
| API main | main.rs | 180 | Entrypoint (typical) |
| API playground | handlers/playground.rs | 8 | Trivial static serving |
| Ingestion graph driver | graph/mod.rs | 263 | Requires live Neo4j |
| Ingestion main | main.rs | 120 | Entrypoint (typical) |
| Common logging | logging.rs | 50 | No unit tests |
| Validator main | main.rs | 170 | Entrypoint (typical) |
| Validator OpenCode | opencode.rs | 189 | No unit tests |
| Benchmarker main | main.rs | 183 | Entrypoint (typical) |
| MCP main | main.rs | — | Entrypoint (typical) |
| MCP SSE transport | sse_transport.rs | 231 | No unit tests |

---

## Recommendations

### Priority 1 — Must-have before v1.0

1. **Add unit tests for `handlers/chat.rs`** — Chat is the primary user-facing feature. Test message routing, session lifecycle, and streaming response formatting without requiring a live OpenCode backend (mock the client).

2. **Add unit tests for `pipeline/streaming_runner.rs`** — The streaming runner is the primary ingestion path. Mock Neo4j/Qdrant clients to test stage sequencing, error recovery, and progress tracking.

3. **Add unit tests for `opencode.rs`** — Both API and validator copies. Mock the HTTP client to test request construction, response parsing, and error handling.

4. **Add integration tests for `POST /tools/chat/send`** — Only uncovered REST endpoint.

### Priority 2 — Important for reliability

5. **Mock-based tests for `graph/mod.rs`** — Use a mock Neo4j bolt client to test query execution, connection pooling, and error handling.

6. **Unit tests for `typecheck/mod.rs`** — Mock rust-analyzer to test the typecheck stage pipeline integration.

7. **Unit tests for `workspace/lifecycle.rs`** — State machine transitions should have exhaustive test coverage. Current 7.6% suggests the async execution paths are untested.

8. **Unit tests for `sse_transport.rs`** — SSE session management, message routing, connection lifecycle.

### Priority 3 — Nice-to-have

9. **Smoke tests for main.rs entrypoints** — At minimum, verify router construction and middleware setup without binding a port.

10. **Mock-based tests for `pipeline/runner.rs`** — Test the `run()` orchestration path with mocked stage results.

---

## Coverage Trend

| Date | Line Coverage | Notes |
|------|--------------|-------|
| 2026-04-08 | 48.82% | Baseline measurement |

Target: **≥80%** on critical paths (handlers, pipeline stages, security endpoints) before v1.0 release.
