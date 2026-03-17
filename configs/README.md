# Configuration Files

This directory contains configuration for all services in the rust-brain stack.

## Directory Structure

```
configs/
├── opencode/          # OpenCode AI coding assistant
│   ├── Dockerfile     # Builds the OpenCode server image
│   └── config.json    # OpenCode settings: MCP, provider, model, tools
├── litellm/           # LiteLLM proxy (unified LLM gateway)
│   ├── Dockerfile     # Builds the LiteLLM proxy image
│   └── config.yaml    # Model list and general settings
├── grafana/           # Grafana dashboards and datasources
├── neo4j/             # Neo4j graph database config
├── prometheus/        # Prometheus scrape targets
└── qdrant/            # Qdrant vector store config
```

## OpenCode (`configs/opencode/`)

OpenCode is an AI coding assistant served over HTTP.

**`config.json`** key settings:
- **MCP**: Connects to the `rustbrain` MCP server at `http://mcp-sse:3001/sse` for Rust code intelligence tools (`search_code`, `get_function`, `get_callers`, `get_trait_impls`, `find_type_usages`, `get_module_tree`, `query_graph`).
- **Provider**: Uses LiteLLM as an OpenAI-compatible endpoint at `http://litellm:4000`. Auth via `LITELLM_MASTER_KEY`.
- **Model**: `anthropic/claude-sonnet-4-20250514`
- **Tools**: `write` and `bash` are disabled — read-only mode for safety.

**Port**: `4096`

## LiteLLM (`configs/litellm/`)

LiteLLM is a unified proxy that exposes multiple LLM providers through a single OpenAI-compatible API.

**`config.yaml`** model list:
| Model name | Backend | Auth |
|---|---|---|
| `anthropic/claude-sonnet-4-20250514` | Anthropic API | `ANTHROPIC_API_KEY` |
| `ollama/codellama:7b` | Ollama at `http://ollama:11434` | none |
| `openai/gpt-4o` | OpenAI API | `OPENAI_API_KEY` |

**`general_settings.master_key`**: Set via `LITELLM_MASTER_KEY` env var. Used by OpenCode (and any other client) to authenticate against the proxy.

**Port**: `4000`

## Required Environment Variables

| Variable | Used by | Description |
|---|---|---|
| `ANTHROPIC_API_KEY` | LiteLLM | Anthropic API key |
| `OPENAI_API_KEY` | LiteLLM | OpenAI API key (optional) |
| `LITELLM_MASTER_KEY` | LiteLLM, OpenCode | Shared secret for proxy auth |
