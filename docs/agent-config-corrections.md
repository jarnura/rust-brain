# Agent Prompt Corrections Spec

When transforming the raw agent prompt designs into corrected `.opencode/agents/*.md` files,
apply ALL of the following systematic corrections.

---

## 1. PostgreSQL Table Mappings

All agent prompts reference a `symbols` table that does not exist. The actual table is `extracted_items`.

| Raw Prompt Says | Correct To | Notes |
|---|---|---|
| `FROM symbols` | `FROM extracted_items` | Table name |
| `symbols.name` | `extracted_items.name` (for display) or `extracted_items.fqn` (for lookups) | `fqn` is UNIQUE and the primary lookup key; `name` is the short name |
| `symbols.kind` | `extracted_items.item_type` | Column rename |
| `symbols.file_path` | Join: `source_files.file_path` via `extracted_items.source_file_id` | Not a direct column — requires JOIN |
| `symbols.line_start` | `extracted_items.start_line` | Column rename |
| `symbols.line_end` | `extracted_items.end_line` | Column rename |
| `symbols.visibility` | `extracted_items.visibility` | Same name, works as-is |
| `symbols.signature` | `extracted_items.signature` | Same name, works as-is |
| `symbols.module_path` | Derive from `fqn` (split by `::`) or join `source_files.module_path` | No direct column |
| `symbols.doc_comment` | `extracted_items.doc_comment` | Same name, works as-is |
| `symbols.parent_impl` | Not available as column — derive from `fqn` pattern (`Type::method`) | No direct column |
| `symbols.crate_name` | Derive from `fqn` (first segment) or join `source_files.crate_name` | No direct column on extracted_items |

### Corrected SQL Example (Explorer symbol lookup)

**Raw prompt says:**
```sql
SELECT * FROM symbols WHERE name = $1;
```

**Correct to:**
```sql
SELECT ei.fqn, ei.name, ei.item_type, ei.visibility, ei.signature,
       ei.doc_comment, ei.start_line, ei.end_line,
       sf.file_path, sf.crate_name, sf.module_path
FROM extracted_items ei
JOIN source_files sf ON ei.source_file_id = sf.id
WHERE ei.name = $1;
```

### Corrected SQL Example (fuzzy lookup)

**Raw:**
```sql
SELECT * FROM symbols WHERE name ILIKE '%' || $1 || '%' ORDER BY length(name) ASC LIMIT 10;
```

**Correct to:**
```sql
SELECT ei.fqn, ei.name, ei.item_type, ei.visibility, ei.signature,
       sf.file_path, sf.crate_name
FROM extracted_items ei
JOIN source_files sf ON ei.source_file_id = sf.id
WHERE ei.name ILIKE '%' || $1 || '%'
ORDER BY length(ei.name) ASC LIMIT 10;
```

### Corrected SQL Example (file inventory)

**Raw:**
```sql
SELECT name, kind, line_start, visibility FROM symbols WHERE file_path = $1 ORDER BY line_start;
```

**Correct to:**
```sql
SELECT ei.name, ei.item_type, ei.start_line, ei.visibility
FROM extracted_items ei
JOIN source_files sf ON ei.source_file_id = sf.id
WHERE sf.file_path = $1
ORDER BY ei.start_line;
```

---

## 2. Neo4j Relationship Mappings

| Raw Prompt Says | Actual Relationship | Status |
|---|---|---|
| `CALLS` | `CALLS` | EXISTS (227K edges in snapshot) |
| `IMPLEMENTS` | `IMPLEMENTS` | EXISTS |
| `USES_TYPE` | `USES_TYPE` | EXISTS (144K edges) |
| `CONTAINS` | `CONTAINS` | EXISTS |
| `IMPORTS` | `IMPORTS` | EXISTS |
| `RETURNS` | `RETURNS` | EXISTS |
| `HAS_FIELD` | `HAS_FIELD` | EXISTS (8.4K edges) |
| `HAS_VARIANT` | `HAS_VARIANT` | EXISTS (5K edges) |
| `FOR` / `FOR_TYPE` | `FOR` | EXISTS (40.5K edges) |
| `PRODUCES` | **Use `RETURNS`** | RENAME — `PRODUCES` doesn't exist |
| `CONSUMES` | **Use `ACCEPTS`** | RENAME — `CONSUMES` doesn't exist |
| `DEPENDS_ON` (Crate→Crate) | **Does not exist yet** | TO BE BUILT — note in prompt as "requires DEPENDS_ON relationship (Phase 0)" |
| `HAS_METHOD` (Trait→Function) | **Does not exist yet** | TO BE BUILT — note in prompt as "requires HAS_METHOD relationship (Phase 0)" |

