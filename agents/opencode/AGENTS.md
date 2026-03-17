# AGENTS.md - OpenCode Configuration

This is the central configuration for OpenCode agents in the `rust-brain` project.

## 🧠 RustBrain MCP Tools - PRIORITY 1

**CRITICAL: Always use `rust-brain_*` MCP tools FIRST for any codebase exploration.**

This workspace has access to the `rust-brain` MCP server. These tools query a Neo4j knowledge graph and Qdrant vector database for semantic understanding.

### Tool Priority

1. **`rust-brain_search_code(query, ...)`** - Semantic code search (Natural language or code fragments)
2. **`rust-brain_get_function(fqn)`** - Get detailed info, signature, docs, callers, and callees
3. **`rust-brain_get_callers(fqn, depth?)`** - Trace execution paths (up to 5 levels)
4. **`rust-brain_get_trait_impls(trait_name, ...)`** - Find all implementations of a trait
5. **`rust-brain_find_type_usages(type_name, ...)`** - Find all places where a type is used
6. **`rust-brain_get_module_tree(crate_name)`** - Understand hierarchical module structure
7. **`rust-brain_query_graph(query, ...)`** - Advanced custom Cypher queries

### When to Use Each Tool

| Goal | Tool |
|------|------|
| "What is X?" | `rust-brain_search_code("X")` → `rust-brain_get_function(fqn)` |
| "How does X work?" | `rust-brain_search_code("X")` → `rust-brain_get_function` → `rust-brain_get_callers` |
| "Who calls X?" | `rust-brain_get_callers("crate::module::X")` |
| "What implements trait T?" | `rust-brain_get_trait_impls("T")` |
| "Where is type T used?" | `rust-brain_find_type_usages("T")` |
| "Show me crate structure" | `rust-brain_get_module_tree("crate_name")` |

**Do NOT use file reading or grep until you have exhausted these semantic tools.**

## 🛠 Available Skills

OpenCode agents have access to specialized skills. Invoke them via `skill(name="...")` or `task(load_skills=["..."])`.

- **/playwright** - MUST USE for any browser-related tasks (verification, scraping, testing).
- **/git-master** - MUST USE for ANY git operations (atomic commits, rebase, history search).
- **/refactor** - Intelligent refactoring with LSP, AST-grep, and TDD verification.
- **/dev-browser** - Browser automation with persistent state for multi-step workflows.

## 📋 Workspace Rules

Refer to the shared `AGENTS.md` at the project root for general workspace rules, memory management, and behavioral guidelines.

### OpenCode Conventions
- **Atomic Execution**: Focus on ONE specific task at a time.
- **Verification**: No task is complete without `lsp_diagnostics` and build verification.
- **Notepad**: Append learnings to `.sisyphus/notepads/{plan-name}/learnings.md`.
- **Plan Integrity**: NEVER modify the священный (sacred) plan file in `.sisyphus/plans/`.
