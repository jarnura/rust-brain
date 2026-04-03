# Agent System Testing Guide

This guide documents the testing practices for the 13-agent SDLC system running on rust-brain. These practices were developed through systematic CLASS A testing and debugging of the Orchestrator + Explorer agent flow.

---

## 1. Three-Layer Verification

Every test should be verified across three independent log sources. Any single layer can mask issues the others reveal.

### Layer 1: MCP Server Logs (Ground Truth for Tool Calls)

```bash
docker logs rustbrain-mcp-sse --since 5m 2>&1 | grep "Tool call:"
```

Shows every `tools/call` with arguments, responses, and errors. This is the **ground truth** for what MCP tools were actually invoked. The OpenCode session DB does NOT record MCP tool calls.

**What to look for:**
- Tool name and arguments for each call
- `Tool error:` lines indicating failures
- `Invalid input` lines indicating malformed JSON from the model
- Sequence of calls (did the agent follow the cost hierarchy?)

### Layer 2: API Server Logs (Actual Queries Executed)

```bash
docker logs rustbrain-api --since 5m 2>&1 | grep -E "Cypher|Semantic|Get function"
```

Shows the actual Cypher queries, semantic searches, and function lookups executed by the API. Confirms:
- Whether `file_path IS NOT NULL` filters are applied in query templates
- What query_graph templates resolve to
- Whether semantic search queries are reasonable

### Layer 3: OpenCode Session DB (Full Agent Behavior)

```bash
docker cp rustbrain-opencode:/home/opencode/.local/share/opencode/opencode.db /tmp/opencode.db

python3 << 'PYEOF'
import sqlite3, json

conn = sqlite3.connect('/tmp/opencode.db')
cursor = conn.cursor()

# List recent sessions
cursor.execute("SELECT id, title FROM session ORDER BY time_created DESC LIMIT 10")
for sid, title in cursor.fetchall():
    print(f"  {sid[:25]}... | {title}")

# Extract tool calls from a specific session
SESSION_ID = "ses_XXXXX"  # Replace with actual ID
cursor.execute("SELECT data FROM part WHERE session_id=? ORDER BY time_created ASC", (SESSION_ID,))
for (data_str,) in cursor.fetchall():
    data = json.loads(data_str)
    if data.get('type') == 'tool':
        tool = data.get('tool','')
        status = data.get('state',{}).get('status','')
        inp = json.dumps(data['state'].get('input',{}))[:150]
        print(f"  [{tool}] ({status}) {inp}")
PYEOF
```

Shows bash/grep/read/glob calls that MCP logs don't capture. The `part` table with `type='tool'` has the complete tool trace per session, including:
- `invalid` tool calls (model sent malformed JSON)
- `bash` commands with their arguments
- `read` file accesses
- `grep`/`glob` filesystem searches

---

## 2. CLASS A Test Matrix

Seven standardized queries covering each Explorer navigation mode. Each tests different tool combinations and known edge cases.

### Test Queries

| # | Query | Mode | Expected Tools | Key Verification |
|---|-------|------|----------------|-----------------|
| 1 | "Where is payments_create defined?" | symbol_lookup | search_code -> get_function(FQN) | Correct file path, no OpenAPI stubs |
| 2 | "Who calls payments_create?" | call_chain | search_code -> get_function -> get_callers | Entry point identified, KNOWN_GAP for async |
| 3 | "What's in the router crate?" | module_map | get_module_tree | Full module hierarchy |
| 4 | "What breaks if I change ConnectorIntegration?" | impact_analysis | search_code -> get_trait_impls -> find_type_usages -> get_callers | Impl count, affected crates |
| 5 | "Find all implementations of ConnectorIntegration" | trait_impl | search_code -> get_trait_impls | 381+ impl blocks |
| 6 | "What crates depend on router?" | crate_deps | query_graph find_crate_dependents | 0 dependents (top-level binary) |
| 7 | "Search for error handling patterns in payment flows" | pattern_search | search_code | Error types, retry logic |

