# Changelog

All notable changes to the rust-brain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

## [0.4.0] - 2026-04-21

A major release focused on **multi-tenancy**, **SSE reliability**, and **agent trace observability**. This version introduces workspace-scoped data isolation across all three stores, cursor-based SSE reconnection with gap-free backfill, and a rich agent trace UI for monitoring multi-agent executions.

### Added

#### Multi-Tenancy & Workspace Isolation

- **Per-workspace Postgres schema isolation**: Ingestion pipeline creates and populates a dedicated schema per workspace (e.g., `ws_abc123456789`), keeping code items fully isolated
- **Per-workspace Qdrant collection lifecycle**: Each workspace gets its own Qdrant collection with independent search routing and collection creation/deletion
- **Per-workspace Neo4j graph partitioning**: Graph nodes and relationships carry a `workspace_id` label, enabling label-based isolation queries and per-workspace graph traversal
- `WorkspaceGraphClient` for Neo4j multi-tenancy with label injection on node writes (RUSA-194)
- Workspace label injection for user Cypher queries (RUSA-198)
- `GET /workspaces/:id/stats` endpoint returning per-workspace item counts, consistency status, and isolation verification (RUSA-215)
- Periodic workspace resource gauge collector for background metric collection (RUSA-213)
- Workspace-aware metrics middleware layer for request attribution (RUSA-214)
- FQN-based crate-origin filtering in ingestion pipeline to exclude dependency items from workspace scope (RUSA-235)
- Full workspace-aware pipeline for Postgres and Qdrant — ingestion, search, and queries are workspace-scoped (RUSA-238, RUSA-244)
- Apache AGE graph extension added to Postgres container for openCypher support (RUSA-192)
- AGE openCypher POC module with 10 ported queries (RUSA-193)
- Qdrant workspace migration script for existing deployments (RUSA-189)

#### Audit Service & Cross-Workspace Leak Detection

- `rustbrain-audit` service for continuous cross-workspace data leak detection (RUSA-197)
- Per-workspace audit metrics and Prometheus alerting rules (RUSA-214)
- Cross-workspace leak detection contract tests and enforcement scripts (RUSA-199, RUSA-202)
- Workspace Overview Grafana dashboard for resource monitoring (RUSA-216)

#### SSE Reliability & Cursor-Based Reconnection

- Cursor-based SSE reconnection with backfill for MCP and chat streams — clients reconnect without data loss (RUSA-252)
- Per-execution sequence numbering and cursor-based event storage in Postgres (RUSA-251)
- SSE reconnection UI with gap-free backfill and connection state indicator in frontend (RUSA-257)
- Opaque Unknown event persistence replacing silent MessagePart drops in SSE streaming (RUSA-249)

#### Agent Trace Frontend

- Typed event model and parser for agent trace data (RUSA-253)
- ToolCallCard component for paired tool call rendering with input/output display (RUSA-254)
- JSON syntax highlighting and copy-to-clipboard for tool call details (RUSA-255)
- Collapsible event groups with transcript navigation for execution playback (RUSA-256)
- Happy-path E2E integration test for agent trace rendering (RUSA-258)

#### Workspace Frontend Controls

- Workspace stats header with workspace switcher dropdown (RUSA-221)
- Archive/delete workspace controls with confirm-by-name safety dialog (RUSA-220)
- Workspace-scoped stats displayed on playground dashboard (RUSA-217)
- Workspace type alignment with backend — shows indexing progress in UI

#### Security Hardening

- Cypher template registry with `resolve_system` mode for safe predefined queries (RUSA-196)
- Hardened workspace label injection against security bypass patterns (RUSA-198)
- Closed `apoc.graph.fromCypher` bypass in validate_cypher and hardened comment handling (RUSA-198)
- Cardinality guard integration tests for query result bounding (Section 9)

#### Architecture Decision Records

- `docs/adr/ADR-005-multi-tenancy-physical-isolation.md` — multi-tenancy physical isolation architecture
- `docs/adr/ADR-006-neo4j-workspace-label-migration.md` — Neo4j workspace label migration strategy

