# rust-brain Runbook

Operational guide for starting, stopping, monitoring, and troubleshooting the rust-brain infrastructure.

## Quick Reference

| Action | Command |
|--------|---------|
| Start all services | `bash scripts/start.sh` |
| Stop all services | `bash scripts/stop.sh` |
| Health check | `bash scripts/healthcheck.sh` |
| E2E smoke test | `bash scripts/smoke-test.sh` |
| View logs | `docker-compose logs -f [service]` |
| Reset all data | `docker-compose down -v && bash scripts/start.sh` |

## Starting the System

### Full Startup

```bash
cd /path/to/rust-brain

# Ensure .env exists
cp .env.example .env  # Only needed first time

# Start everything
bash scripts/start.sh
```

The startup script performs these phases:

1. **Core Databases** — Starts Postgres, Neo4j, Qdrant, Ollama
2. **Health Waits** — Waits for each service to be healthy
3. **Qdrant Init** — Creates vector collections and indexes
4. **Model Pull** — Downloads embedding and code models
5. **Observability** — Starts Prometheus, Grafana, Pgweb
6. **Health Check** — Verifies all endpoints

### Startup Time Estimates

| Service | Typical Startup |
|---------|-----------------|
| Postgres | 5-10 seconds |
| Neo4j | 30-60 seconds |
| Qdrant | 5-10 seconds |
| Ollama | 30-60 seconds (first run) |
| Models | 2-5 minutes (first download) |
| Prometheus | 5-10 seconds |
| Grafana | 10-15 seconds |

**First-time startup:** 5-10 minutes (model downloads)
**Subsequent startups:** 1-2 minutes

### Starting Individual Services

```bash
# Start only databases
docker-compose up -d postgres neo4j qdrant

# Start only observability
docker-compose up -d prometheus grafana

# Start only AI layer
docker-compose up -d ollama
```

## Stopping the System

### Graceful Shutdown

```bash
bash scripts/stop.sh
```

This runs `docker-compose down`, stopping all containers but preserving data volumes.

### Stop and Remove Data

```bash
docker-compose down -v
```

**Warning:** This deletes all stored data:
- Postgres database contents
- Neo4j graph data
- Qdrant vector indices
- Ollama pulled models (volume)
- Prometheus metrics
- Grafana dashboards

### Stop Individual Services

```bash
# Stop specific service
docker-compose stop neo4j

# Stop and remove container (keeps volume)
docker-compose rm -sf neo4j
```

## Health Checks

### Automated Health Check

```bash
bash scripts/healthcheck.sh
```

Output example:
```
╔══════════════════════════════════════════════════════════════╗
║           RUST-BRAIN — Health Check                          ║
╚══════════════════════════════════════════════════════════════╝

=== HTTP Endpoints ===
Postgres (pgweb)     ✓ OK
Neo4j Browser        ✓ OK
Qdrant Dashboard     ✓ OK
Ollama API           ✓ OK
Prometheus           ✓ OK
Grafana              ✓ OK

=== TCP Ports ===
Postgres             ✓ OK (localhost:5432)
Neo4j Bolt           ✓ OK (localhost:7687)
Qdrant gRPC          ✓ OK (localhost:6334)
```

### E2E Smoke Test

The smoke test validates the full pipeline end-to-end — not just port availability, but actual data queries across all three stores, MCP tool invocation, and cross-DB aggregate search.

```bash
bash scripts/smoke-test.sh
```

**Checks performed (9 total):**

| # | Check | What It Validates |
|---|-------|-------------------|
| 1 | Postgres query | `extracted_items` count > 0 via `pg_query` |
| 2 | Neo4j query | Total graph node count > 0 via `query_graph` |
| 3 | Qdrant health | Vector store healthy with points > 0 via `/health` |
| 4 | Semantic search | `search_semantic` returns results for a known query |
| 5 | MCP SSE bridge | SSE endpoint returns a valid session ID |
| 6 | Aggregate search | Cross-DB fan-out returns real code results |
| 7 | Trait graph query | `MATCH (n:Trait) RETURN count(n)` > 0 |
| 8 | API health | `/health` status is healthy |
| 9 | OpenCode health | OpenCode dependency reports healthy |

