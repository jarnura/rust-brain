# Threat Model: query_graph Workspace Label Injection

**Issue**: RUSA-198
**Status**: In Progress
**Date**: 2026-04-17
**Author**: API Engineer

## Overview

The `POST /tools/query_graph` endpoint is the only public raw-Cypher surface in rust-brain. It accepts user-supplied Cypher queries and executes them against a shared Neo4j Community instance. Without workspace isolation, any user can access all data across all workspaces.

This document names each attack class against the workspace label injection mechanism and describes how the implementation defends it.

## Trust Boundary

```
User Request (untrusted)
    │
    ▼
┌──────────────────────────────────┐
│  X-Workspace-Id header           │  ← Extracted from HTTP headers
│  (validated: 12-char hex)        │
├──────────────────────────────────┤
│  workspace_label.rs              │  ← Injection + validation layer
│  - validate_workspace_id()       │
│  - strip_comments()              │
│  - validate_single_statement()   │
│  - validate_no_planner_reentry() │
│  - inject_labels_into_patterns() │
├──────────────────────────────────┤
│  graph.rs validate_cypher()      │  ← Existing read-only check
├──────────────────────────────────┤
│  Neo4j Bolt execution            │  ← With workspace label enforced
└──────────────────────────────────┘
```

## Attack Classes

### 1. Comment Injection

**Attack**: Embed malicious Cypher inside comments to hide it from naive string scanners.
```
MATCH (n) /* WHERE n:Workspace_other */ RETURN n
MATCH (n) // bypass label check
```

**Defense**: `strip_comments()` removes all `//` and `/* */` comments (including nested) BEFORE label injection. The injection operates on the stripped query. String literals are preserved — `//` inside quotes is not stripped.

**Residual risk**: Edge cases in string-literal detection. Mitigated by tests covering string-embedded comment sequences.

### 2. Label Literal Collision

**Attack**: User includes a string literal that resembles a workspace label, hoping to confuse the parser.
```
MATCH (n {name: ":Workspace_other"}) RETURN n
```

**Defense**: The injection parser only modifies node patterns `(var:Label...)`, not property maps `{key: "value"}`. String literal content inside quotes is never treated as a label. The parser tracks quote state to distinguish between label syntax and string values.

**Residual risk**: Minimal — Cypher syntax makes the distinction between `:Label` (outside quotes, after `(`) and `":value"` (inside quotes) unambiguous.

### 3. Multi-Statement Queries

**Attack**: Use semicolons to chain an unfiltered second query after a filtered first.
```
MATCH (n:Workspace_self) RETURN n; MATCH (m) RETURN m
```

**Defense**: `validate_single_statement()` rejects any query containing `;` after comment stripping. This runs before label injection.

**Residual risk**: None — Cypher requires `;` for multi-statement execution. Rejecting all semicolons is a complete defense.

### 4. WITH/UNION Smuggling

**Attack**: Use `WITH` or `UNION` to create a second query scope that might not receive the workspace label.
```
MATCH (n:Workspace_self) WITH n MATCH (m) RETURN m
MATCH (n) RETURN n UNION MATCH (m) RETURN m
```

**Defense**: `inject_labels_into_patterns()` scans the ENTIRE query for ALL `MATCH` clauses, including those after `WITH` and in each `UNION`/`UNION ALL` branch. Every node pattern gets the workspace label regardless of position in the query.

**Residual risk**: Complex nested CTEs or subqueries might be missed by a regex-based parser. Mitigated by tests covering WITH, UNION, and UNION ALL patterns.

### 5. Variable-Rebinding

**Attack**: Use `WITH ... AS` to rebind a filtered variable to a new name, then use the new name in an unfiltered context.
```
MATCH (n:Workspace_self) WITH n AS x MATCH (x)-[:CALLS]->(y) RETURN y
```

