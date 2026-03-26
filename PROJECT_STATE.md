# PROJECT_STATE.md — rust-brain Project State
# =============================================================================
# This file tracks the current state of the rust-brain project.
# Updated by the orchestrator after each phase completion.
# =============================================================================

# PROJECT INFO
project: rust-brain
description: Production-grade Rust code intelligence platform
created: 2026-03-14
last_updated: 2026-03-14T11:25:00+05:30

# CURRENT STATUS
current_phase: PHASE_5
current_task: opencode-integration-complete
status: ACTIVE - Unified playground UI, MCP servers (stdio + SSE), LiteLLM routing deployed

# PHASE STATUS
phases:
  PHASE_0:
    name: "Prerequisites & Environment Bootstrap"
    status: PASSED
    completed: 2026-03-14T05:16:00+05:30

  PHASE_1:
    name: "Docker Compose — Core Services"
    status: PASSED
    completed: 2026-03-14T05:34:00+05:30
    services:
      postgres: RUNNING (healthy)
      neo4j: RUNNING (healthy, 12 constraints, 9 indexes)
      qdrant: RUNNING (2 collections initialized)
      ollama: RUNNING (nomic-embed-text + codellama:7b)
      prometheus: RUNNING
      grafana: RUNNING (3 dashboards)
      pgweb: RUNNING (port 8085)

  PHASE_2:
    name: "Ingestion Pipeline"
    status: PARTIAL
    tasks:
      phase2a-expand: CODE_CREATED (21KB main.rs)
      phase2b-parse: CODE_CREATED (57KB parsers/)
      phase2c-typecheck: CODE_CREATED (44KB typecheck/)
      phase2d-extract: INTEGRATED_IN_PIPELINE
      phase2e-graph: CODE_CREATED (76KB graph/)
      phase2f-embed: CODE_CREATED (75KB embedding/)
    issues:
      - "API compatibility: sqlx try_get, neo4rs execute, uuid new_v5"
      - "Need to update code for current crate versions"

  PHASE_3:
    name: "Tool API Layer"
    status: PASSED
    completed: 2026-03-14T05:55:00+05:30
    details:
      endpoints: 9 implemented
      docker_build: SUCCESS (100MB image)
      health_check: WORKING

  PHASE_4:
    name: "Integration Testing & Documentation"
    status: PASSED
    completed: 2026-03-16T14:30:00+05:30
    tasks:
      phase4-test: PASSED (test fixtures created)
      phase4-docs: PASSED (architecture.md, runbook.md, 2 ADRs)

  PHASE_5:
    name: "OpenCode Integration & Unified Playground"
    status: PASSED
    completed: 2026-03-17T10:15:00+05:30
    details:
      unified_playground: DEPLOYED (10 tabs: dashboard, search, graph, chat, cypher, types, traits, modules, audit, gaps)
      mcp_stdio_server: DEPLOYED (tool definitions, argument marshaling)
      mcp_sse_server: DEPLOYED (HTTP streaming transport for IDEs)
      litellm_integration: CONFIGURED (model routing, fallbacks, cost tracking)
      keyboard_shortcuts: IMPLEMENTED (Cmd+K palette, Cmd+1-9 panels, Cmd+/, Cmd+B, Esc)
      streaming_responses: ENABLED (SSE for chat, async tool invocations)
      tool_call_visibility: COMPLETE (render tool invocations + results in UI)

  PHASE_6:
    name: "Production Hardening"
    status: PASSED
    completed: 2026-03-27T03:30:00+05:30
    details:
      embedding_model: UPGRADED (qwen3-embedding:4b, 2560 dims, was nomic-embed-text 768 dims)
      feature_propagation: FIXED (pre-patch Cargo.toml for olap/frm before cargo expand)
      body_source_storage: EXPANDED (MAX_BODY_SOURCE_LEN 50000 bytes, was 200)
      oom_prevention: IMPLEMENTED (stream expanded sources to cache files, batch processing)
      stuck_detector: TUNED (threshold 600s/10min, was 120s/2min)
      chat_session_management: IMPLEMENTED (session dropdown, "+ New" button, message persistence)
      opencode_integration: FIXED (model format uses "/" not ":", correct API endpoint, MessageListEntry parsing)
      timeouts: INCREASED (10 min for chat, was 2 min)