**When to run:**
- After every `docker compose up`
- In CI on PRs touching `configs/opencode/`, `services/mcp/`, `services/api/`
- Before any release tag

**Exit codes:** 0 = all pass, non-zero = number of failures.

### Manual Health Checks

#### Postgres

```bash
# Via psql
docker-compose exec postgres psql -U rustbrain -d rustbrain -c "SELECT 1"

# Via pg_isready
docker-compose exec postgres pg_isready -U rustbrain -d rustbrain
```

##### Apache AGE Graph Extension

AGE is compiled into the Postgres image but only activated when the `age-poc` compose override is used.

```bash
# Start Postgres with AGE (POC/dev)
docker compose -f docker-compose.yml -f docker-compose.age-poc.yml up -d postgres

# Verify AGE extension is loaded
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT * FROM ag_catalog.ag_graph;"

# Create a test graph and run a Cypher query
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT create_graph('test_graph');"
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT * FROM cypher('test_graph', \$\$ CREATE (n:Test {name: 'hello'}) RETURN n \$\$) AS (n agtype);"

# Clean up test graph
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT drop_graph('test_graph', true);"
```

Production uses only `docker compose -f docker-compose.yml up -d postgres` — no AGE init script is mounted.

#### Neo4j

```bash
# HTTP check
curl -s http://localhost:7474

# Cypher query
docker-compose exec neo4j cypher-shell -u neo4j -p <your-password> "RETURN 1"
```

#### Qdrant

```bash
# Health endpoint
curl -s http://localhost:6333/healthz

# List collections
curl -s http://localhost:6333/collections | jq '.'
```

#### Ollama

```bash
# List models
curl -s http://localhost:11434/api/tags | jq '.'

# Test embedding
curl -s http://localhost:11434/api/embed \
  -d '{"model": "qwen3-embedding:4b", "input": "test"}' | jq '.embeddings[0] | length'
```

#### Prometheus

```bash
# Health check
curl -s http://localhost:9090/-/healthy

# Targets status
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[].health'
```

#### Grafana

```bash
curl -s http://localhost:3000/api/health
```

## Common Failure Scenarios

### 1. Port Already in Use

**Symptoms:**
```
Error: port is already allocated
```

**Diagnosis:**
```bash
# Check what's using the port
sudo lsof -i :5432  # Postgres
sudo lsof -i :7474  # Neo4j HTTP
sudo lsof -i :7687  # Neo4j Bolt
sudo lsof -i :6333  # Qdrant
sudo lsof -i :11434 # Ollama
sudo lsof -i :3000  # Grafana
```

**Fix:**
```bash
# Kill the conflicting process
sudo kill -9 <PID>

# Or change the port in .env
POSTGRES_PORT=5433
```

### 2. Neo4j Fails to Start

**Symptoms:**
- Neo4j container keeps restarting
- `docker-compose ps` shows unhealthy status

**Diagnosis:**
```bash
# Check logs
docker-compose logs neo4j

# Common issue: memory
docker-compose exec neo4j cat /conf/neo4j.conf | grep memory
```

**Fixes:**

1. **Memory issues:**
   ```bash
   # Reduce memory in configs/neo4j/neo4j.conf
   server.memory.heap.initial_size=512M
   server.memory.heap.max_size=1G
   ```

2. **Permission issues:**
   ```bash
   # Fix volume permissions
   sudo chown -R 7474:7474 ./data/neo4j 2>/dev/null || true
   docker-compose restart neo4j
   ```

3. **Reset Neo4j data:**
   ```bash
   docker-compose down -v neo4j_data neo4j_logs
   docker-compose up -d neo4j
   ```

### 3. Ollama Out of Memory

**Symptoms:**
- Model loading fails
- Container crashes during inference

**Diagnosis:**
```bash
# Check memory usage
docker stats rustbrain-ollama

# Check available system memory
free -h
```

**Fixes:**

1. **Increase Docker memory limit** (Docker Desktop)
2. **Use smaller model:**
   ```bash
   # In .env, change to smaller model
   CODE_MODEL=codellama:7b-instruct-q4_0
   ```