**Defense**: The injection applies to ALL node patterns in ALL MATCH clauses, regardless of variable names. Even if `n` is filtered and rebound to `x`, the second MATCH still gets `:Workspace_<id>` injected into its node patterns. The `y` node also gets the label.

**Residual risk**: Low — injection is positional, not variable-based. Every node pattern is labeled regardless of how variables flow.

### 6. Unicode + Whitespace Tricks

**Attack**: Use zero-width characters, non-breaking spaces, or other Unicode tricks to create identifiers that look like valid labels but bypass string comparison.
```
MATCH (n:Fun\u0000ction) RETURN n
MATCH (n:Function\u00A0:Workspace_other) RETURN n
```

**Defense**: `validate_workspace_id()` enforces `^[0-9a-f]{12}$` — only lowercase hex is accepted. The injection parser rejects non-ASCII characters in Cypher identifiers. Zero-width characters are stripped or cause rejection.

**Residual risk**: Some Unicode normalization attacks might not be caught. Mitigated by workspace_id validation (the only user-controlled string that becomes a label is the hex ID from the header).

### 7. APOC Procedure Calls (Planner Reentry)

**Attack**: Use APOC procedures that accept and execute arbitrary Cypher, bypassing the label injection.
```
CALL apoc.cypher.run('MATCH (n) RETURN n', {})
CALL apoc.cypher.runMany('MATCH (n) RETURN n;')
CALL apoc.do.when(true, 'MATCH (n) RETURN n', '')
```

**Defense**: Three layers of defense:

1. **API layer**: `validate_no_planner_reentry()` in `workspace_label.rs` rejects queries containing `apoc.cypher.run`, `apoc.cypher.runMany`, `apoc.cypher.runFile`, `apoc.do.when`, `apoc.do.case`, `apoc.when`, `apoc.case`, `apoc.periodic.commit`, `apoc.periodic.iterate`, and `apoc.trigger.*`. Case-insensitive.

2. **graph.rs**: `APOC_PLANNER_REENTRY_PREFIXES` constant in `validate_cypher()` provides a second check. This catches the same procedures even if workspace_label.rs is somehow bypassed.

3. **Neo4j config**: `dbms.security.procedures.unrestricted` in `neo4j.conf` is narrowed from `apoc.*` to an explicit allowlist of read-only namespaces. Planner-reentry procedures are NOT on the list and will be rejected by Neo4j itself.

**Residual risk**: If a new APOC version introduces a planner-reentry procedure with an unexpected name, it could bypass the API-layer checks. The Neo4j config allowlist provides the final defense — only explicitly listed procedure namespaces can execute.

**Confirmed denied at Neo4j config**: `apoc.cypher.*`, `apoc.do.*`, `apoc.when`, `apoc.case`, `apoc.periodic.*`, `apoc.trigger.*`

### 8. Parameter Injection

**Attack**: User supplies a parameter named `workspace_label` or `workspace_id`, hoping it overrides the server-injected label.
```
{"query": "MATCH (n:Workspace_$workspace_label) RETURN n", "parameters": {"workspace_label": "Workspace_evil"}}
```

**Defense**: Two layers:

1. The injection uses `format!()` to bake the validated workspace label directly into the Cypher string — it never uses `$param` binding for the workspace label. User-supplied parameters cannot affect the injected label.

2. The `query_graph` handler strips `workspace_label` and `workspace_id` keys from `req.parameters` before passing them to Neo4j. This is defense-in-depth.

**Residual risk**: Minimal — the workspace label is never parameterized, so there is no injection vector through parameters.

### 9. Deep Nesting / Variable-Length Paths

**Attack**: Use complex path patterns hoping the label injection misses some node patterns.
```
MATCH ((n)-[*1..5]-(m)) RETURN n, m
MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) RETURN a, b, c
```

**Defense**: The injection parser finds ALL node patterns `(...)` that are not relationship patterns `[...]`. Path patterns are decomposed — each node variable gets the workspace label. Anonymous nodes `()` become `(:Workspace_<id>)`.