### Neo4j Node Properties

Actual node labels in the graph: `Crate`, `Module`, `Function`, `Struct`, `Enum`, `Trait`, `Impl`, `TypeAlias`, `Const`, `Static`, `Macro`, `Type`

Key property: nodes use `fqn` as the unique identifier (not just `name`).
- Raw prompts often use `{name: $name}` — correct to `{name: $name}` (the `name` property exists on nodes) BUT note that `name` is not unique. For precise lookups, use `{fqn: $fqn}`.

---

## 3. Qdrant Collection Mappings

| Raw Prompt Says | Actual Collection | Status |
|---|---|---|
| `collection: "code_embeddings"` | `code_embeddings` | EXISTS (219K vectors, 2560-dim) |
| `collection: "docs"` | `doc_embeddings` | EXISTS but with different name — RENAME in prompts |
| `collection: "crate_docs"` | **Does not exist** | TO BE BUILT — note as "requires crate_docs collection (Phase 0)" |
| `collection: "external_docs"` | **Does not exist** | TO BE BUILT — note as "requires external_docs collection (Phase 0)" |
| `collection: "blog_posts"` | **Does not exist** | DEFER — Phase 6 only, note as unavailable |
| `collection: "error_history"` | **Does not exist** | DEFER — Phase 4 only, note as unavailable |

---

## 4. Tool Name Mappings (Conceptual → OpenCode MCP)

The raw prompts reference tools by conceptual names. In OpenCode, MCP tools are prefixed with `mcp_rustbrain_`.

### Tools that map to existing MCP tools:

| Raw Prompt Tool | OpenCode MCP Tool | Notes |
|---|---|---|
| `qdrant_search(collection: "code_embeddings")` | `mcp_rustbrain_search_code` | High-level semantic search |
| `get_function` | `mcp_rustbrain_get_function` | Direct match |
| `get_callers` | `mcp_rustbrain_get_callers` | Direct match |
| `get_trait_impls` | `mcp_rustbrain_get_trait_impls` | Direct match |
| `find_type_usages` | `mcp_rustbrain_find_type_usages` | Direct match |
| `get_module_tree` | `mcp_rustbrain_get_module_tree` | Direct match |
| `query_graph` / `neo4j_query` (Cypher) | `mcp_rustbrain_query_graph` | Read-only Cypher |
| `find_calls_with_type` | `mcp_rustbrain_find_calls_with_type` | Direct match |
| `find_trait_impls_for_type` | `mcp_rustbrain_find_trait_impls_for_type` | Direct match |

### Tools that map to NEW MCP tools (to be built):

| Raw Prompt Tool | OpenCode MCP Tool | Status |
|---|---|---|
| `pg_query` (raw SQL) | `mcp_rustbrain_pg_query` | TO BE BUILT |
| `context_store` (artifact CRUD) | `mcp_rustbrain_context_store` | TO BE BUILT |
| `status_check` (task status) | `mcp_rustbrain_status_check` | TO BE BUILT |
| `task_update` (task state transitions) | `mcp_rustbrain_task_update` | TO BE BUILT |

### Tools that map to OpenCode native capabilities (NOT MCP):

| Raw Prompt Tool | OpenCode Equivalent | Permission Needed |
|---|---|---|
| `read_file(path, start, end)` | `bash: "cat"`, `"head"`, `"tail"` | `"cat *": "allow"` |
| `write_file(path, content)` | OpenCode `edit` capability | `"edit": "allow"` |
| `grep_codebase(pattern, glob)` | `bash: "grep"`, `"rg"` | `"grep *": "allow"`, `"rg *": "allow"` |
| `cargo_check(args)` | `bash: "cargo check"` | `"cargo check*": "allow"` |
| `cargo_clippy(args)` | `bash: "cargo clippy"` | `"cargo clippy*": "allow"` |
| `cargo_test(args)` | `bash: "cargo test"` | `"cargo test*": "allow"` |
| `cargo_build(args)` | `bash: "cargo build"` | `"cargo build*": "allow"` |
| `git_commit(msg, files)` | `bash: "git commit"` | `"git commit*": "allow"` |
| `git_tag(name)` | `bash: "git tag"` | `"git tag*": "allow"` |
| `git_push(branch, tags)` | `bash: "git push"` | `"git push*": "allow"` |
| `web_fetch(url)` | OpenCode `webfetch` | `"webfetch": "allow"` |
| `gitbook_search(query)` | Use `mcp_rustbrain_search_code` on `doc_embeddings` collection | Semantic search fallback |

