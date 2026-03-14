# Changelog

All notable changes to the rust-brain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

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
