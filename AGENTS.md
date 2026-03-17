# AGENTS.md - Unified Workspace Rules

This file defines the shared workspace rules for both **OpenClaw** and **OpenCode** agent systems in the `rust-brain` project.

## 🧠 RustBrain MCP Tools - MANDATORY FIRST

This workspace provides deep code intelligence via the `rustbrain` MCP server. **You MUST use these tools before attempting standard file reading or grep-based searches.**

### Tool Priority & Usage

1.  **`search_code(query)`**: Semantic search. Use this for "What is X?" or "How do I find X?".
2.  **`get_function(fqn)`**: Detailed item info. Use this once you have a Fully Qualified Name (FQN).
3.  **`get_callers(fqn, depth?)`**: Call graph traversal. Use this to understand impact or tracing.
4.  **`get_trait_impls(trait_name)`**: Polymorphism resolution. Use this to find handlers/implementations.
5.  **`find_type_usages(type_name)`**: Type reach analysis. Use this to see where a struct/enum is used.
6.  **`get_module_tree(crate_name)`**: Architecture overview. Use this to understand crate structure.
7.  **`query_graph(query)`**: Advanced exploration. Use Cypher for complex relationship queries.

**Workflow Example:**
`search_code("ingestion pipeline")` → `get_function("crate::pipeline::run")` → `get_callers("crate::pipeline::run")`

## 📂 System-Specific Configuration

While this file governs shared workspace behavior, each agent system maintains its own specific identity, memory, and specialized instructions:

-   **OpenClaw**: Config and soul in `agents/openclaw/`
-   **OpenCode**: Config and soul in `agents/opencode/`

Refer to the `AGENTS.md` file within those subdirectories for system-specific soul, memory, and heartbeat rules.

## 📜 Shared Principles

-   **Atomic Execution**: Perform one logical task at a time. Verify success before proceeding.
-   **Documentation**: Record significant architectural decisions in `.sisyphus/notepads/`.
-   **Safety**: Never run destructive commands (`rm -rf`, `git push --force`) without explicit user confirmation.
-   **Memory**: If it's worth remembering, write it to a file. Mental notes do not persist.

## 🛠️ Environment Context

-   **Project Root**: `/home/jarnura/projects/rust-brain/`
-   **Infrastructure**: Managed via Docker Compose (Neo4j, Qdrant, Postgres, Ollama).
-   **Tool API**: Agent-facing endpoints are available at `http://localhost:8088`.