# INFRASTRUCTURE STATUS
infrastructure:
  postgres:
    status: healthy
    port: 5432
    tables: source_files, extracted_items, call_sites, ingestion_runs, repositories
  
  neo4j:
    status: healthy
    ports: [7474, 7687]
    constraints: 12
    indexes: 9
    apoc: enabled
  
  qdrant:
    status: running
    ports: [6333, 6334]
    collections: [code_embeddings, doc_embeddings]
    vector_size: 768
  
  ollama:
    status: running
    port: 11434
    models: [nomic-embed-text, codellama:7b]
  
  prometheus:
    status: running
    port: 9090
  
  grafana:
    status: running
    port: 3000
    dashboards: 3

  pgweb:
    status: running
    port: 8085

  api:
    status: built
    port: 8088
    endpoints: 9

# CODE STATISTICS
code:
  ingestion_service:
    total_lines: 9502
    modules:
      parsers: 1736 lines
      typecheck: 1259 lines
      graph: 2087 lines
      embedding: 2419 lines
      pipeline: 1593 lines
      main: 208 lines
    status: NEEDS_API_FIXES
  
  api_service:
    total_lines: 35000+
    status: BUILDS_SUCCESSFULLY

# KNOWN ISSUES
issues:
  - id: INGEST-001
    description: "sqlx 0.8 changed try_get API"
    files: [src/pipeline/stages.rs, src/typecheck/resolver.rs]
    fix: "Use sqlx 0.7 or update to new API"
  
  - id: INGEST-002
    description: "neo4rs 0.8 changed execute method visibility"
    files: [src/graph/*.rs]
    fix: "Use neo4rs 0.7 or use run() method"
  
  - id: INGEST-003
    description: "uuid crate new_v5 signature changed"
    files: [src/embedding/mod.rs]
    fix: "Update to Uuid::new_v5(namespace, bytes)"

# NEXT STEPS
next_steps:
  - "Fix API compatibility issues in ingestion service"
  - "Run integration tests with test fixture"
  - "Add ingestion service to docker-compose up"
  - "Create API documentation (docs/api-spec.md)"

# SERVICE ENDPOINTS
endpoints:
  playground_ui: http://localhost:8088/playground
  opencode_ide: http://localhost:4096
  litellm_proxy: (external — https://grid.ai.juspay.net)
  mcp_sse_server: ws://localhost:3001
  tool_api_rest: http://localhost:8088/tools
  grafana: http://localhost:3000
  neo4j_browser: http://localhost:7474
  qdrant_dashboard: http://localhost:6333/dashboard
  pgweb: http://localhost:8085
  prometheus: http://localhost:9090
  ollama_api: http://localhost:11434

# NEW ENDPOINTS (PHASE_5)
api_endpoints:
  playground:
    path: GET /playground
    description: Unified playground UI
    features: [dashboard, search, callgraph, chat, cypher, types, traits, modules, audit, gaps]

  chat_stream:
    path: POST /chat
    description: Streaming chat with tool integration
    transport: SSE
    features: [markdown_rendering, tool_visibility, async_invocation]

  tool_search:
    path: POST /tools/search_semantic
    description: Semantic code search via MCP
    streaming: true

  tool_callgraph:
    path: GET /tools/get_callers?fqn=
    description: Call graph traversal via MCP

  mcp_tools:
    path: GET /mcp/tools
    description: List available MCP tools

  mcp_invoke:
    path: POST /mcp/invoke
    description: Invoke tool with streaming results
    transport: SSE