---

## 5. ISSUE-001 Updates

ISSUE-001 (TypecheckStage skipped) is **CLOSED** as of commit `316cb7d` (2026-03-29).

### Explorer prompt corrections:
- The "Handling the impl block bug" section should be updated to note the fix is deployed
- call_sites: 99,654 rows populated (was 0)
- trait_implementations: 29,738 rows populated (was 0)
- The grep fallback protocol can remain as a safety net but framed as "edge case fallback" not "known systemic bug"
- Remove/soften the "Known data quality issues" warning about impl block functions

### Debugger prompt corrections:
- The "Known data quality issues" note about TypecheckStage should be updated
- Neo4j CALLS relationships are now populated (227K edges in snapshot)

---

## 6. Context Window Notes

The raw prompts include context window budget tables. These should be preserved but updated to note:
- The model is `glm-latest` via LiteLLM proxy with 128K context / 16K output
- Adjust token budgets if they assume a different context size

---

## 7. Artifact Table References

Several prompts include SQL queries against the `artifacts` table for caching (Research agent checks for existing briefs, Explorer checks for existing CodeMaps). These queries are correct as designed — the `artifacts` table will be created as part of Phase 0 infrastructure.

Similarly, prompts that reference `tasks` table operations are correct — this table will be created alongside `artifacts`.

---

## 8. Tool Section Rewrite Pattern

Each prompt has a "Tool access" or "Tool hierarchy" section. These need to be rewritten to:

1. **Replace conceptual tool names** with actual MCP tool names or OpenCode bash commands
2. **Update SQL examples** per the table mapping above
3. **Update Cypher examples** per the relationship mapping above
4. **Add a note** at the top of the tools section:

```
NOTE: MCP tools are accessed via the rust-brain MCP server (prefixed mcp_rustbrain_*).
Filesystem, compiler, and git operations use OpenCode bash permissions.
Tool budgets from the original design still apply.
```

---

## 9. Per-Agent Correction Summary

| Agent | Key Corrections Needed |
|---|---|
| Orchestrator | Tool names: status_check, context_store, task_update. No SQL/Cypher examples to fix. |
| Research | `qdrant_search` collections: "docs" → "doc_embeddings". `gitbook_search` → use search_code on doc_embeddings. pg_query SQL: symbols → extracted_items. |
| Explorer | Most corrections needed. All SQL examples, Neo4j queries (PRODUCES→RETURNS, CONSUMES→ACCEPTS), ISSUE-001 update, tool mapping for read_file/grep_codebase → bash. |
| Planner | SQL: symbols → extracted_items. Neo4j: DEPENDS_ON (note as pending). read_file unavailable (Planner has no bash). |
| Developer | SQL: symbols → extracted_items. cargo_check/clippy → bash. read_file/write_file → bash/edit. |
| Debugger | SQL: symbols → extracted_items. Neo4j queries work as-is (CALLS exists). cargo_check/test → bash. ISSUE-001 update. |
| Reviewer | SQL: symbols → extracted_items. Neo4j HAS_METHOD → note as pending. read_file → bash. cargo_clippy → bash. |
| Testing | SQL: symbols → extracted_items. cargo_test/check → bash. read_file/write_file → bash/edit. |
| Deployment | SQL: symbols → extracted_items. Neo4j DEPENDS_ON → note as pending. cargo_build → bash. git_* → bash. |
| Documentation | SQL: symbols → extracted_items. read_file/write_file → bash/edit. cargo_check → bash. |
| Blog Writer | SQL: symbols → extracted_items. read_file → bash. Qdrant "blog_posts" → note as unavailable. |
| Demo Creator | SQL: symbols → extracted_items. read_file/write_file → bash/edit. cargo_check/run → bash. |
