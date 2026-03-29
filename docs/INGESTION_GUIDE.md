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
9. [Monitoring Ingestion](#monitoring-ingestion)
10. [Troubleshooting](#troubleshooting)
11. [Incremental Ingestion (Planned)](#incremental-ingestion-planned)

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

A **clean ingestion** starts fresh by removing all previous data before ingesting. This is useful when:
- Switching to a different crate
- Fixing data corruption
- Testing with different embedding models
- Starting fresh after failed ingestion

#### Option 1: Nuclear Reset (All Data)

Completely removes all data volumes and starts fresh:

```bash
# Stop all services and remove all data volumes
docker compose down -v

# Start fresh services
bash scripts/start.sh

# Run ingestion
./scripts/ingest.sh /path/to/crate
```

**What this removes:**
- All Postgres tables (items, source files, call sites)
- All Neo4j graph nodes and relationships
- All Qdrant vector collections
- All Ollama pulled models (optional - only if you remove `ollama_data` volume)

#### Option 2: Clean All 3 Databases (Keep Services Running)

Reset all data while keeping services running:

```bash
# Reset Postgres
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "TRUNCATE extracted_items, source_files, call_sites, trait_implementations CASCADE;"

# Reset Neo4j
docker exec rustbrain-neo4j cypher-shell -u neo4j -p yourpassword "MATCH (n) DETACH DELETE n"

# Reset Qdrant
curl -X DELETE http://localhost:6333/collections/code_embeddings
curl -X DELETE http://localhost:6333/collections/doc_embeddings
bash scripts/init-qdrant.sh
```

#### Option 3: Clean Individual Databases

Reset only specific databases:

```bash
# Reset Postgres only (items, source files)
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "TRUNCATE extracted_items, source_files CASCADE;"

# Reset Qdrant vectors only
curl -X DELETE http://localhost:6333/collections/code_embeddings
bash scripts/init-qdrant.sh

# Reset Neo4j graph only
docker exec rustbrain-neo4j cypher-shell -u neo4j -p yourpassword "MATCH (n) DETACH DELETE n"
```

#### Option 4: Clean Ingestion Script

Create a reusable script:

```bash
cat > scripts/clean-ingestion.sh << 'EOF'
#!/bin/bash
# Clean all rust-brain data before fresh ingestion

set -e

# Load environment
source .env 2>/dev/null || true

echo "=== Cleaning all rust-brain data ==="

# Reset Postgres
echo "Clearing Postgres..."
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "TRUNCATE extracted_items, source_files, call_sites, trait_implementations CASCADE;" 2>/dev/null || true

# Reset Neo4j
echo "Clearing Neo4j..."
docker exec rustbrain-neo4j cypher-shell -u neo4j -p "${NEO4J_PASSWORD:-password}" \
  "MATCH (n) DETACH DELETE n" 2>/dev/null || true

# Reset Qdrant
echo "Clearing Qdrant..."
curl -s -X DELETE http://localhost:6333/collections/code_embeddings 2>/dev/null || true
curl -s -X DELETE http://localhost:6333/collections/doc_embeddings 2>/dev/null || true

# Reinitialize Qdrant collections
echo "Reinitializing Qdrant..."
bash scripts/init-qdrant.sh

echo ""
echo "=== Clean complete ==="
echo "Ready for fresh ingestion: ./scripts/ingest.sh /path/to/crate"
EOF
chmod +x scripts/clean-ingestion.sh
```

Usage:
```bash
# Clean all data
./scripts/clean-ingestion.sh

# Run fresh ingestion
./scripts/ingest.sh /path/to/crate
```

#### Option 5: Soft Reset (Upsert Mode)

Just run ingestion - it replaces existing items with same FQN:

```bash
# Upsert mode - replaces existing items
./scripts/ingest.sh /path/to/crate
```

**Note:** This doesn't remove orphaned items (files deleted from crate but still in database).

#### Clean Ingestion Summary

| Method | Speed | Scope | When to Use |
|--------|-------|-------|-------------|
| `docker compose down -v` | Slow | Everything | Complete reset, switching projects |
| `./scripts/clean-ingestion.sh` | Medium | All 3 databases | Fresh ingestion, same project |
| `TRUNCATE` Postgres | Fast | Items only | Re-ingesting same crate |
| `DELETE` Qdrant collections | Fast | Vectors only | Re-embedding with new model |
| `DETACH DELETE` Neo4j | Fast | Graph only | Rebuilding graph |
| Just run ingestion | Fastest | Upsert only | Quick re-ingestion |

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

**Purpose:** Generate vector embeddings for semantic search using GPU acceleration

**Process:**
1. Generate text representation for each item (signature + docs + context)
2. Send to Ollama embedding API (GPU-accelerated)
3. Store vectors in Qdrant with metadata payload

**Output:** Vector embeddings (2560 dimensions with qwen3-embedding:4b)

**Batching:** Processes items in batches of 8 to balance speed vs memory

---

## GPU Embedding Configuration

### Overview

The embed stage uses **Ollama** with GPU acceleration to generate vector embeddings. GPU support significantly speeds up embedding generation, especially for large codebases.

### Supported GPUs

| Vendor | Requirements | Notes |
|--------|--------------|-------|
| **NVIDIA** | CUDA 11.0+ | Most common, best support |
| **AMD** | ROCm 5.0+ | Linux only |
| **Apple Silicon** | Metal | macOS only (M1/M2/M3) |

### Checking GPU Availability

```bash
# Check NVIDIA GPU
nvidia-smi

# Check if Docker has GPU access
docker run --rm --gpus all nvidia/cuda:11.8-base nvidia-smi

# Check Ollama GPU usage
docker exec rustbrain-ollama nvidia-smi
```

### GPU Configuration in docker-compose.yml

The Ollama service is configured for GPU in `docker-compose.yml`:

```yaml
ollama:
  image: ollama/ollama:latest
  container_name: rustbrain-ollama
  ports:
    - "${OLLAMA_PORT:-11434}:11434"
  volumes:
    - ollama_data:/root/.ollama
  deploy:
    resources:
      reservations:
        devices:
          - driver: nvidia
            count: all
            capabilities: [gpu]
  environment:
    - CUDA_VISIBLE_DEVICES=all
```

### Enabling GPU (if disabled)

If GPU is not enabled, edit `docker-compose.yml`:

```yaml
# Option 1: Use all GPUs
deploy:
  resources:
    reservations:
      devices:
        - driver: nvidia
          count: all
          capabilities: [gpu]

# Option 2: Use specific GPU
deploy:
  resources:
    reservations:
      devices:
        - driver: nvidia
          device_ids: ['0']  # First GPU only
          capabilities: [gpu]
```

Then restart:
```bash
docker compose down
docker compose up -d ollama
```

### Verifying GPU Usage

```bash
# Watch GPU during embedding
watch -n 1 nvidia-smi

# Check Ollama is using GPU
docker logs rustbrain-ollama 2>&1 | grep -i cuda

# Verify model loaded on GPU
docker exec rustbrain-ollama ollama ps
```

Expected output shows model loaded:
```
NAME                    ID              SIZE    PROCESSOR    UNTIL
qwen3-embedding:4b      abc123...       2.5 GB  100% GPU     Forever
```

### GPU Memory Requirements

| Model | Dimensions | VRAM Required | Notes |
|-------|------------|---------------|-------|
| `nomic-embed-text` | 768 | ~1 GB | Small, fast |
| `all-minilm` | 384 | ~0.5 GB | Smallest |
| `qwen3-embedding:4b` | 2560 | ~3 GB | **Recommended** |
| `qwen3-embedding:8b` | 4096 | ~6 GB | Higher quality |

**For Hyperswitch (161K items):**
- Model: `qwen3-embedding:4b`
- GPU VRAM: ~3 GB for model + ~1 GB for batch processing
- Total time: ~50 minutes (vs ~4+ hours on CPU)

### Configuring Embedding Model

In `.env`:
```bash
# Embedding model (GPU)
EMBEDDING_MODEL=qwen3-embedding:4b
EMBEDDING_DIMENSIONS=2560

# Alternative models
# EMBEDDING_MODEL=nomic-embed-text
# EMBEDDING_DIMENSIONS=768
```

### Pulling Embedding Model

```bash
# Pull model to Ollama
docker exec rustbrain-ollama ollama pull qwen3-embedding:4b

# Verify model is available
docker exec rustbrain-ollama ollama list
```

### Embedding Performance Tuning

#### Batch Size

Control concurrent embedding requests in `.env`:

```bash
# Small GPU (4GB VRAM)
EMBED_BATCH_SIZE=4

# Medium GPU (8GB VRAM) - default
EMBED_BATCH_SIZE=8

# Large GPU (16GB+ VRAM)
EMBED_BATCH_SIZE=16

# CPU only (very slow)
EMBED_BATCH_SIZE=2
```

#### Parallel vs Sequential

For limited GPU memory, use smaller batches:

```bash
# Reduce batch size
./scripts/ingest.sh /path/to/crate --memory-budget 32GB

# Or set in .env
EMBED_BATCH_SIZE=4
```

### Embedding Text Representation

Each item is converted to text before embedding:

```
Function Example:
─────────────────
pub fn process_payment<T: PaymentMethod>(payment: T) -> Result<PaymentResult, Error>
Process a payment through the configured payment gateway

Module: router::payments
Crate: router
Traits used: T: PaymentMethod
Body preview:
{
    let gateway = self.select_gateway(&payment)?;
    gateway.process(payment).await
}

Struct Example:
─────────────────
pub struct PaymentRouter {
    gateways: Vec<Box<dyn Gateway>>,
    config: RouterConfig,
}

Route payments to appropriate payment gateways based on rules.

Module: router::core
Crate: router
Fields: gateways, config
```

### Embedding Without GPU (CPU Fallback)

If no GPU is available, Ollama falls back to CPU:

```bash
# Check if using CPU
docker exec rustbrain-ollama ollama ps
# Shows "100% CPU" instead of "100% GPU"
```

CPU embedding is **much slower**:
- GPU: ~100-200 embeddings/second
- CPU: ~5-10 embeddings/second

For CPU-only, reduce batch size:
```bash
EMBED_BATCH_SIZE=2
```

### Troubleshooting GPU Issues

#### GPU Not Detected

```bash
# Check NVIDIA driver
nvidia-smi

# Check CUDA version
nvcc --version

# Check Docker GPU support
docker run --rm --gpus all nvidia/cuda:11.8-base nvidia-smi
```

#### Out of GPU Memory

```bash
# Check GPU memory
nvidia-smi --query-gpu=memory.used,memory.total --format=csv

# Reduce batch size
EMBED_BATCH_SIZE=2

# Or use smaller model
EMBEDDING_MODEL=nomic-embed-text
EMBEDDING_DIMENSIONS=768
```

#### Model Not Loading on GPU

```bash
# Force GPU load
docker exec rustbrain-ollama ollama run qwen3-embedding:4b

# Check logs
docker logs rustbrain-ollama 2>&1 | grep -i "gpu\|cuda"
```

### GPU Embedding Performance

**Benchmarks with qwen3-embedding:4b (2560 dims):**

| Hardware | Items/sec | 100K items |
|----------|-----------|------------|
| RTX 4070 Ti SUPER (16GB) | ~180/sec | ~9 minutes |
| RTX 3080 (10GB) | ~120/sec | ~14 minutes |
| RTX 3060 (12GB) | ~80/sec | ~21 minutes |
| CPU (32 cores) | ~8/sec | ~3.5 hours |

**Your setup (RTX 4070 Ti SUPER):**
- Previous ingestion: 161,258 items in ~50 minutes total
- Embed stage alone: ~15 minutes at ~180 embeddings/sec

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

## Monitoring Ingestion

### Real-time Container Logs

```bash
# Follow ingestion logs in real-time
docker logs -f rustbrain-ingestion

# Last 100 lines
docker logs rustbrain-ingestion --tail 100

# With timestamps
docker logs -f rustbrain-ingestion --timestamps
```

### Memory & Resource Monitoring

```bash
# Watch container resource usage (updates every 2 seconds)
watch -n 2 'docker stats rustbrain-ingestion --no-stream'

# Check host memory
watch -n 5 'free -h'

# Check all running containers
docker stats --no-stream
```

### Service Health During Ingestion

```bash
# Check all services at once
bash scripts/healthcheck.sh

# Individual service checks
curl -s http://localhost:6333/healthz                        # Qdrant
curl -s http://localhost:11434/api/tags                      # Ollama
docker exec rustbrain-postgres pg_isready -U rustbrain       # Postgres
docker exec rustbrain-neo4j cypher-shell -u neo4j -p pass "RETURN 1"  # Neo4j
```

### Grafana Dashboards

```bash
# Open Grafana in browser
open http://localhost:3000
# Login: admin / rustbrain
```

Available dashboards:
- **rust-brain Overview** - System-wide metrics
- **Ingestion Pipeline** - Stage progress and timing

### Database Progress Queries

```bash
# Items inserted so far
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT COUNT(*) FROM extracted_items;"

# Source files processed
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT COUNT(*) FROM source_files;"

# Current ingestion run status
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT * FROM ingestion_runs ORDER BY started_at DESC LIMIT 5;"
```

### Vector Count in Qdrant

```bash
# Embeddings created so far
curl -s http://localhost:6333/collections/code_embeddings | jq '.result.points_count'

# Collection info (vectors, segments, status)
curl -s http://localhost:6333/collections/code_embeddings | jq '.result'
```

### Neo4j Graph Progress

```bash
# Node count by type
curl -s http://localhost:7474/db/neo4j/tx/commit \
  -u neo4j:yourpassword \
  -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH (n) RETURN labels(n)[0] as type, count(n) as count ORDER BY count DESC"}]}' | jq .

# Total relationship count
curl -s http://localhost:7474/db/neo4j/tx/commit \
  -u neo4j:yourpassword \
  -H "Content-Type: application/json" \
  -d '{"statements":[{"statement":"MATCH ()-[r]->() RETURN count(r)"}]}' | jq .
```

### Combined Monitor Script

Create a monitoring script for quick status:

```bash
cat > scripts/monitor-ingestion.sh << 'EOF'
#!/bin/bash
# Monitor ingestion progress

echo "=== INGESTION MONITOR ==="
echo "Time: $(date)"
echo ""

echo "--- Container Status ---"
docker ps --filter "name=ingestion" --format "table {{.Names}}\t{{.Status}}"

echo ""
echo "--- Memory Usage ---"
docker stats --no-stream --format "table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}" \
  rustbrain-ingestion rustbrain-ollama rustbrain-postgres rustbrain-neo4j 2>/dev/null

echo ""
echo "--- Progress ---"
ITEMS=$(docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -t -c 'SELECT COUNT(*) FROM extracted_items;' 2>/dev/null | tr -d ' ')
VECTORS=$(curl -s http://localhost:6333/collections/code_embeddings 2>/dev/null | jq -r '.result.points_count // "N/A"')
echo "Items: ${ITEMS:-N/A}"
echo "Vectors: ${VECTORS:-N/A}"

echo ""
echo "--- Last 10 Log Lines ---"
docker logs rustbrain-ingestion --tail 10 2>&1
EOF
chmod +x scripts/monitor-ingestion.sh
```

Run it:
```bash
./scripts/monitor-ingestion.sh
```

### Stage Progress from Verbose Logs

```bash
# Run ingestion with verbose logging
./scripts/ingest.sh /path/to/crate --verbose 2>&1 | tee ingestion.log

# In another terminal, watch for stage progress
tail -f ingestion.log | grep -E "Stage|Processing|Completed|ERROR|items"
```

### Monitoring Quick Reference

| What | Command |
|------|---------|
| **Logs** | `docker logs -f rustbrain-ingestion` |
| **Memory** | `docker stats rustbrain-ingestion --no-stream` |
| **Items** | `docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -t -c "SELECT COUNT(*) FROM extracted_items;"` |
| **Vectors** | `curl -s localhost:6333/collections/code_embeddings \| jq '.result.points_count'` |
| **Health** | `bash scripts/healthcheck.sh` |
| **Grafana** | `http://localhost:3000` |

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
