# rust-brain Test Suite

This directory contains the test infrastructure for rust-brain.

## Structure

```
tests/
├── fixtures/
│   └── test-crate/       # Test fixture crate for ingestion testing
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs    # Comprehensive Rust code fixture
├── smoke/
│   └── test_services.sh  # Service health checks
├── integration/
│   ├── test_pipeline.sh  # Pipeline verification tests
│   └── test_api.sh       # API endpoint tests
└── README.md             # This file
```

## Test Fixture Crate

The `test-crate` is a comprehensive Rust code fixture designed to exercise all aspects of the ingestion pipeline:

### Features Tested

- **Functions**: Public, private, async, unsafe, generic with trait bounds
- **Structs**: With derive macros, generic parameters, tuple structs, unit structs
- **Enums**: Simple, with variants, explicit discriminants, C-style for FFI
- **Traits**: Definitions, implementations, associated types and constants
- **Modules**: Nested modules, re-exports, cross-module calls
- **Type Aliases**: Simple and complex
- **Constants and Statics**: Including atomic statics
- **Macro Usage**: Derive macros, vec!, println!, etc.
- **Doc Comments**: Module, item, and field level

### Building

```bash
cd tests/fixtures/test-crate
cargo check
cargo test
```

## Smoke Tests

`tests/smoke/test_services.sh` verifies all HTTP endpoints are listening.

### Services Checked

- PostgreSQL (5432)
- pgweb (8081)
- Neo4j HTTP (7474) and Bolt (7687)
- Qdrant REST (6333) and gRPC (6334)
- Ollama (11434)
- Prometheus (9090)
- Grafana (3000)
- Node Exporter (9100)

### Running

```bash
# From rust-brain root
./tests/smoke/test_services.sh
```

### Requirements

- Docker running with services up
- `nc` (netcat) for port checks
- `curl` for HTTP checks
- `psql` for database checks (optional)
- `jq` for JSON parsing (optional)

## Integration Tests

### Pipeline Tests

`tests/integration/test_pipeline.sh` runs ingestion and verifies data in all storage layers.

#### Tests

1. **Postgres Tables**: Verifies schema tables exist
2. **Ingestion Run**: Runs ingestion on test fixture
3. **Postgres Data**: Verifies source files and extracted items
4. **Neo4j Nodes**: Verifies graph nodes and relationships
5. **Qdrant Embeddings**: Verifies vector collections
6. **Semantic Search**: Tests vector similarity search

#### Running

```bash
# From rust-brain root
./tests/integration/test_pipeline.sh
```

### API Tests

`tests/integration/test_api.sh` tests all API endpoints.

#### Endpoints Tested

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | Health check |
| `/tools/search_semantic` | POST | Semantic code search |
| `/tools/get_function` | GET | Function details |
| `/tools/get_callers` | GET | Call graph queries |
| `/tools/get_trait_impls` | GET | Trait implementations |
| `/tools/find_usages_of_type` | GET | Type usage lookup |
| `/tools/get_module_tree` | GET | Module hierarchy |
| `/tools/query_graph` | POST | Raw Cypher queries |

#### Running

```bash
# From rust-brain root
./tests/integration/test_api.sh
```

Note: API tests require the Tool API service running on port 8080.

## Running All Tests

```bash
# Start services
docker compose up -d

# Wait for services to be healthy
sleep 30

# Run smoke tests
./tests/smoke/test_services.sh

# Run integration tests
./tests/integration/test_pipeline.sh
./tests/integration/test_api.sh
```

## Test Requirements

- Docker with docker-compose
- Bash 4.0+
- curl
- jq
- nc (netcat)
- psql (optional, for deeper Postgres checks)

## Timing

All tests are designed to complete within 5 minutes:

- Smoke tests: ~30 seconds
- Pipeline tests: ~2-3 minutes (depends on ingestion)
- API tests: ~1 minute

## Exit Codes

- `0`: All tests passed
- `1`: One or more tests failed
