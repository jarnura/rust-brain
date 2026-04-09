# Changelog

All notable changes to the rust-brain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

#### Workspace Management System
- Workspace module with DB migrations and REST endpoints for project isolation
- DockerClient for per-workspace volume orchestration
- Workspace clone, diff, commit, reset, and SSE stream endpoints
- GitHub client for repository access without local checkout
- Workspace archiving with automatic Docker volume cleanup

#### Execution Engine
- OpenCode container manager with orchestrator flow and event bridge
- Container lifecycle management for sandboxed code execution
- Multi-agent system configuration for autonomous development

#### Benchmarker/Validator Services
- Validator service with LLM-as-judge and composite scorer
- Benchmarker dashboard with run management and CI integration
- Validator runs migration and REST query endpoints
- Full validation pipeline: extractor → preparator → executor → comparator

#### MCP Tools (5 new tools)
- `pg_query` — Read-only SQL queries against Postgres
- `context_store` — Persistent context management
- `status_check` — Service health verification
- `task_update` — Task status tracking
- `aggregate_search` — Cross-database search (Qdrant + Postgres + Neo4j)

#### React Frontend/Playground
- React Editor Playground with Vite + React 18 + Tailwind
- Mobile-responsive navigation with drawer
- Call Sites tab for turbofish analysis
- Session persistence and management for chat

#### Snapshot Distribution System
- Zero-ingestion onboarding with pre-built snapshots
- Auto-split snapshots for GitHub Releases (>2GB support)
- Snapshot optimization (5.5GB → 3.0GB by excluding expanded_source)
- Cross-platform macOS compatibility

#### Docker Integration Improvements
- Non-root user support in API container
- Docker CLI installation for workspace volume management
- Docker socket access for container orchestration
- IPv6 resolution fixes for healthchecks

#### Additional Features
- Chat streaming with SSE support
- Neo4j placeholder nodes for relationship targets
- Memory-bounded streaming pipeline with bounded channels
- Comprehensive monitoring system for ingestion pipeline
- GPU embedding support with qwen3-embedding:4b (2560 dimensions)

### Changed

- Increased chat timeout from 2 to 10 minutes for long-running conversations
- Switched embedding model from CodeLlama to qwen3-embedding:4b (2560 dimensions)
- Multi-agent config moved to global OpenCode configuration
- Call graph construction improvements (7 bugs fixed in FQN identity pipeline)
- Embed stage now loads items from database when pipeline state unavailable

### Fixed

- Doc comment extraction using Tree-sitter byte ranges
- MCP Server API URL in Docker (api → rustbrain-api)
- Embed stage database fallback for standalone execution
- aggregate_search MCP tool deserialization of callers/callees
- Chat UI streaming state corruption across multi-turn conversations
- Callers/callees display in detail panel for impl blocks
- Neo4j restore volume name resolution
- macOS compatibility for snapshot workflow and Docker setup

### Security

- Hardened Cypher injection prevention
- Added request limits and rate limiting
- Security audit remediation for code quality

### Documentation

- Added INGESTION_GUIDE.md with comprehensive walkthrough
- Updated MCP documentation for new typecheck tools
- Corrected endpoint counts and MCP tool counts
- Added Quick Start snapshot section to README
- Comprehensive documentation audit and remediation
- Added KNOWN_ISSUES.md documenting all known limitations and failure modes

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
