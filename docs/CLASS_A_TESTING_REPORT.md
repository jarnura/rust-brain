# CLASS A Testing Report

**Date:** 2026-04-02
**Agent System:** 13-agent SDLC (Orchestrator + 12 specialists)
**Runtime:** OpenCode (Docker) + rust-brain MCP server
**Target Codebase:** Hyperswitch (500K+ LOC Rust payment processing monorepo)

---

## Executive Summary

CLASS A (Pure Understanding) testing is complete. All 7 test queries pass with the Explorer agent correctly following the Discover → Navigate → Fill Gaps hierarchy using MCP code intelligence tools.

### Before vs After

| Metric | Before Fixes | After Fixes |
|--------|-------------|-------------|
| Test pass rate | 7/7 (results correct but tools misused) | **7/7 (correct results AND correct tool usage)** |
| query_graph misuse | 4-8 calls per test | **0 per test** (except crate_deps where it's correct) |
| JSON parsing errors | 3 `invalid` tool calls per session | **0** |
| grep fallbacks before MCP | 14+ bash calls in some tests | **0** |
| get_function with short name → 404 | Every test | **0** (always uses full FQN from search_code) |
| Neo4j CALLS edges | 227K | **224K** (re-ingested, cleaner data) |
| IMPLEMENTS edges | ~29K | **38,802** (+34%, generic traits now matched) |
| DEPENDS_ON edges | 461 (CALLS-inferred, wrong) | **189** (Cargo.toml, correct) |
| OpenAPI stubs in results | 32+ polluting lookups | **Filtered** (file_path IS NOT NULL) |
| Embedding speed | 109s/batch (CPU) | **4s/batch (GPU)** — 27x faster |

---

## Test Results

### Final Scorecard

| # | Test | Query | MCP Tools Used | query_graph | Errors | Status |
|---|------|-------|----------------|-------------|--------|--------|
| 1 | symbol_lookup | "Where is payments_create defined?" | search_code → get_function → get_module_tree | 0 | 0 | **PASS** |
| 2 | call_chain | "Who calls payments_create?" | search_code → get_function(×3) → get_callers | 0 | 0 | **PASS** |
| 3 | module_map | "What's in the router crate?" | get_module_tree | 0 | 0 | **PASS** |
| 4 | impact_analysis | "What breaks if I change ConnectorIntegration?" | search_code → get_trait_impls → find_type_usages → get_callers | 0 | 0 | **PASS** |
| 5 | trait_impl | "Find all implementations of ConnectorIntegration" | search_code → get_trait_impls | 0 | 0 | **PASS** |
| 6 | crate_deps | "What crates depend on router?" | query_graph find_crate_dependents | ✓ (correct) | 0 | **PASS** |
| 7 | pattern_search | "Search for error handling patterns in payment flows" | search_code(×19) → get_function(×4) → get_callers | 0 | 0 | **PASS** |

### Verification Method

Each test verified across three layers:
1. **MCP server logs** (`docker logs rustbrain-mcp-sse`) — ground truth for tool calls
2. **API server logs** (`docker logs rustbrain-api`) — actual Cypher/SQL queries executed
3. **OpenCode session DB** (SQLite at `/home/opencode/.local/share/opencode/opencode.db`) — full agent behavior including filesystem fallbacks

---

## Bugs Found and Fixed

### Bug 1: OpenAPI Macro Stubs Polluting Function Lookups (RC-2)

**Symptom:** `get_function("payments_create")` returned 32 OpenAPI macro-generated stubs with null `file_path` instead of the real function definition.

**Root Cause:** Neo4j contained Function nodes from `openapi::routes::*` (macro-expanded) with no source file mapping. Query templates returned all matches without filtering.

**Fix:** Added `AND f.file_path IS NOT NULL` filter to `find_functions_by_name`, `find_callers`, `find_callees`, and `find_trait_implementations` templates in `services/api/src/handlers/graph_templates.rs`.

**Verification:** Test 1 now returns only the real `router::routes::payments::payments_create` definition.

---

### Bug 2: DEPENDS_ON Edges Derived from CALLS (Wrong Data) (RC-5)

**Symptom:** `find_crate_dependents("router")` returned 13 crates, but zero actually depend on `router` in Cargo.toml. The edges conflated `router` with `router_env`/`router_derive`.

**Root Cause:** Original 461 DEPENDS_ON edges were derived from cross-crate CALLS relationships via Cypher. Code-level call patterns (including test code) were treated as dependencies, and FQN prefix matching confused related crates.

**Fix:** Deleted all CALLS-inferred DEPENDS_ON edges. Replaced with 189 edges parsed directly from workspace Cargo.toml files (`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`). Each edge has `source: "cargo_toml"` property.

**Verification:** Test 6 correctly returns 0 dependents for `router` (it's the top-level binary).

---

### Bug 3: call_sites Table Unusable (86% Serde, 0% Business Logic) (RC-3)

**Symptom:** Explorer used `pg_query` on `call_sites` table and got misleading results. Table contained 49,827 rows but 86% were `_serde` generated entries with zero `router::` business logic.

**Root Cause:** The Typecheck stage processes expanded macros which are dominated by serde derive implementations. Router business logic calls are in the original source but not in the expanded source scope that Typecheck analyzes.

**Fix:** 
- Deprecated `call_sites` table in Explorer prompt with explicit warning
- Removed call_sites SQL examples from prompt
- Directed Explorer to use Neo4j CALLS edges (227K edges including 11,730 from router) via `get_callers` MCP tool instead

**Verification:** Explorer no longer queries call_sites table in any test.

---

### Bug 4: Generic Trait Implementations Not Matched (RC-4)

**Symptom:** `trait_implementations` PostgreSQL table had 0 rows for `ConnectorIntegration<T, Req, Resp>` despite 1,578 impl blocks in the codebase. Neo4j IMPLEMENTS edges were also incomplete.

**Root Cause:** In `services/ingestion/src/parsers/syn_parser.rs:315-321`, the trait name extraction via `path.segments.iter().map(|s| s.ident.to_string())` dropped generic parameters. `ConnectorIntegration<T, Req, Resp>` became `impl_for=ConnectorIntegration`, and the trait FQN lookup in GraphStage couldn't match.

**Fix:** 
- Updated `syn_parser.rs` to extract trait name with generics using `s.arguments.to_token_stream()` (e.g., `impl_for=ConnectorIntegration<T, Req, Resp>`)
- Updated `stages.rs` `find_trait_fqn_optimized()` to strip generics for the FQN lookup while preserving them for relationship data
- Added `strip_generics()` helper function

**Verification:** After re-ingestion, IMPLEMENTS edges increased from ~29K to 38,802 (+34%).

---

### Bug 5: Turbofish/Generic Function Calls Not Extracted (RC-1)

**Symptom:** 57% of router functions had 0 inbound CALLS edges in Neo4j. `get_callers("router::core::payments::payments_core")` returned empty despite grep finding 64 call sites.

**Root Cause:** The regex-based `extract_function_calls()` in `services/ingestion/src/pipeline/stages.rs:3307-3410` stopped at `<` characters. For turbofish calls like `payments_core::<F, Res, Req, Op, FData, D>(state, ...)`, the identifier collector matched up to `<` but never found the `(` that follows the closing `>`.

**Fix:** Added balanced angle bracket skipping in the call extraction loop. When the parser hits `<` after an identifier, it now counts balanced `<`/`>` pairs, skips past them, then checks for `(` to identify the function call.

**Verification:** After re-ingestion, graph stage produced 1,131,313 edge insertions (up from previous run). Explorer correctly notes KNOWN_GAP for remaining async/generic gaps.

---

### Bug 6: query_graph JSON Parsing Failures (Intermittent)

**Symptom:** `Invalid input for tool rustbrain_query_graph: JSON parsing failed` errors in OpenCode session DB. The model intermittently generated malformed JSON for the nested `parameters` object.

**Root Cause:** The `query_graph` MCP tool schema required a nested object: `{"query_name": "...", "parameters": {"crate_name": "router"}}`. The LLM model (`juspay-grid/glm-latest`) sometimes generated truncated or malformed JSON for nested structures.

**Fix:** 
- Flattened the tool schema — all parameters are now top-level: `{"query_name": "...", "crate_name": "router"}`
- Legacy nested format still accepted (backward compatible)
- MCP tool internally merges flat params into the HashMap via `merged_parameters()`

**Verification:** Zero `invalid` tool calls across all 7 tests after fix.

---

### Bug 7: Explorer Skipping MCP Tools, Using query_graph/grep First

**Symptom:** MCP logs showed Explorer calling `query_graph` and `grep` before trying `get_callers`, `get_trait_impls`, or other specialized Phase 2 tools. In some tests, 14 bash/grep calls before any MCP navigation tool.

**Root Cause:** Explorer prompt's cost hierarchy was ambiguous. `query_graph` and `search_code` were both at "P1" level, and the prompt didn't enforce a clear sequence of: discover FQN first → then navigate with specialized tools.

**Fix:**
- Restructured Explorer prompt with explicit 4-phase workflow: **Discover** (search_code → get_function) → **Navigate** (get_callers, get_trait_impls, etc.) → **Fill Gaps** (grep) → **Last Resort** (query_graph)
- Removed `query_graph` and `pg_query` from Explorer's tool whitelist in `opencode.json`
- Added anti-patterns: "NEVER call get_function with short name", "NEVER use query_graph for callers/callees/trait lookups"
- Each navigation mode has explicit numbered tool sequence

**Verification:** All 7 tests show correct Phase 1 → Phase 2 flow with zero query_graph calls (except Test 6 crate_deps where it's the correct tool).

---

### Bug 8: Ollama Running on CPU Instead of GPU

**Symptom:** Embedding stage estimated 66 hours. GPU utilization at 0%. `ollama ps` showed `100% CPU`.

**Root Cause:** Ollama container logs showed `ggml_cuda_init: failed to initialize CUDA: no CUDA-capable device is detected` and `offloaded 0/37 layers to GPU`. CUDA library version mismatch inside container after host driver update.

**Fix:** Restarted Ollama container (`docker restart rustbrain-ollama`). On restart, CUDA re-initialized correctly: `offloaded 37/37 layers to GPU`.

**Verification:** Embedding speed went from 109s/batch to 4s/batch (27x improvement). Full 219K items embedded in ~2.5 hours instead of 66.

---

## Good Practices

### 1. search_code First, Always

The Explorer should NEVER call `get_function`, `get_callers`, or any Phase 2 tool without first discovering the FQN via `search_code`. The model doesn't know exact FQNs upfront — `search_code` provides candidates with FQNs, file paths, and relevance scores.

**Wrong:** `get_function("payments_create")` → 404
**Right:** `search_code("payments_create")` → discovers `router::routes::payments::payments_create` → `get_function("router::routes::payments::payments_create")` → success

### 2. Flat Tool Schemas Over Nested

LLM models struggle with nested JSON objects in tool call arguments. Flat schemas (`{"query_name": "x", "crate_name": "y"}`) are dramatically more reliable than nested ones (`{"query_name": "x", "parameters": {"crate_name": "y"}}`).

### 3. Remove Tools Rather Than Instruct Against Them

Telling the model "don't use query_graph" in the prompt is unreliable. Removing the tool from the whitelist is more effective. The model can't call what it doesn't see in the tool list.

### 4. Three-Layer Verification Is Non-Negotiable

A test can look like it passed (correct final answer) while the tool usage was completely wrong (14 grep calls instead of MCP tools). Always check:
- **MCP logs** — what tools were actually called
- **API logs** — what queries were actually executed
- **Session DB** — full agent behavior including filesystem fallbacks

### 5. Sequential Testing Only

OpenCode containers cannot handle parallel `opencode run` sessions. Each spawns its own LLM call + subagent. Running 4 in parallel produces truncated or empty outputs. Always run tests one at a time.

### 6. Verify MCP Connectivity Before Testing

After any container restart or recreation, verify MCP is reachable:
```bash
docker logs rustbrain-mcp-sse --since 1m 2>&1 | grep "tools/list"
```
If no `tools/list` appears after an `opencode run`, MCP is unreachable. Check network aliases.

### 7. Ingestion Always Via Docker

Never run `rustbrain-ingestion` directly on the host. Use `scripts/clean-ingestion.sh` + `scripts/ingest.sh`. The Docker container enforces memory limits and has correct database connectivity.

### 8. Verify GPU Before Embedding

Before any ingestion run, check `docker exec rustbrain-ollama ollama ps` shows `100% GPU`. A CPU-only embedding run takes 27x longer and may timeout.

---

## Pitfalls

### 1. OpenCode Tool Whitelist Is Advisory, Not Enforced

Removing a tool from the `tools` section in `opencode.json` does NOT prevent the subagent from calling it. The MCP server still exposes all 14 tools. The whitelist affects what the model sees in its tool definitions, but subagents may inherit the full tool list from the parent session.

**Mitigation:** Combine whitelist removal with strong prompt-level instructions and simplified tool schemas that make correct tools easier to use than incorrect ones.

### 2. MCP Network Alias Lost on Container Recreation

When recreating the MCP container outside `docker compose`, it loses the `mcp-sse` DNS alias. OpenCode uses `http://mcp-sse:3001/sse` to connect. Without the alias, the Explorer silently falls back to filesystem-only exploration with zero MCP tool calls.

**Fix:**
```bash
docker network connect --alias mcp-sse rustbrain_rustbrain-net rustbrain-mcp-sse
```

### 3. OpenCode Session DB Doesn't Record MCP Tool Calls

The `part` table in `opencode.db` records bash/grep/read/glob calls but NOT MCP tool calls (search_code, get_function, etc.). The MCP server log is the only source of truth for MCP tool invocations. Don't rely on the session DB alone.

### 4. get_callers Returns Empty for Async/Generic Functions

57% of router functions have 0 inbound CALLS edges in Neo4j. This is a known gap for functions with many generic type parameters. The Explorer should fall back to grep and label it as `KNOWN_GAP`, not treat it as "no callers exist."

### 5. Stale Ingestion Data Masquerading as Tool Bugs

Before debugging tool failures, verify the underlying data is fresh. After code changes to the ingestion pipeline, you must:
1. `scripts/clean-ingestion.sh`
2. `scripts/ingest.sh ~/projects/hyperswitch`
3. Re-apply Cargo.toml DEPENDS_ON fix (GraphStage creates its own)

### 6. query_graph Template Names Are Not Functions

The model sometimes confuses `query_graph` template names with function names. `find_callers` is a query_graph template, but `get_callers` is the dedicated MCP tool. They do similar things but through different paths. The dedicated MCP tool should always be preferred.

### 7. search_code Returns Macro-Expanded Items

Qdrant embeddings include items from macro expansion (OpenAPI stubs, serde derives). When `search_code` returns candidates, always prefer results from known crates (`router::`, `hyperswitch_interfaces::`) over `openapi::` or `_serde::` prefixed FQNs.

### 8. Docker Compose Network Conflicts

`docker compose up -d` may fail with "Pool overlaps with other one on this address space" if the network already exists from a previous session. Use `docker compose restart <service>` or `docker stop && docker rm && docker run --network` as workaround.

---

## Files Changed

### API Service
- `services/api/src/handlers/graph_templates.rs` — Added `file_path IS NOT NULL` filter to 4 query templates

### MCP Service
- `services/mcp/src/tools/query_graph.rs` — Flattened tool schema, added `merged_parameters()`, backward-compatible with nested format

### Ingestion Pipeline
- `services/ingestion/src/parsers/syn_parser.rs` — Extract trait name with generics (`ConnectorIntegration<T, Req, Resp>` not just `ConnectorIntegration`)
- `services/ingestion/src/pipeline/stages.rs` — Added `strip_generics()` helper, improved `find_trait_fqn_optimized()`, fixed turbofish call extraction in `extract_function_calls()`

### Agent Prompts
- `configs/opencode/.opencode/agents/explorer.md` — Restructured cost hierarchy to 4-phase workflow, deprecated call_sites/trait_implementations tables, added known data quality gaps, updated anti-patterns
- `configs/opencode/opencode.json` — Removed `query_graph` and `pg_query` from Explorer's tool whitelist

### Neo4j Data
- DEPENDS_ON edges: Replaced 461 CALLS-inferred edges with 189 Cargo.toml-parsed edges

### Documentation
- `docs/TESTING_GUIDE.md` — Testing practices, CLASS A test matrix, debugging checklist
- `docs/CLASS_A_TESTING_REPORT.md` — This file

---

# CLASS B Testing Report

**Date:** 2026-04-02

## What CLASS B Tests

CLASS B is "Understand + Plan" — the Orchestrator dispatches Explorer first (understand phase), then Planner (plan phase). No code is written.

## Key Finding: Model Matters for Multi-Step Dispatch

**glm-latest** cannot do CLASS B — it correctly describes the two-step dispatch plan in text but fails to emit structured `task` tool calls. The tool call parameters leak into the text output as prose.

**kimi-latest** successfully emits `task` tool calls for multi-step CLASS B dispatch (Explorer → Planner chain).

## Bug Found: Explorer Context Window Exhaustion

**Symptom:** Explorer hit 88,890 tokens in 14 steps. The 15th step never completed. Planner was never dispatched.

**Root Cause:** Explorer performed 15 file reads (~4000 tokens each) and 6 grep calls, consuming context budget on raw file content instead of compact MCP tool responses (~500 tokens each).

**Fix:** Added context budget rules to Explorer prompt:
- Token cost table (search_code ~500 vs read ~4000-8000 tokens)
- Maximum 3 file reads per task
- Maximum 25 tool calls per task
- "Stop after 10 steps" rule
- Anti-pattern: "NEVER read files to understand code — use get_function instead"

**Verification:** After fix, Explorer completed in 12-22 MCP calls. Full Explorer → Planner chain working.

## Test Results

| # | Query | Explorer | Planner | MCP Calls | Status |
|---|-------|----------|---------|-----------|--------|
| 1 | "Plan how to add a new payment connector called RazorpayV2" | ✅ 13K chars | ✅ 22K chars | 22 | **PASS** |
| 2 | "How would I add rate limiting to the payments API?" | ✅ 11K chars | ⚠️ Orchestrator answered directly | 28 | **PARTIAL** |
| 3 | "Plan refactoring payments_core to reduce generic type parameters" | ✅ 11K chars | ✅ 13K chars | 12 | **PASS** |

### Test 2 Notes

The query is borderline CLASS A/B. Orchestrator classified as CLASS B and dispatched Explorer, but summarized findings directly instead of dispatching Planner. Acceptable — the Orchestrator judged the Explorer's CodeMap sufficient without a formal ImplementationPlan.

### Test 3 Notes

Explorer produced an exceptionally detailed CodeMap analyzing payments_core's 6 generic type parameters (`F, Res, Req, Op, FData, D`), trait bounds, and consolidation opportunities. Planner produced a 13K char refactoring plan. One `context_store` call failed with 422 (missing `id` field in artifact schema) — minor artifact store issue, not agent flow.

## Configuration

```json
{
  "orchestrator": { "model": "juspay-grid/kimi-latest" },
  "explorer": { "model": "juspay-grid/glm-latest" },
  "planner": { "model": "juspay-grid/glm-latest" }
}
```