#### Documentation Updates (2026-04-17)

- `docs/GAP_ANALYSIS.md` — Resolved Gap 5 (cross-DB aggregation) as RESOLVED. Updated architecture diagram (22→47+ endpoints, port 8080→8088, removed codellama, added qwen3-embedding:4b). Updated Gap 8 and Gap 10 to reflect partial progress. Revised phase recommendations to mark Phase 1 complete and Phase 2 mostly complete. Updated revision to v3.
- `docs/future-scope.md` — Added "Current State" sections to all 6 feature areas. Updated Section 6 (Web UI) to reflect existing Playground instead of aspirational UI. Added workspace isolation, MCP integration, OpenCode IDE, and snapshot distribution as completed work. Updated priority table with status column.
- `RELEASE_CHECKLIST.md` — Corrected endpoint counts: Code Intelligence 10→12, Chat 6→9, Workspace 5→9. Added missing sections: Execution (4 routes), Artifacts (4 routes), Tasks (4 routes), Validator/Benchmarker (5 routes), System (6 routes).
- AGENTS.md — Updated doc list from "20 documentation files" to "20+ documentation files". Added docs/adr/ ADR count (001-006), docs/agent-prompts/, docs/prompts/, docs/issues/, docs/handoff/, docs/screenshots/ directories.

#### Documentation: Workspace API & Editor Playground (RUSA-88)

- `docs/api-spec.md` — Complete rewrite of Workspace Management section with accurate request/response schemas, cURL examples, error codes, and data model tables for all 13 workspace/execution endpoints
- `docs/architecture.md` — Added Editor Playground section with service boundary diagram, data model tables, workspace lifecycle flow, multi-tenant schema isolation, Docker volume strategy, integration points, key design decisions, and failure modes
- `docs/getting-started.md` — Added Editor Playground setup section with prerequisites, quick workflow cURL walkthrough, execution lifecycle explanation, timeout configuration, and workspace isolation overview
- `README.md` — Added Workspace API (8 endpoints) and Execution (4 endpoints) to Agent Tool API table, added Editor Playground description with quick-start example

#### Previous Release Accumulations

##### Phase 3: Cross-Store Intelligence

- `GET /api/consistency` endpoint with summary and full detail modes
- `consistency_check` MCP tool for verifying data integrity across Postgres, Neo4j, and Qdrant
- Per-store item counts with discrepancy detection and recommendations
- ADR-004 documenting cross-store consistency design decisions
- `POST /tools/search_docs` endpoint for semantic documentation search
- `search_docs` MCP tool for natural language doc queries
- Doc embeddings collection (417 vectors across 30 documentation files)
- `scripts/embed_docs.py` for automated documentation vectorization
- Native OpenCode orchestrator dispatch replacing hardcoded 4-phase pipeline
- Dynamic agent timeline in playground showing real-time execution progress
- Session replay with historical tool invocations and code diffs
- ADR-003 documenting workspace isolation architecture
- `GET /health` returns per-store counts (Postgres items, Neo4j nodes/edges, Qdrant points)
- Dependency health status with latency metrics
- `GET /health/consistency` for quick consistency verification

##### Phase 2: Production Readiness

- GitHub Actions CI pipeline with cargo check, clippy, test, and build jobs
- 10 comprehensive E2E tests spanning all endpoint classes
- `KNOWN_ISSUES.md` — comprehensive documentation of all known limitations and failure modes
- `RELEASE_CHECKLIST.md` — systematic release verification protocol
- `docs/INGESTION_PERFORMANCE.md` — baseline performance metrics and bottleneck analysis
- `docs/COVERAGE_REPORT.md` — test coverage documentation
- `docs/adr/ADR-003-workspace-isolation.md` — workspace isolation architecture
- `docs/adr/ADR-004-cross-store-consistency.md` — cross-store consistency design
- Tokio-based async ingestion achieving 284K items indexed
- Configurable container keep-alive for debugging
- OpenCode containers exposed via host port mapping
- Git-based write workflow for developer agent
- Rebuild-affected script for post-commit container updates

