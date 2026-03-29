# rust-brain Ingestion Guide

Complete guide to ingesting Rust codebases into rust-brain for code intelligence.

---

## Table of Contents

1. [Overview](#overview)
2. [Prerequisites](#prerequisites)
3. [Docker Requirements](#docker-requirements)
4. [Quick Start](#quick-start)
5. [Ingestion Modes](#ingestion-modes)
6. [Configuration](#configuration)
7. [Pipeline Stages](#pipeline-stages)
8. [Memory Management](#memory-management)
9. [Troubleshooting](#troubleshooting)
10. [Incremental Ingestion (Planned)](#incremental-ingestion-planned)

---

## Overview

rust-brain ingestion analyzes Rust codebases and builds a queryable knowledge graph with:
- **Semantic search** via vector embeddings (Qdrant)
- **Call graph traversal** (Neo4j)
- **Trait resolution** and implementation tracking
- **Monomorphization tracking** for generic code

### Key Principle: Containerized Execution

**Ingestion ALWAYS runs in a Docker container.** This is enforced by the `ingest.sh` wrapper script.

**Why?**
- Memory safety: Prevents host OOM crashes (ingestion can use 16-62GB RAM)
- Reproducibility: Consistent environment across machines
- Isolation: Cargo expand can have side effects; container prevents pollution
- Resource limits: Docker enforces hard memory limits

---

## Prerequisites

### System Requirements

| Requirement | Minimum | Recommended | Notes |
|-------------|---------|-------------|-------|
| **RAM** | 16 GB | 32 GB+ | Large crates need more memory |
| **CPU** | 4 cores | 8+ cores | Parallel processing |
| **Disk** | 20 GB free | 50+ GB SSD | Expanded sources cache |
| **Docker** | 24.0+ | Latest | Required for all services |
| **Docker Compose** | 2.20+ | Latest | Container orchestration |

### Verify Prerequisites

```bash
# Check Docker version
docker --version
# Expected: Docker version 24.0.0 or higher

# Check Docker Compose version
docker compose version
# Expected: Docker Compose version v2.20.0 or higher

# Check available memory
free -h
# Ensure at least 16GB total RAM

# Check Docker has access to enough memory
docker info | grep -i memory
```

### Target Crate Requirements

The Rust crate you want to ingest must:

1. **Compile successfully** with `cargo check`
2. **Have valid Cargo.toml** with all dependencies available
3. **Be on local filesystem** (or mounted into Docker)

```bash
# Verify target crate compiles
cd /path/to/your/crate
cargo check
```

---

## Docker Requirements

### Why Docker is Mandatory

Running ingestion directly on the host is **blocked** because:

1. **Memory Safety**: Ingestion can consume 16-62GB RAM. Without container limits, this can crash the host system.

2. **Cargo Expand Side Effects**: The `cargo expand` command modifies `Cargo.toml` files temporarily. Container execution ensures clean restoration.

3. **Environment Consistency**: Different machines have different Rust toolchains, Cargo versions, and system libraries. Docker provides a consistent environment.

4. **Resource Enforcement**: Docker enforces hard memory limits that cannot be exceeded.

### Container Configuration

The ingestion container is defined in `docker-compose.yml`:

```yaml
ingestion:
  build:
    context: .
    dockerfile: services/ingestion/Dockerfile
  container_name: rustbrain-ingestion
  volumes:
    - ${INGEST_TARGET_PATH:-./target-repo}:/workspace/target-repo
    - ${HOME}/.cargo/registry:/root/.cargo/registry:ro  # Share cargo cache
    - ${HOME}/.cargo/git:/root/.cargo/git:ro            # Share git cache
    - rustbrain-expand-cache:/tmp/rustbrain-expand-cache  # Persist expand cache
  environment:
    DATABASE_URL: postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@postgres:5432/${POSTGRES_DB}
    NEO4J_URI: bolt://neo4j:7687
    EMBEDDING_URL: http://ollama:11434
    QDRANT_URL: http://qdrant:6333
  deploy:
    resources:
      limits:
        memory: 32G        # Hard memory limit
        cpus: "20"         # CPU limit
```

### Memory Limits

| Setting | Default | Maximum | Configurable |
|---------|---------|---------|--------------|
| Container limit | 32GB | 62GB | `--memory-budget` flag |
| Expand stage | 2GB | - | Internal quota |
| Parse stage | 3GB | - | Internal quota |
| Embed stage | 1.5GB | - | Internal quota |

---

## Quick Start

### 1. Start the Platform

```bash
cd /path/to/rust-brain

# Start all services (databases, Ollama, etc.)
bash scripts/start.sh

# Verify services are healthy
bash scripts/healthcheck.sh
```

### 2. Run Ingestion

```bash
# Ingest a Rust crate
./scripts/ingest.sh /path/to/your/rust/crate

# Example: Ingest Hyperswitch
./scripts/ingest.sh ~/projects/hyperswitch
```

### 3. Verify Ingestion

```bash
# Check item count in Postgres
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT COUNT(*) FROM extracted_items;"

# Check vector count in Qdrant
curl -s http://localhost:6333/collections/code_embeddings | jq '.result.points_count'

# Check graph nodes in Neo4j
curl -s http://localhost:7474/db/neo4j/tx/commit \
  -u neo4j:yourpassword \
  -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (n) RETURN count(n)"}]}'
```

---

## Ingestion Modes

### Full/Clean Ingestion (Current Default)

Every ingestion run is a **full ingestion** that processes all files:

```bash
./scripts/ingest.sh /path/to/crate
```

**What happens:**
1. Discovers all `.rs` files in the crate
2. Runs `cargo expand` on each crate to resolve macros
3. Parses expanded source with tree-sitter + syn
4. Extracts functions, structs, enums, traits, impls
5. Builds Neo4j graph with relationships
6. Generates embeddings for all items
7. Stores everything in Postgres + Neo4j + Qdrant

**Data handling:**
- Existing items with the same FQN are **replaced** (upsert)
- No automatic cleanup of orphaned items
- No incremental detection of changes

### Dry Run (Validation Only)

Parse and validate without writing to databases:

```bash
./scripts/ingest.sh /path/to/crate --dry-run --verbose
```

**Use cases:**
- Validate crate can be parsed before full ingestion
- Debug parsing errors without DB pollution
- Test memory requirements

### Partial Stage Execution

Run specific pipeline stages only:

```bash
# Only parse (skip embeddings)
./scripts/ingest.sh /path/to/crate --stages expand,parse

# Only embeddings (after parse is done)
./scripts/ingest.sh /path/to/crate --stages embed
```

**Available stages:**
| Stage | Description |
|-------|-------------|
| `expand` | Run `cargo expand` to resolve macros |
| `parse` | Parse source with tree-sitter + syn |
| `typecheck` | Extract call sites (currently broken) |
| `extract` | Store items in Postgres |
| `graph` | Build Neo4j relationships |
| `embed` | Generate vector embeddings |

### Clean Ingestion (Reset First)

To completely reset all data before ingestion:

```bash
# Option 1: Reset specific database
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c "TRUNCATE extracted_items, source_files CASCADE;"

# Option 2: Reset Qdrant vectors
curl -X DELETE http://localhost:6333/collections/code_embeddings
bash scripts/init-qdrant.sh

# Option 3: Reset Neo4j graph
docker exec rustbrain-neo4j cypher-shell -u neo4j -p password "MATCH (n) DETACH DELETE n"

# Option 4: Nuclear - reset everything
docker compose down -v
docker compose up -d
```

---

## Configuration

### Environment Variables

Create a `.env` file in the rust-brain root directory:

```bash
# Database Configuration
POSTGRES_USER=rustbrain
POSTGRES_PASSWORD=your-secure-password
POSTGRES_DB=rustbrain

# Neo4j Configuration
NEO4J_PASSWORD=your-secure-password

# Model Configuration
EMBEDDING_MODEL=qwen3-embedding:4b
EMBEDDING_DIMENSIONS=2560

# Performance Tuning
EMBED_BATCH_SIZE=32           # Concurrent embedding requests
NEO4J_BATCH_SIZE=1000         # Batch size for graph writes
MAX_CONCURRENCY=4             # Pipeline parallelism

# Timeouts (in seconds)
CHAT_TIMEOUT=600              # API chat timeout
CARGO_EXPAND_TIMEOUT=180      # Per-crate expand timeout
```

### CLI Flags

```bash
./scripts/ingest.sh <workspace> [OPTIONS]
```

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--crate-path <PATH>` | `-c` | (required) | Path to Rust crate |
| `--database-url <URL>` | `-d` | from env | Postgres connection URL |
| `--neo4j-url <URL>` | | from env | Neo4j bolt URL |
| `--embedding-url <URL>` | | from env | Ollama embedding endpoint |
| `--stages <LIST>` | `-s` | all | Comma-separated stages |
| `--dry-run` | | false | Parse without writing |
| `--fail-fast` | | false | Stop on first error |
| `--max-concurrency <N>` | | 4 | Parallel task limit |
| `--verbose` | `-v` | false | Debug logging |
| `--memory-budget <SIZE>` | | 16GB | Container memory limit |
| `--help` | `-h` | | Show help |

### Memory Budget Options

```bash
# Small crate (< 10K LOC)
./scripts/ingest.sh /path/to/crate --memory-budget 8GB

# Medium crate (10K-50K LOC)
./scripts/ingest.sh /path/to/crate --memory-budget 16GB

# Large crate (50K-100K LOC)
./scripts/ingest.sh /path/to/crate --memory-budget 32GB

# Very large crate (100K+ LOC)
./scripts/ingest.sh /path/to/crate --memory-budget 48GB
```

---

## Pipeline Stages

### Stage Flow

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   EXPAND    │────▶│    PARSE    │────▶│  TYPECHECK  │────▶│   EXTRACT   │
│             │     │             │     │  (skipped)  │     │             │
│ cargo expand│     │ tree-sitter │     │             │     │  Postgres   │
│ macro resolve│    │ + syn       │     │             │     │  storage    │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
                                                                   │
                                                                   ▼
                         ┌─────────────┐                    ┌─────────────┐
                         │    EMBED    │◀───────────────────│    GRAPH    │
                         │             │                    │             │
                         │   Ollama    │                    │   Neo4j     │
                         │   Qdrant    │                    │   relations │
                         └─────────────┘                    └─────────────┘
```

### Stage Details

#### 1. Expand Stage

**Purpose:** Resolve all macros using `cargo expand`

**Process:**
1. Discover all crates in workspace
2. Pre-patch `Cargo.toml` for required features (olap, frm)
3. Run `cargo expand --lib -p <crate>` for each crate
4. Cache expanded sources to `/tmp/rustbrain-expand-cache/`
5. Restore original `Cargo.toml` files

**Output:** Expanded source files with macros resolved

**Memory:** ~2GB quota (sources streamed to cache files, not held in memory)

**Timeout:** 180 seconds per crate

#### 2. Parse Stage

**Purpose:** Extract code structure from expanded sources

**Process:**
1. Read expanded source from cache
2. **Phase 1 - tree-sitter**: Fast skeleton extraction (item types, names, positions)
3. **Phase 2 - syn**: Deep semantic parsing (signatures, generics, attributes)
4. Fallback to tree-sitter data if syn fails

**Output:** `ParsedItem` objects with full metadata

**Memory:** ~3GB quota

#### 3. Typecheck Stage

**Purpose:** Extract call sites and monomorphization info

**Status:** Currently **broken/skipped** due to state bug

**Planned Output:** Call sites with type arguments, trait impl quality scores

#### 4. Extract Stage

**Purpose:** Store items in PostgreSQL

**Process:**
1. Insert source files with original + expanded source
2. Insert/replace items in `extracted_items` table
3. Link items to source files

**Output:** Database records with FQN, signatures, doc comments

#### 5. Graph Stage

**Purpose:** Build Neo4j knowledge graph

**Process:**
1. Create nodes: Crate, Module, Function, Struct, Enum, Trait, Impl
2. Create relationships:
   - `CONTAINS`: Crate → Module → Item
   - `IMPLEMENTS`: Impl → Trait
   - `FOR`: Impl → Type
   - `CALLS`: Function → Function
   - `USES_TYPE`: Item → Type

**Output:** Queryable graph in Neo4j

#### 6. Embed Stage

**Purpose:** Generate vector embeddings for semantic search

**Process:**
1. Generate text representation for each item (signature + docs + context)
2. Send to Ollama embedding API
3. Store vectors in Qdrant with metadata payload

**Output:** Vector embeddings (2560 dimensions with qwen3-embedding:4b)

**Batching:** Processes items in batches of 8 to balance speed vs memory

---

## Memory Management

### How Memory is Managed

The ingestion pipeline uses a **stage quota system**:

```
Total Budget: 16GB (default)
├── Discover quota:    512MB
├── Expand quota:      2GB
├── Parse quota:       3GB
├── Typecheck quota:   1GB
├── Graph quota:       2GB
├── Embed quota:       1.5GB
├── Runtime overhead:  ~6GB (DB pools, async runtime, OS)
└── Safety margin:     ~1GB
```

### Key Memory Safety Features

1. **Expanded sources streamed to cache files**
   - Not held in memory
   - Only paths stored in state

2. **Batch processing with memory trimming**
   - Crates processed in batches of 8
   - `trim_memory()` called between batches
   - Cargo registry cache released

3. **Hard Docker memory limit**
   - Container killed if exceeds limit
   - Prevents host OOM

4. **Stuck detector with 10-minute threshold**
   - Warns if no progress for 10 minutes
   - Large crates can legitimately take longer

### Monitoring Memory During Ingestion

```bash
# Watch container memory
watch -n 5 'docker stats rustbrain-ingestion --no-stream'

# Check host memory
watch -n 5 'free -h'
```

### If OOM Occurs

```bash
# Increase memory budget
./scripts/ingest.sh /path/to/crate --memory-budget 32GB

# Or reduce concurrency
./scripts/ingest.sh /path/to/crate --max-concurrency 2
```

---

## Troubleshooting

### Common Issues

#### 1. "cargo expand" fails

**Symptoms:**
```
ERROR: cargo expand failed for crate 'my-crate'
```

**Causes:**
- Missing features
- Compile errors in macro code
- Dependency resolution failure

**Fix:**
```bash
# Test expand manually
cd /path/to/crate
cargo expand --lib 2>&1 | head -100

# Check for missing features
cargo check --all-features

# Verbose ingestion for details
./scripts/ingest.sh /path/to/crate --verbose
```

#### 2. Out of Memory

**Symptoms:**
- Container killed during ingestion
- Host becomes unresponsive

**Fix:**
```bash
# Increase memory
./scripts/ingest.sh /path/to/crate --memory-budget 48GB

# Reduce parallelism
./scripts/ingest.sh /path/to/crate --max-concurrency 2 --memory-budget 32GB
```

#### 3. Ingestion Appears Stuck

**Symptoms:**
- No progress for extended time
- No error messages

**Diagnosis:**
```bash
# Check if still running
docker logs rustbrain-ingestion --tail 50

# Check cargo expand processes
docker exec rustbrain-ingestion ps aux | grep cargo

# Check memory
docker stats rustbrain-ingestion --no-stream
```

**Fix:**
- Large crates can legitimately take 10+ minutes per stage
- If stuck > 30 minutes with no output, kill and retry with `--verbose`

#### 4. Connection Refused

**Symptoms:**
```
ERROR: connection refused to postgres:5432
```

**Fix:**
```bash
# Check services are running
docker compose ps

# Restart services
docker compose restart postgres neo4j qdrant ollama

# Wait for healthy status
bash scripts/healthcheck.sh
```

#### 5. Embedding Failures

**Symptoms:**
```
ERROR: embedding request failed: connection refused
```

**Fix:**
```bash
# Check Ollama is running
curl http://localhost:11434/api/tags

# Check model is pulled
curl http://localhost:11434/api/tags | jq '.models[].name'

# Pull model if missing
docker exec rustbrain-ollama ollama pull qwen3-embedding:4b
```

### Getting Debug Logs

```bash
# Enable verbose logging
./scripts/ingest.sh /path/to/crate --verbose

# Check container logs
docker logs rustbrain-ingestion 2>&1 | tee ingestion.log

# Check specific service logs
docker logs rustbrain-postgres --tail 100
docker logs rustbrain-ollama --tail 100
```

---

## Incremental Ingestion (Planned)

### Current Limitation

**Every ingestion is a full re-ingestion.** For large codebases (100K+ LOC), this can take 30+ minutes.

### Planned Implementation

The incremental ingestion feature is tracked as **Gap #6** in `docs/GAP_ANALYSIS.md`.

**Planned approach:**

```
┌──────────────────────────────────────────────────────────────┐
│                 INCREMENTAL INGESTION FLOW                   │
└──────────────────────────────────────────────────────────────┘

1. DETECT CHANGES
   └─ Compute content hash for each file
   └─ Compare with stored hashes in source_files.content_hash
   └─ Identify: new, modified, deleted files

2. PROCESS CHANGED FILES ONLY
   └─ Run expand + parse for changed files
   └─ Generate new embeddings
   └─ Update graph relationships

3. CLEANUP ORPHANS
   └─ Delete items for removed files
   └─ Remove orphaned graph nodes
   └─ Delete orphaned vectors

4. UPDATE STATE
   └─ Store new content hashes
   └─ Track last-ingested git commit
```

### Planned CLI

```bash
# Incremental ingestion (future)
./scripts/ingest.sh /path/to/crate --incremental

# Check what would be updated (future)
./scripts/ingest.sh /path/to/crate --incremental --dry-run

# Force full re-ingestion
./scripts/ingest.sh /path/to/crate --full
```

### Database Schema Additions (Planned)

```sql
-- Track ingestion runs
CREATE TABLE ingestion_runs (
    id UUID PRIMARY KEY,
    repository_path TEXT NOT NULL,
    git_commit TEXT,
    started_at TIMESTAMP,
    completed_at TIMESTAMP,
    status TEXT,
    items_processed INTEGER,
    items_added INTEGER,
    items_updated INTEGER,
    items_deleted INTEGER
);

-- Add content hash to source_files
ALTER TABLE source_files ADD COLUMN content_hash TEXT;
ALTER TABLE source_files ADD COLUMN last_ingested_at TIMESTAMP;
```

### When Will This Be Available?

Incremental ingestion is a **high priority** feature planned for Phase 2. See `docs/GAP_ANALYSIS.md` for the full roadmap.

---

## Summary

| Aspect | Current State |
|--------|---------------|
| **Execution** | Always in Docker container |
| **Mode** | Full ingestion only |
| **Memory** | 16GB default, up to 62GB configurable |
| **Incremental** | Not yet implemented (planned) |
| **Data handling** | Upsert (replace existing items) |
| **Cleanup** | Manual (no automatic orphan removal) |

### Quick Reference Commands

```bash
# Start platform
bash scripts/start.sh

# Ingest a crate
./scripts/ingest.sh /path/to/crate

# Ingest with more memory
./scripts/ingest.sh /path/to/crate --memory-budget 32GB

# Dry run (validation)
./scripts/ingest.sh /path/to/crate --dry-run --verbose

# Check health
bash scripts/healthcheck.sh

# Reset all data
docker compose down -v && bash scripts/start.sh
```
