# ADR-008 — Caller/Callee Traversal API (REQ-DP-03)

**Status:** Accepted  
**Deciders:** Architect, Board  
**Implementing issue:** RUSAA-74

---

## Context

The data-plane requires depth-limited BFS traversal of the Neo4j call graph to answer
"who calls X?" and "what does X call?". The existing `/tools/get_callers` endpoint is
workspace-header–based, depth-capped at 5, and returns flat lists without provenance.
The new v1 API uses path-based repo scoping, supports up to depth 10, returns structured
graph results (nodes + edges), and annotates each edge with dispatch provenance.

---

## Decision

### §3.3 — Caller/Callee Traversal Endpoints

#### Endpoints

```
GET /v1/repos/{repo_id}/items/{fqn_b64}/callers
GET /v1/repos/{repo_id}/items/{fqn_b64}/callees
```

`repo_id` — workspace identifier (matches `X-Workspace-Id` in legacy endpoints).  
`fqn_b64` — URL-safe base64-encoded fully-qualified name (no padding).

#### Query Parameters

| Parameter | Type   | Default | Max | Description                            |
|-----------|--------|---------|-----|----------------------------------------|
| `depth`   | uint   | 3       | 10  | BFS traversal depth                    |
| `limit`   | uint   | 50      | 200 | Max edges per page                     |
| `cursor`  | string | —       | —   | Opaque continuation cursor             |

#### Response Shape

```json
{
  "root": {
    "fqn": "crate::module::my_fn",
    "name": "my_fn",
    "kind": "Function",
    "file_path": "src/module.rs",
    "line": 42
  },
  "nodes": [
    { "fqn": "...", "name": "...", "kind": "...", "file_path": "...", "line": null }
  ],
  "edges": [
    {
      "from_fqn": "caller::fn",
      "to_fqn": "crate::module::my_fn",
      "depth": 1,
      "provenance": "direct"
    }
  ],
  "cycles_detected": false,
  "next_cursor": "eyJvZmZzZXQiOjUwfQ"
}
```

#### Edge Provenance

| Value           | Meaning                                                      |
|-----------------|--------------------------------------------------------------|
| `direct`        | Static call via `CALLS` edge; no type specialisation         |
| `monomorph`     | `CALL_INSTANTIATES` edge, or `CALLS` with concrete type args |
| `dyn_candidate` | `CALLS` edge with `dispatch = "dynamic"` property            |

#### BFS Algorithm

1. Start frontier = {`root_fqn`}; visited = {`root_fqn`}
2. For each depth level:
   - Query Neo4j for immediate neighbors via `CALLS` and `CALL_INSTANTIATES` edges
   - Skip nodes already in visited (record `cycles_detected = true`)
   - Add new nodes to visited and the next frontier
3. Collect (edge, provenance) pairs in BFS order
4. Apply cursor offset + limit to produce the page
5. Emit `next_cursor` when more edges remain

---

## Implementation

- **Library crate:** `crates/rb-query` — `CallGraphTraverser`, `TraversalOptions`,
  `TraversalResult`, `EdgeProvenance`, cursor encode/decode
- **Handler module:** `services/api/src/handlers/repos.rs` — Axum handlers, base64 FQN
  decoding, parameter validation
- **Route wiring:** `services/api/src/main.rs`

---

## Consequences

- Adds two new endpoints under the `/v1/repos/` namespace (stable, repo-scoped).
- `CALL_INSTANTIATES` edges are queried but currently return 0 rows; provenance for
  monomorphized calls is derived from `CALLS.concrete_types` until ingestion emits
  `CALL_INSTANTIATES`.
- Cursor encodes a simple offset; ordering is BFS insertion order (deterministic for
  a given graph state).
- Multi-tenancy guaranteed: workspace label injected into every Cypher pattern.