##### Workspace & Execution Engine

- Workspace module with DB migrations and REST endpoints for project isolation
- DockerClient for per-workspace volume orchestration
- Workspace clone, diff, commit, reset, and SSE stream endpoints
- GitHub client for repository access without local checkout
- Workspace archiving with automatic Docker volume cleanup
- OpenCode container manager with orchestrator flow and event bridge
- Container lifecycle management for sandboxed code execution
- Multi-agent system configuration for autonomous development

##### Benchmarker/Validator Services

- Validator service with LLM-as-judge and composite scorer
- Benchmarker dashboard with run management and CI integration
- Validator runs migration and REST query endpoints
- Full validation pipeline: extractor → preparator → executor → comparator

##### MCP Tools (6 new tools)

- `pg_query` — Read-only SQL queries against Postgres
- `context_store` — Persistent context management
- `status_check` — Service health verification
- `task_update` — Task status tracking
- `aggregate_search` — Cross-database search (Qdrant + Postgres + Neo4j)
- `consistency_check` — Cross-database consistency verification

##### React Frontend/Playground

- React Editor Playground with Vite + React 18 + Tailwind
- Mobile-responsive navigation with drawer
- Call Sites tab for turbofish analysis
- Session persistence and management for chat

##### Snapshot Distribution System

- Zero-ingestion onboarding with pre-built snapshots
- Auto-split snapshots for GitHub Releases (>2GB support)
- Snapshot optimization (5.5GB → 3.0GB by excluding expanded_source)
- Cross-platform macOS compatibility

##### Docker Integration

- Non-root user support in API container
- Docker CLI installation for workspace volume management
- Docker socket access for container orchestration
- IPv6 resolution fixes for healthchecks

##### Additional Features

- Chat streaming with SSE support
- Neo4j placeholder nodes for relationship targets
- Memory-bounded streaming pipeline with bounded channels
- Comprehensive monitoring system for ingestion pipeline
- GPU embedding support with qwen3-embedding:4b (2560 dimensions)

### Changed

- MCP tool count increased from 15 to 16 (added search_docs, consistency_check)
- API route count increased to 54 unique paths (was 49)
- Documentation file count increased to 20+ files
- Chat timeout increased from 2 to 10 minutes for long-running conversations
- Embedding model switched from CodeLlama to qwen3-embedding:4b (2560 dimensions)
- Multi-agent config moved to global OpenCode configuration
- Call graph construction improvements (7 bugs fixed in FQN identity pipeline)
- Embed stage now loads items from database when pipeline state unavailable
- Ingestion pipeline is fully workspace-aware — all stages scope data per workspace
- Neo4j nodes now carry `workspace_id` labels for multi-tenant isolation
- `GET /health` returns per-store counts with dependency health and latency metrics

### Fixed

- Silent MessagePart drops in SSE streaming replaced with opaque Unknown event persistence (RUSA-249)
- Graph stage failures now surface errors instead of silently skipping (RUSA-245)
- Docker volume fallback for workspace file listing when volume mount is missing (RUSA-225)
- DiffViewer crash on non-string diff_summary (RUSA-228)
- Chat UI streaming state corruption across multi-turn conversations
- Callers/callees display in detail panel for impl blocks
- Neo4j restore volume name resolution
- macOS compatibility for snapshot workflow and Docker setup
- AGE agtype casting in read-path queries (RUSA-191)
- Doc comment extraction using Tree-sitter byte ranges
- MCP Server API URL in Docker (api → rustbrain-api)
- Embed stage database fallback for standalone execution
- aggregate_search MCP tool deserialization of callers/callees
- Workspace init race condition in classic playground dashboard load
- Missing X-Workspace-Id header in playground UI requests
- f32::EPSILON comparison replaced with 1e-4 tolerance in search handler tests (RUSA-205)
- Docker volume creation size quota removed to support larger workspaces
- CI failures — fmt, clippy, and GitGuardian issues resolved (RUSA-207)