3. **Enable GPU** (if available):
   ```yaml
   # In docker-compose.yml, uncomment GPU section
   deploy:
     resources:
       reservations:
         devices:
           - driver: nvidia
             count: all
             capabilities: [gpu]
   ```

### 4. Ingestion OOM with Large Crates

If ingestion runs out of memory when processing large codebases with `cargo expand`:

**Symptoms:**
- Ingestion process killed during expand stage
- System becomes unresponsive during batch processing

**Fix:**
The ingestion pipeline now streams expanded sources to cache files instead of loading everything into memory. Ensure you're using the latest version. For very large monorepos:

```bash
# Reduce batch size in .env
EMBED_BATCH_SIZE=8
# Run with single parallel job
PARALLEL_JOBS=1
```

### 5. Feature Flags Not Propagating (olap/frm)

If `cargo expand` fails due to missing features:

**Symptoms:**
- Expand errors about undefined features
- Conditional compilation not working

**Fix:**
The pipeline now pre-patches `Cargo.toml` to enable required features before expansion. For custom features:

```bash
# In ingestion config
REQUIRED_FEATURES=olap,frm
```

### 6. Chat Operations Timeout

If chat requests timeout during long-running queries:

**Symptoms:**
- Chat returns timeout errors
- Long queries never complete

**Fix:**
Timeouts have been increased to 10 minutes. If you need longer:

```bash
# In .env
CHAT_TIMEOUT=900  # 15 minutes
```

### 7. Ingestion Appears Stuck

If ingestion appears to hang without progress:

**Symptoms:**
- No log output for extended periods
- Process not using CPU

**Diagnosis:**
The stuck detector now waits 10 minutes before warning. Large crates can legitimately take time.

**Fix:**
If genuinely stuck after 10+ minutes:
```bash
# Check process is still running
ps aux | grep rustbrain

# Check for zombie cargo expand processes
ps aux | grep cargo

# Restart ingestion with verbose logging
rustbrain-ingestion -v --stages expand,parse
```

### 8. Qdrant Collection Not Found

**Symptoms:**
- 404 errors when querying vectors
- Collection doesn't exist

**Diagnosis:**
```bash
# List collections
curl -s http://localhost:6333/collections | jq '.'
```

**Fix:**
```bash
# Re-run initialization
bash scripts/init-qdrant.sh
```

### 9. Postgres Connection Refused

**Symptoms:**
- Connection errors from services
- `pg_isready` fails

**Diagnosis:**
```bash
# Check if Postgres is running
docker-compose ps postgres

# Check logs
docker-compose logs postgres
```

**Fixes:**

1. **Wait longer** (Postgres can take time to initialize)
   ```bash
   sleep 30
   docker-compose exec postgres pg_isready -U rustbrain
   ```

2. **Reset Postgres:**
   ```bash
   docker-compose down -v postgres_data
   docker-compose up -d postgres
   # Wait for initialization
   sleep 30
   ```

### 10. Prometheus Targets Down

**Symptoms:**
- Grafana dashboards show "No data"
- Prometheus shows targets as down

**Diagnosis:**
```bash
# Check target status
curl -s http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.health != "up")'

# Check Prometheus config
docker-compose exec prometheus cat /etc/prometheus/prometheus.yml
```

**Fix:**
```bash
# Restart services to re-register
docker-compose restart prometheus grafana

# Or full restart
bash scripts/stop.sh && bash scripts/start.sh
```

### 11. Apache AGE Extension Not Loading

**Symptoms:**
- `ERROR: could not open extension control file "/usr/share/postgresql/16/extension/age.control"`
- `ERROR: shared library "age" not found` on container startup
- `SELECT create_graph(...)` fails with `function does not exist`
- `SELECT * FROM cypher(...)` returns `function cypher does not exist`

**Diagnosis:**
```bash
# Check if AGE shared library loaded at startup
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SHOW shared_preload_libraries;"

# Check if extension is installed
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT * FROM pg_available_extensions WHERE name = 'age';"

# Check search_path (must include ag_catalog for Cypher)
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SHOW search_path;"

# Check container logs for AGE errors
docker logs rustbrain-postgres 2>&1 | grep -i age
```