**Residual risk**: Deeply nested parenthetical expressions or unusual Cypher syntax might confuse the parser. Mitigated by tests covering variable-length paths, multi-hop patterns, and nested parentheses.

### 10. Empty / Malformed Input

**Attack**: Submit empty strings, partial keywords, or malformed queries to crash the injector or cause it to pass through unmodified.
```
""  ;  "MATCH"  "MATCH (n) RETURN"
```

**Defense**: Each stage validates its input:
- Empty query → rejected by `inject_workspace_label()` before any processing
- Semicolons → rejected by `validate_single_statement()`
- Partial keywords → if they contain no node patterns, no injection occurs (safe passthrough for queries like `RETURN 1`)
- Queries with node patterns but no RETURN → Neo4j will reject at execution time

**Residual risk**: Very low — malformed queries either get rejected early or result in no-op injection (which is safe — no data accessed means no leak).

## Additional Defenses

### Direct Workspace Label Injection by User

**Attack**: User tries to set their own workspace label in the query.
```
MATCH (n:Workspace_attacker) RETURN n
```

**Defense**: `inject_labels_into_patterns()` scans for any existing `:Workspace_` prefix in the query. If found, the query is REJECTED — the user is not allowed to specify workspace labels. The server is the sole authority on workspace context.

### Label Enumeration

**Attack**: User queries `labels(n)` or `:Workspace_*` to discover all workspace identifiers.
```
MATCH (n) RETURN labels(n)
MATCH (n:Workspace_*) RETURN n
```

**Defense**: After injection, `MATCH (n)` becomes `MATCH (n:Workspace_<id>)`, so only the caller's workspace nodes are returned. `labels(n)` will only see labels on workspace-scoped nodes. `Workspace_*` is not valid Cypher syntax (Neo4j doesn't support wildcard labels).

### WHERE Label Filter Bypass

**Attack**: User adds `WHERE n:Workspace_other` to bypass the injected label.
```
MATCH (n:Workspace_self) WHERE n:Workspace_other RETURN n
```

**Defense**: This is a logical contradiction — the MATCH requires `:Workspace_self` but the WHERE requires `:Workspace_other`. Neo4j will return zero results. No data leaks because the MATCH pattern already constrains the result set.

## Defense-in-Depth Summary

| Layer | Mechanism | What It Catches |
|-------|-----------|----------------|
| 1. Header validation | `validate_workspace_id()` | Invalid/malicious workspace IDs |
| 2. Comment stripping | `strip_comments()` | Hidden Cypher in comments |
| 3. Statement validation | `validate_single_statement()` | Multi-statement injection |
| 4. APOC blocking (parser) | `validate_no_planner_reentry()` | Planner-reentry procedures |
| 5. Label injection | `inject_labels_into_patterns()` | All node patterns get workspace filter |
| 6. APOC blocking (validator) | `validate_cypher()` + `APOC_PLANNER_REENTRY_PREFIXES` | Second line of defense for APOC |
| 7. Write blocking | `validate_cypher()` + `CYPHER_WRITE_TOKENS` | Write operations |
| 8. Neo4j config | `procedures.unrestricted` allowlist | Blocks unapproved procedures at DB level |
| 9. Parameter stripping | Handler removes `workspace_*` from params | Parameter injection |

## Re-escalation Criteria

Per CEO direction: if any test in the malformed-input suite fails or exposes an attack class the implementation cannot defend, **stop and report immediately**. Do not ship a partial defense.

## Related

- [ADR-005: Multi-Tenancy Physical Isolation](../adr/ADR-005-multi-tenancy-physical-isolation.md) — Phase 3, Option B+
- [RUSA-180](/RUSA/issues/RUSA-180) — parent task
- [RUSA-199](/RUSA/issues/RUSA-199) — QA Lead's cross-workspace leak tests (seeds from this test suite)