### Security

- Cypher template registry with `resolve_system` mode — safe predefined query execution (RUSA-196)
- Hardened workspace label injection against bypass patterns (RUSA-198)
- Closed `apoc.graph.fromCypher` bypass in validate_cypher (RUSA-198)
- Hardened comment handling in Cypher validation
- Security audit remediation for code quality
- Non-root container execution for ingestion and MCP services
- Request limits and rate limiting

### Documentation

- Added INGESTION_GUIDE.md with comprehensive walkthrough
- Updated MCP documentation for new typecheck tools
- Corrected endpoint counts and MCP tool counts
- Added Quick Start snapshot section to README
- Comprehensive documentation audit and remediation
- Added KNOWN_ISSUES.md documenting all known limitations and failure modes
- Added RELEASE_CHECKLIST.md for systematic release verification
- Added ADR-005 (multi-tenancy physical isolation) and ADR-006 (Neo4j workspace label migration)
- Added workspace stats endpoint to API spec (RUSA-243)

## [0.2.0] - 2026-03-15

### Fixed

#### Doc Comment Extraction Fix

- **Location**: `services/ingestion/src/parsers/mod.rs`
- **Problem**: Tree-sitter byte ranges for function/struct nodes do not include preceding `///` doc comments. When extracting code snippets using these byte ranges, the resulting code would be missing its documentation, leading to incomplete context for embedding generation.
- **Root Cause**: The Tree-sitter parser calculates byte ranges based on the AST node boundaries, which start at the function/struct keyword, not the doc comments that precede it. The `syn` crate was returning empty doc strings because the extraction logic was relying on Tree-sitter's byte ranges alone.
- **Fix Applied**: When `syn` returns an empty doc string, the code now falls back to extracting doc comments directly from the full source file using a separate extraction method that looks for `///` comment blocks preceding the item.
- **How to Verify**: Run the ingestion pipeline on a Rust file with documented functions. Check that the `doc` field in the parsed items contains the full `///` comments, not just an empty string.

#### Embed Stage Database Fallback

- **Location**: `services/ingestion/src/pipeline/stages.rs`
- **Problem**: The embed stage was being skipped entirely when running standalone (without a preceding parse stage) because it expected `parsed_items` to exist in the pipeline state. This prevented the embed stage from being usable independently.
- **Root Cause**: The embed stage only checked the pipeline state's `parsed_items` field for input data. When running the embed stage in isolation (e.g., after a restart or in a separate process), this field would be empty even though the database contained previously parsed items ready for embedding.
- **Fix Applied**: Added a `load_items_from_database()` method to the embed stage that queries Postgres for items that need embeddings when `parsed_items` is not available in the state. This allows the embed stage to run independently and pick up where previous runs left off.
- **How to Verify**: 
  1. Run only the parse stage and verify items are stored in the database
  2. Run only the embed stage without running parse first
  3. Verify that embeddings are generated for the items loaded from the database

#### MCP Server API URL Fix

- **Location**: `services/mcp/Dockerfile`
- **Problem**: The MCP server container could not connect to the API service due to an incorrect hostname in `API_BASE_URL`.
- **Root Cause**: Docker Compose service discovery uses the service name defined in `docker-compose.yml` as the hostname. The URL `http://api:8080` used the wrong service name; the actual service is named `rustbrain-api`.
- **Fix Applied**: Changed `API_BASE_URL` from `http://api:8080` to `http://rustbrain-api:8080` in the Dockerfile environment variables.
- **How to Verify**: 
  1. Rebuild the MCP server container: `docker compose build mcp`
  2. Start the services: `docker compose up -d`
  3. Check MCP server logs: `docker compose logs mcp`
  4. Verify no connection errors to the API service

## [0.1.0] - Initial Release

- Initial release of rust-brain ingestion pipeline
- Tree-sitter based Rust code parsing
- Vector embedding generation and storage
- MCP server for model context protocol support