**Fixes:**

1. **Extension not found (build failed):**
   ```bash
   # Rebuild the Docker image from scratch
   docker compose build --no-cache postgres
   docker compose up -d postgres
   ```

2. **Shared library not loaded:**
   ```bash
   # Verify postgresql.conf has shared_preload_libraries
   docker exec rustbrain-postgres cat /usr/share/postgresql/postgresql.conf.sample | grep shared_preload
   # Should show: shared_preload_libraries = 'age'
   # If missing, rebuild: docker compose build --no-cache postgres
   ```

3. **search_path missing ag_catalog:**
   ```bash
   # Manually set for current session
   docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
     -c "SET search_path = ag_catalog, \"$user\", public;"

   # Fix permanently for the database
   docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
     -c "ALTER DATABASE rustbrain SET search_path = ag_catalog, \"$user\", public;"
   ```

4. **Init script didn't run (existing volume):**
   The `/docker-entrypoint-initdb.d/` scripts only run on first container initialization.
   If the `postgres_data` volume already existed, AGE setup was skipped.
   ```bash
   # Option A: Run the setup SQL manually
   docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
     -f /docker-entrypoint-initdb.d/02-age-setup.sql

   # Option B: Reset the database (DESTRUCTIVE — loses all data)
   docker compose down -v postgres_data
   docker compose up -d postgres
   ```

### 12. Cross-Store Consistency Failure

**Symptoms:**
- Prometheus alert `ConsistencyCheckFailed` fires (severity: critical)
- Grafana "Cross-Store Consistency" dashboard shows red status
- `/health/consistency` returns HTTP 503

**Diagnosis:**
```bash
# Check aggregate health
curl -s http://localhost:8088/health/consistency | jq '.'

# Detailed per-crate report
curl -s "http://localhost:8088/api/consistency?detail=full" | jq '.'

# Check a specific crate
curl -s "http://localhost:8088/api/consistency?crate=my_crate&detail=full" | jq '.'
```

**Common inconsistency patterns:**

| Symptom | Cause | Fix |
|---------|-------|-----|
| Neo4j count < Postgres count | Graph stage incomplete or failed | Re-run Graph stage |
| Qdrant count < Postgres count | Embed stage incomplete or failed | Re-run Embed stage |
| Neo4j count > Postgres count | Orphaned graph nodes from deleted source | Re-ingest the crate |
| Qdrant count > Postgres count | Orphaned embeddings from deleted source | Re-ingest the crate |

**Recovery — Selective Stage Re-Run:**

```bash
# Re-run Graph stage only (skips expand/parse/typecheck/extract)
bash scripts/ingest.sh --from-stage graph /path/to/crate

# Re-run Embed stage only (skips everything up to embedding)
bash scripts/ingest.sh --from-stage embed /path/to/crate

# Re-run Graph + Embed stages
bash scripts/ingest.sh --from-stage graph /path/to/crate
```

**Recovery — Full Re-Ingestion:**

```bash
# Nuclear option: re-ingest from scratch (safe — idempotent writes)
bash scripts/ingest.sh /path/to/crate
```

**Verification after recovery:**

```bash
# Confirm consistency is restored
curl -s http://localhost:8088/health/consistency | jq '.status'
# Expected: "healthy"

# Check detailed counts match
curl -s "http://localhost:8088/api/consistency?crate=my_crate" | jq '.store_counts'
# All three counts should match
```

**Monitoring:**
- Grafana dashboard: `http://localhost:3000/d/rustbrain-consistency`
- Prometheus alerts: `http://localhost:9090/alerts`
- The blackbox exporter probes `/health/consistency` every 15 seconds

## Resetting Data

### Reset All Data

```bash
# Stop everything and remove volumes
docker-compose down -v

# Remove any orphaned volumes
docker volume prune -f

# Start fresh
bash scripts/start.sh
```

### Reset Specific Database