### Running a Test

```bash
# Run the test
docker exec rustbrain-opencode opencode run --format json "YOUR_QUERY_HERE" 2>/dev/null > /dev/null &

# Wait for completion (60-180s depending on query complexity)
sleep 120

# Check MCP tool calls
docker logs rustbrain-mcp-sse --since 3m 2>&1 | grep "Tool call:" | \
  python3 -c "
import sys, re
for i, line in enumerate(sys.stdin):
    m = re.search(r'(\d{2}:\d{2}:\d{2}).*Tool call: (\w+) with args (.*)', line)
    if m:
        print(f'{i+1:2}. [{m.group(1)}] {m.group(2):30} {m.group(3)[:120]}')
"

# Check for errors
docker logs rustbrain-mcp-sse --since 3m 2>&1 | grep "Tool error:" | head -5
```

### Success Criteria

- Orchestrator prints `**Phase**: CLASS A` and dispatches to `explorer`
- Explorer follows cost hierarchy: search_code -> get_function(FQN) -> Phase 2 tools -> grep -> query_graph
- Zero `invalid` tool calls (no malformed JSON)
- Zero `Tool error:` in MCP logs
- Results contain FQNs, file paths, and line numbers
- grep fallbacks are labeled as `KNOWN_GAP` or `grep_fallback` in search traces

### Failure Signals

| Signal | What It Means | Fix |
|--------|--------------|-----|
| Orchestrator calls MCP tools directly | Orchestrator prompt not enforcing delegation | Update orchestrator.md |
| Explorer starts with bash/grep | MCP tools not available or cost hierarchy violated | Check MCP connectivity, update explorer.md |
| `get_function("payments_create")` -> 404 | Short name used instead of FQN | Explorer should use search_code first to discover FQN |
| 32 OpenAPI stubs in results | `file_path IS NOT NULL` filter not working | Check graph_templates.rs |
| `Invalid input for tool` | Model generating malformed JSON | Simplify tool schema (flatten nested objects) |
| Zero MCP calls in session | MCP server unreachable from OpenCode | Check network alias, restart both containers |

---

## 3. Cost Hierarchy Validation

The Explorer must follow this sequence. Track violations during testing.

```
PHASE 1 — DISCOVER (find the FQN first)
  search_code              -> gives you FQN candidates
  get_function(full_fqn)   -> REQUIRES full FQN from search_code

PHASE 2 — NAVIGATE (use FQN with specialized tools)
  get_callers(fqn)
  get_trait_impls(name)
  find_type_usages(fqn)
  get_module_tree(name)
  find_calls_with_type(fqn)
  find_trait_impls_for_type(fqn)

PHASE 3 — FILL GAPS
  bash grep/rg

PHASE 4 — LAST RESORT
  query_graph              -> ONLY for crate deps, neighbors, multi-hop
  pg_query                 -> ONLY for bulk counts/aggregation
```

**Violations to flag:**
- `query_graph` used before `get_callers`/`get_trait_impls` for the same data
- `get_function` called with short name (must be full FQN)
- `grep` used before Phase 2 tools
- `pg_query` used for caller/callee queries (call_sites table is deprecated)

---

## 4. Infrastructure Checks

### Sequential-Only OpenCode Sessions

Cannot run parallel `opencode run` commands — the container overwhelms and produces truncated output. Always run tests one at a time.

### MCP Container Network Alias

When recreating the MCP container outside docker-compose, you MUST add the network alias:

```bash
docker network connect --alias mcp-sse rustbrain_rustbrain-net rustbrain-mcp-sse
```

Without this, OpenCode can't resolve `mcp-sse:3001` and silently falls back to filesystem-only exploration (zero MCP tool calls).

**Verification:**
```bash
docker exec rustbrain-opencode sh -c "curl -sf http://mcp-sse:3001/sse >/dev/null && echo OK || echo FAIL"
```

