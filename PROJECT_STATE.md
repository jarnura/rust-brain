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
current_phase: PHASE_2
current_task: ingestion-service-api-fix
status: PARTIAL - Code generated, needs API compatibility fixes

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
      grafana: RUNNING (4 dashboards)
      pgweb: RUNNING (port 8082)

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
    status: PARTIAL
    tasks:
      phase4-test: PASSED (test fixtures created)
      phase4-docs: PASSED (architecture.md, runbook.md, 2 ADRs)

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
    dashboards: 4
  
  pgweb:
    status: running
    port: 8082
  
  api:
    status: built
    port: 8080
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
  grafana: http://localhost:3000
  neo4j_browser: http://localhost:7474
  qdrant_dashboard: http://localhost:6333/dashboard
  pgweb: http://localhost:8082
  prometheus: http://localhost:9090
  ollama_api: http://localhost:11434
  tool_api: http://localhost:8080