```bash
# Reset Postgres only
docker-compose down -v postgres_data
docker-compose up -d postgres

# Reset Neo4j only
docker-compose down -v neo4j_data neo4j_logs
docker-compose up -d neo4j

# Reset Qdrant only
docker-compose down -v qdrant_data
docker-compose up -d qdrant
bash scripts/init-qdrant.sh

# Reset Ollama models (re-download)
docker-compose down -v ollama_data
docker-compose up -d ollama
bash scripts/pull-models.sh
```

### Reset Observability Data

```bash
# Reset Prometheus metrics
docker-compose down -v prometheus_data
docker-compose up -d prometheus

# Reset Grafana dashboards (uses provisioned ones from configs/)
docker-compose down -v grafana_data
docker-compose up -d grafana
```

## OpenCode Developer Agent Write Workflow

The developer agent in OpenCode needs write access to create and modify files. A git-based workflow provides isolated write access:

### How It Works

1. On container start, git is configured with `GIT_USER_NAME` and `GIT_USER_EMAIL`
2. The entrypoint script clones or copies the target repo to `/workspace/target-repo-work` (excluding `target/` and `node_modules/` to avoid build artifact permission issues)
3. An initial commit is created from the copied source
4. A feature branch is created (e.g., `opencode/changes-20260408-120000`)
5. Developer agent works in the writable clone
6. Changes are committed to the feature branch for review

If the container restarts and the work directory already has commits (persistent volume), setup is skipped and the agent continues from where it left off.

### Required Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TARGET_REPO_URL` | (none) | Optional URL to clone instead of copying from `TARGET_REPO_PATH` |
| `TARGET_REPO_PATH` | (none) | Host path to the mounted repo (read-only source) |
| `OPENCODE_WORK_DIR` | `/workspace/target-repo-work` | Writable directory inside container |
| `GIT_USER_NAME` | `OpenCode Developer` | Git author name for commits |
| `GIT_USER_EMAIL` | `opencode@rustbrain.local` | Git author email for commits |
| `FEATURE_BRANCH_PREFIX` | `opencode` | Prefix for feature branches |
| `GH_TOKEN` | (none) | GitHub token for private repos and pushing |

### Configuration in .env

```bash
# Source repo (mounted read-only)
TARGET_REPO_PATH=/home/user/projects/hyperswitch

# Or clone from URL (overrides TARGET_REPO_PATH)
TARGET_REPO_URL=https://github.com/juspay/hyperswitch

# Git identity
GIT_USER_NAME="Your Name"
GIT_USER_EMAIL="your@email.com"

# For private repos or pushing changes
GH_TOKEN=ghp_xxxx
```

### Verifying Write Access

```bash
# Check the work directory was created
docker exec rustbrain-opencode ls -la /workspace/target-repo-work

# Verify git config
docker exec rustbrain-opencode git config user.name
docker exec rustbrain-opencode git config user.email

# Check the feature branch
docker exec rustbrain-opencode git branch
```

### Troubleshooting

**Issue: Work directory is empty**

The target repo must be available either via `TARGET_REPO_URL` or mounted at `TARGET_REPO_PATH`:

```bash
# Check if source repo is mounted
docker exec rustbrain-opencode ls -la /workspace/target-repo

# Check logs for clone/copy errors
docker logs rustbrain-opencode 2>&1 | head -50
```

**Issue: Git push fails**

Ensure `GH_TOKEN` is set with repo write permissions:

```bash
# Test token access
curl -H "Authorization: token $GH_TOKEN" https://api.github.com/user

# Check if token is passed to container
docker exec rustbrain-opencode env | grep GH_TOKEN
```

**Issue: Feature branch not created**

Check entrypoint script execution:

```bash
# View startup logs
docker logs rustbrain-opencode 2>&1 | grep -A5 "feature branch"
```

### Resetting OpenCode Work Directory

To wipe the work directory and start fresh:

```bash
docker compose down -v opencode_work
docker compose up -d opencode
```

## Port Reference Table