### GPU Verification for Embeddings

Before running ingestion, verify Ollama is using GPU:

```bash
docker exec rustbrain-ollama ollama ps
```

Must show `100% GPU`, not `100% CPU`. If CPU-only:
1. Check `nvidia-smi` on host
2. Restart Ollama: `docker restart rustbrain-ollama`
3. Verify: `docker logs rustbrain-ollama 2>&1 | grep "offloaded.*layers"`

Expected: `offloaded 37/37 layers to GPU`

Speed difference: ~4s/batch (GPU) vs ~109s/batch (CPU) = **27x faster**

### Ingestion via Docker Only

Never run the ingestion binary directly on the host machine.

```bash
# Clean databases first
./scripts/clean-ingestion.sh

# Run ingestion via Docker
./scripts/ingest.sh ~/projects/hyperswitch --memory-budget 32GB
```

### Container Health Checks

Before running tests, verify all services are healthy:

```bash
docker ps --filter name=rustbrain --format "{{.Names}} {{.Status}}" | sort
```

All containers should show `(healthy)`. Key ones:
- `rustbrain-api` — REST API
- `rustbrain-mcp-sse` — MCP server
- `rustbrain-opencode` — Agent runtime
- `rustbrain-ollama` — Embedding model
- `rustbrain-postgres` — PostgreSQL
- `rustbrain-neo4j` — Neo4j graph
- `rustbrain-qdrant` — Vector store

---

## 5. Known Data Quality Gaps

These are documented in the Explorer prompt under "Known Neo4j data quality gaps":

| Gap | Impact | Workaround |
|-----|--------|-----------|
| Async/generic functions (57% of router functions have 0 inbound CALLS) | `get_callers` returns empty | Explorer falls back to grep, labels as KNOWN_GAP |
| call_sites table (86% serde, 0% business logic) | Misleading if queried | Deprecated in Explorer prompt, use Neo4j CALLS instead |
| trait_implementations table missing ConnectorIntegration | 0 rows for complex generic traits | Use `get_trait_impls` MCP tool (uses Neo4j IMPLEMENTS) |
| OpenAPI macro stubs (null file_path) | Pollute function lookups | Filtered by `file_path IS NOT NULL` in query templates |

---

## 6. Debugging Checklist

When a test fails, check in this order:

1. **MCP connectivity**: Can OpenCode reach `mcp-sse:3001`?
2. **MCP tool list**: Does `tools/list` return 14 tools?
3. **Tool call errors**: Any `Tool error:` or `invalid` in MCP logs?
4. **API errors**: Any 4xx/5xx in API logs?
5. **Cost hierarchy**: Did Explorer follow search_code -> get_function -> Phase 2 flow?
6. **FQN correctness**: Did Explorer use full FQN or short name?
7. **Grep fallback count**: How many bash/grep calls vs MCP calls?
8. **Data quality**: Is the expected data actually in Neo4j/PostgreSQL?

### Quick Neo4j Spot Checks

```bash
# Count nodes and edges
curl -s -u neo4j:rustbrain_dev_2024 -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (n) RETURN count(n)"},{"statement":"MATCH ()-[r]->() RETURN count(r)"}]}' \
  http://localhost:7474/db/neo4j/tx/commit

# Check CALLS edges for a specific function
curl -s -u neo4j:rustbrain_dev_2024 -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (caller)-[:CALLS]->(f {fqn: $fqn}) RETURN caller.fqn LIMIT 10", "parameters":{"fqn":"router::core::payments::payments_core"}}]}' \
  http://localhost:7474/db/neo4j/tx/commit

# Check DEPENDS_ON
curl -s -u neo4j:rustbrain_dev_2024 -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (dep)-[:DEPENDS_ON]->(c:Crate {name: $name}) RETURN dep.name", "parameters":{"name":"router"}}]}' \
  http://localhost:7474/db/neo4j/tx/commit
```