| Service | Port | Protocol | Purpose |
|---------|------|----------|---------|
| Postgres | 5432 | TCP | SQL database |
| Pgweb | 8085 | HTTP | Postgres web UI |
| Neo4j HTTP | 7474 | HTTP | Neo4j Browser |
| Neo4j Bolt | 7687 | Bolt | Cypher protocol |
| Qdrant REST | 6333 | HTTP | Vector DB API |
| Qdrant gRPC | 6334 | gRPC | Vector DB gRPC |
| Ollama | 11434 | HTTP | LLM API |
| Prometheus | 9090 | HTTP | Metrics & UI |
| Grafana | 3000 | HTTP | Dashboards |
| Node Exporter | 9100 | HTTP | Host metrics |
| Blackbox Exporter | 9115 | HTTP | HTTP probe service |
| Tool API | 8088 | HTTP | REST API + Playground |
| MCP SSE | 3001 | HTTP/SSE | MCP streaming transport |
| OpenCode | 4096 | HTTP | IDE integration |

### Changing Ports

Edit `.env` file:

```bash
# Example: Change Grafana port
GRAFANA_PORT=3001

# Example: Change Neo4j ports
NEO4J_HTTP_PORT=8474
NEO4J_BOLT_PORT=8687
```

Then restart:
```bash
docker-compose down
docker-compose up -d
```

## Viewing Logs

### All Services

```bash
# Follow all logs
docker-compose logs -f

# Last 100 lines per service
docker-compose logs --tail=100
```

### Specific Service

```bash
# Follow specific service
docker-compose logs -f postgres
docker-compose logs -f neo4j
docker-compose logs -f ollama
docker-compose logs -f qdrant

# With timestamps
docker-compose logs -f --timestamps neo4j
```

### Log Locations (Inside Containers)

| Service | Log Path |
|---------|----------|
| Postgres | `/var/lib/postgresql/data/log/` |
| Neo4j | `/logs/` |
| Qdrant | stdout only |
| Ollama | stdout only |
| Prometheus | stdout only |
| Grafana | `/var/log/grafana/` |

## Useful Commands

```bash
# List all containers
docker-compose ps

# Show resource usage
docker stats

# Execute command in container
docker-compose exec postgres psql -U rustbrain -d rustbrain
docker-compose exec neo4j cypher-shell -u neo4j -p <your-password>

# Copy file from container
docker cp rustbrain-postgres:/var/lib/postgresql/data ./postgres-backup

# Inspect network
docker network inspect rustbrain_rustbrain-net

# Rebuild container (after config change)
docker-compose up -d --force-recreate neo4j
```

## Environment Variables

Key variables in `.env`:

| Variable | Default | Description |
|----------|---------|-------------|
| `POSTGRES_USER` | rustbrain | Database user |
| `POSTGRES_PASSWORD` | <your-password> | Database password |
| `POSTGRES_DB` | rustbrain | Database name |
| `NEO4J_PASSWORD` | <your-password> | Neo4j password |
| `GRAFANA_PASSWORD` | rustbrain | Grafana admin password |
| `EMBEDDING_MODEL` | qwen3-embedding:4b | Embedding model |
| `EMBEDDING_DIMENSIONS` | 2560 | Vector dimensions |
| `CODE_MODEL` | codellama:7b | Code understanding model |
| `EMBED_BATCH_SIZE` | 32 | Concurrent embeddings |
| `NEO4J_BATCH_SIZE` | 1000 | Concurrent graph writes |

## Backup and Restore

### Backup Postgres

```bash
# Full backup
docker-compose exec postgres pg_dump -U rustbrain rustbrain > backup_$(date +%Y%m%d).sql

# Restore
cat backup_20260314.sql | docker-compose exec -T postgres psql -U rustbrain rustbrain
```

### Backup Neo4j

```bash
# Dump database
docker-compose exec neo4j neo4j-admin database dump neo4j --to-path=/backup/

# Or copy data volume
docker run --rm -v rustbrain_neo4j_data:/data -v $(pwd)/backup:/backup alpine tar czf /backup/neo4j_data.tar.gz /data
```

### Backup Qdrant

```bash
# Create snapshot
curl -X POST http://localhost:6333/collections/code_embeddings/snapshots

# Download snapshot
curl http://localhost:6333/collections/code_embeddings/snapshots > qdrant_snapshot.tar
```

