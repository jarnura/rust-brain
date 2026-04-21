# rust-brain Runbook

Operational guide for starting, stopping, monitoring, and troubleshooting the rust-brain infrastructure.

## Quick Reference

| Action | Command |
|--------|---------|
| Start all services | `bash scripts/start.sh` |
| Stop all services | `bash scripts/stop.sh` |
| Health check | `bash scripts/healthcheck.sh` |
| Apply SQL migrations | `bash scripts/apply-migrations.sh` |
| Apply devops migrations | `bash scripts/apply-devops-migrations.sh` |
| E2E smoke test | `bash scripts/smoke-test.sh` |
| View logs | `docker compose logs -f [service]` |
| Reset all data | `docker compose down -v && bash scripts/start.sh` |

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
2.5. **DevOps Migrations** — Applies numbered schema migrations from `scripts/migrations/` (tracked in `_devops_migrations` table)
3. **SQL Migrations** — Applies pending migrations from `services/api/migrations/`
4. **Qdrant Init** — Creates vector collections and indexes
5. **Model Pull** — Downloads embedding and code models
6. **API Key Validation** — Warns if `ANTHROPIC_API_KEY` or `LITELLM_API_KEY` are missing
7. **Observability** — Starts Prometheus, Grafana, Pgweb
8. **Health Check** — Verifies all endpoints

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
docker compose up -d postgres neo4j qdrant

# Start only observability
docker compose up -d prometheus grafana

# Start only AI layer
docker compose up -d ollama
```

## Stopping the System

### Graceful Shutdown

```bash
bash scripts/stop.sh
```

This runs `docker compose down`, stopping all containers but preserving data volumes.

### Stop and Remove Data

```bash
docker compose down -v
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
docker compose stop neo4j

# Stop and remove container (keeps volume)
docker compose rm -sf neo4j
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
docker compose exec postgres psql -U rustbrain -d rustbrain -c "SELECT 1"

# Via pg_isready
docker compose exec postgres pg_isready -U rustbrain -d rustbrain
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
docker compose exec neo4j cypher-shell -u neo4j -p <your-password> "RETURN 1"
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

### Grafana Dashboards

All dashboards are provisioned from `configs/grafana/dashboards/` and auto-loaded on Grafana startup.

| Dashboard | UID | URL | Description |
|-----------|-----|-----|-------------|
| Infrastructure Overview | `rustbrain-infra` | `http://localhost:3000/d/rustbrain-infra` | Container CPU, memory, service status |
| Database Health | `rustbrain-db` | `http://localhost:3000/d/rustbrain-db` | Postgres connections, transactions, DB size |
| Ingestion Pipeline | `rustbrain-pipeline` | `http://localhost:3000/d/rustbrain-pipeline` | Ingestion runs, items extracted, phase durations |
| Cross-Store Consistency | `rustbrain-consistency` | `http://localhost:3000/d/rustbrain-consistency` | Consistency probe status, per-crate item counts |
| Workspace Leak Audit | `rustbrain-leak-audit` | `http://localhost:3000/d/rustbrain-leak-audit` | Orphan volumes/containers, cross-workspace contamination |
| **Workspace Overview** | `rustbrain-workspace-overview` | `http://localhost:3000/d/rustbrain-workspace-overview` | Workspace lifecycle, status distribution, indexing progress, audit events |

After modifying a dashboard JSON, restart Grafana to reload:

```bash
docker compose restart grafana
# Or use the provisioning API for hot reload (no restart):
curl -X POST http://localhost:3000/api/admin/provisioning/dashboards/reload \
  -u admin:rustbrain
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
- `docker compose ps` shows unhealthy status

**Diagnosis:**
```bash
# Check logs
docker compose logs neo4j

# Common issue: memory
docker compose exec neo4j cat /conf/neo4j.conf | grep memory
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
   docker compose restart neo4j
   ```

3. **Reset Neo4j data:**
   ```bash
   docker compose down -v neo4j_data neo4j_logs
   docker compose up -d neo4j
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
docker compose ps postgres

# Check logs
docker compose logs postgres
```

**Fixes:**

1. **Wait longer** (Postgres can take time to initialize)
   ```bash
   sleep 30
   docker compose exec postgres pg_isready -U rustbrain
   ```

2. **Reset Postgres:**
   ```bash
   docker compose down -v postgres_data
   docker compose up -d postgres
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
docker compose exec prometheus cat /etc/prometheus/prometheus.yml
```

**Fix:**
```bash
# Restart services to re-register
docker compose restart prometheus grafana

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

### 13. Cross-Workspace Leak Alert

**Symptoms:**
- Prometheus alert `CrossWorkspaceLeakDetected` fires (severity: critical)
- Prometheus alert `OrphanNodesDetected` fires (severity: warning)
- Grafana "Workspace Isolation" dashboard shows non-zero leak counts

**Diagnosis:**
```bash
# Check leak metrics directly
curl -s http://localhost:8088/metrics | grep rustbrain_workspace_leak

# Run the detection query manually against Neo4j
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n) WITH labels(n) AS labels WHERE size([l IN labels WHERE l STARTS WITH 'Workspace_']) <> 1 RETURN labels, count(*) AS count"

# Find specific contaminated nodes (multi-workspace labels)
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n) WHERE size([l IN labels(n) WHERE l STARTS WITH 'Workspace_']) > 1 RETURN id(n), labels(n), properties(n) LIMIT 50"

# Find orphan nodes (zero workspace labels)
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n) WHERE size([l IN labels(n) WHERE l STARTS WITH 'Workspace_']) = 0 RETURN id(n), labels(n), properties(n) LIMIT 50"

# Correlate with audit logs (search for workspace_id in structured logs)
docker logs rustbrain-api 2>&1 | grep -i "workspace_id" | tail -100
```

**Containment steps:**

1. **Disable the affected workspace** to prevent further contamination:
   ```bash
   curl -X PATCH http://localhost:8088/api/workspaces/{workspace_id} \
     -H 'Content-Type: application/json' -d '{"status": "suspended"}'
   ```

2. **Snapshot data before investigation:**
   ```bash
   docker exec rustbrain-neo4j neo4j-admin database dump neo4j --to-path=/backup/
   docker exec rustbrain-postgres pg_dump -U rustbrain rustbrain > /tmp/pre-investigation.sql
   ```

3. **Investigate the middleware path** — review audit logs for the affected workspace_id to trace which API endpoint bypassed workspace isolation. Common paths:
   - Direct Neo4j bolt connection bypassing API middleware
   - Batch ingestion writing to wrong workspace label
   - Template query missing workspace label filter

4. **Remediate contaminated nodes:**
   ```cypher
   MATCH (n:Workspace_X:Workspace_Y) WHERE id(n) = <node_id>
   REMOVE n:Workspace_Y
   ```

5. **Label orphan nodes** if they belong to a known workspace:
   ```cypher
   MATCH (n) WHERE id(n) = <node_id>
   SET n:Workspace_<correct_id>
   ```

6. **Verify remediation:**
   ```bash
   curl -s http://localhost:8088/api/audit/leak-check | jq '.'
   ```

**Pre-migration baseline:**

Before Phase 3 ships, all existing Neo4j nodes have zero workspace labels (they pre-date workspace isolation). The leak detection job establishes a baseline count of these "legacy orphans" so the `OrphanNodesDetected` alert only fires for *new* orphans.

The baseline is stored as a Prometheus metric `rustbrain_workspace_leak_baseline_orphan_nodes`. To update it after migration:
```bash
curl -X POST http://localhost:8088/api/audit/baseline/recalculate
```

### 14. Docker Resource Leak Detection

**Symptoms:**
- Prometheus alert `RustbrainOrphanVolumes` fires (severity: warning)
- Prometheus alert `RustbrainOrphanContainers` fires (severity: warning)
- Prometheus alert `RustbrainLeakDetectionStale` fires (leak detector not running)
- `docker volume ls` shows volumes not in the workspaces table

**Diagnosis:**

```bash
# Run the leak detector in dry-run mode
./scripts/leak-detector.sh

# Check leak metrics
curl -s http://localhost:8090/metrics | grep rustbrain_leak

# List all workspace volumes
docker volume ls --filter label=rustbrain.workspace=true

# List tracked volumes in Postgres
docker exec rustbrain-postgres psql -U rustbrain -c \
  "SELECT volume_name, status FROM workspaces WHERE volume_name IS NOT NULL;"

# List all execution containers
docker ps -a --filter "name=rustbrain-exec-"

# List tracked containers in Postgres
docker exec rustbrain-postgres psql -U rustbrain -c \
  "SELECT container_id, status FROM executions WHERE container_id IS NOT NULL;"

# Query workspace audit log for cleanup failures
docker exec rustbrain-postgres psql -U rustbrain -c \
  "SELECT * FROM workspace_audit_log WHERE operation IN ('volume_remove_failed','container_remove_failed','cleanup_failed') ORDER BY created_at DESC LIMIT 20;"
```

**Containment steps:**

1. **Identify orphans** — compare Docker resources against Postgres:
   ```bash
   ./scripts/leak-detector.sh --dry-run
   ```

2. **Investigate specific volume** — check what workspace it belonged to:
   ```bash
   docker volume inspect rustbrain-ws-XXXXXXXXXXXX
   docker exec rustbrain-postgres psql -U rustbrain -c \
     "SELECT id, status, created_at FROM workspaces WHERE volume_name = 'rustbrain-ws-XXXXXXXXXXXX';"
   ```

3. **Remove orphans** — only after confirming they're truly orphaned:
   ```bash
   ./scripts/leak-detector.sh --cleanup
   ```

4. **Or set automatic cleanup** via environment:
   ```bash
   # In .env
   LEAK_DETECTION_DRY_RUN=false
   ```

5. **Verify cleanup:**
   ```bash
   ./scripts/leak-detector.sh --dry-run
   ```

**Manual single-resource removal:**

```bash
# Remove a specific orphaned volume
docker volume rm rustbrain-ws-XXXXXXXXXXXX

# Remove a specific orphaned container
docker rm -f <container_id>

# Stop and remove all rustbrain-exec containers
docker ps -a --filter "name=rustbrain-exec-" -q | xargs -r docker rm -f
```

**Scheduling the leak detector:**

The leak detector runs inside the audit service (Rust) which queries Docker and Postgres automatically at `AUDIT_INTERVAL_SECS` intervals. The shell script `scripts/leak-detector.sh` is available as a standalone operational tool.

To run via cron on the host (complementary to the audit service):
```bash
# Every 10 minutes, write metrics for node_exporter textfile collector
*/10 * * * * /path/to/rust-brain/scripts/leak-detector.sh --metrics-only
```

**Audit log retention:**

Workspace audit log entries are automatically pruned after `AUDIT_LOG_RETENTION_DAYS` (default: 90). To manually prune:
```bash
docker exec rustbrain-postgres psql -U rustbrain -c \
  "DELETE FROM workspace_audit_log WHERE created_at < now() - interval '90 days';"
```

### 15. Cross-Workspace Relationship and Label Mismatch Alert

**Symptoms:**
- Prometheus alert `CrossWorkspaceRelationshipDetected` fires (severity: critical)
- Prometheus alert `LabelMismatchDetected` fires (severity: warning)
- Grafana "Workspace Isolation" dashboard shows non-zero cross-workspace edge or mismatch counts
- Audit service logs show `ALERT: N cross-workspace relationships detected` or `ALERT: N label-context mismatches detected`

**Diagnosis:**

```bash
# Check cross-workspace metrics
curl -s http://localhost:8090/metrics | grep -E 'rustbrain_workspace_leak_cross_workspace|rustbrain_workspace_leak_label_mismatch'

# Find specific cross-workspace relationships
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (src)-[r]->(tgt) WITH src, tgt, type(r) AS rel_type, [l IN labels(src) WHERE l STARTS WITH 'Workspace_'][0] AS src_ws, [l IN labels(tgt) WHERE l STARTS WITH 'Workspace_'][0] AS tgt_ws WHERE src_ws IS NOT NULL AND tgt_ws IS NOT NULL AND src_ws <> tgt_ws RETURN src.fqn, src_ws, tgt.fqn, tgt_ws, rel_type LIMIT 50"

# Find label-context mismatches (nodes whose workspace label differs from neighbors)
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n)-[r]-(neighbor) WITH n, [l IN labels(n) WHERE l STARTS WITH 'Workspace_'][0] AS node_ws, neighbor, [l IN labels(neighbor) WHERE l STARTS WITH 'Workspace_'][0] AS nbr_ws WHERE node_ws IS NOT NULL AND nbr_ws IS NOT NULL AND node_ws <> nbr_ws RETURN n.fqn, node_ws, nbr_ws, count(neighbor) AS mismatch_cnt ORDER BY mismatch_cnt DESC LIMIT 50"

# Query audit log for recorded discrepancies
docker exec rustbrain-postgres psql -U rustbrain -c \
  "SELECT workspace_id, operation, detail, created_at FROM workspace_audit_log WHERE operation IN ('cross_workspace_relationship', 'label_mismatch') ORDER BY created_at DESC LIMIT 20;"
```

**Containment steps:**

1. **Identify the affected workspaces** from the alert detail or audit log entries.

2. **Block further cross-workspace writes** by suspending the affected workspace:
   ```bash
   curl -X PATCH http://localhost:8088/api/workspaces/{workspace_id} \
     -H 'Content-Type: application/json' -d '{"status": "suspended"}'
   ```

3. **For cross-workspace relationships** — delete the offending edges:
   ```cypher
   MATCH (src:Workspace_X)-[r]->(tgt:Workspace_Y)
   WHERE src.fqn = $source_fqn AND tgt.fqn = $target_fqn
   DELETE r
   ```

4. **For label mismatches** — relabel the node to the correct workspace:
   ```cypher
   MATCH (n:Workspace_WRONG {fqn: $fqn})
   REMOVE n:Workspace_WRONG
   SET n:Workspace_CORRECT
   ```

5. **Verify remediation:**
   ```bash
   curl -s http://localhost:8090/metrics | grep rustbrain_workspace_leak
   # All cross_workspace_relationships and label_mismatches should be 0
   ```

6. **Re-enable the workspace** once confirmed clean:
   ```bash
   curl -X PATCH http://localhost:8088/api/workspaces/{workspace_id} \
     -H 'Content-Type: application/json' -d '{"status": "active"}'
   ```

**Common root causes:**
- Ingestion pipeline bug: `workspace_label` not passed to `NodeBuilder` or `RelationshipBuilder`
- Direct Neo4j bolt writes bypassing the API middleware
- Race condition during concurrent ingestion of overlapping crates
- Stale workspace label after workspace archival and re-creation

**Prevention:**
- The audit service scans at `AUDIT_INTERVAL_SECS` intervals (default: 600s)
- Alerts fire within 1-5 minutes of detection
- All discrepancies are recorded in `workspace_audit_log` for forensic analysis

### 16. Workspace Creation Fails — "gh repo clone failed" or "Permission denied"

**Symptoms:**
- `POST /workspaces` returns 202 but workspace enters `error` status
- `index_error` contains "gh repo clone failed (exit exit status: 4): To get started with GitHub CLI, please run: gh auth login"
- `index_error` contains "git clone failed (exit exit status: 128): fatal: could not create work tree dir '/tmp/rustbrain-clones/...': Permission denied"

**Root cause 1 — Empty GH_TOKEN:**
When `GH_TOKEN` is set to an empty string in docker-compose.yml, the API detects PAT auth mode and uses `gh repo clone` which requires `gh auth login`. For public repos, the API should use plain `git clone` instead.

**Fix:**
```bash
# Check current GH_TOKEN in the API container
docker exec rustbrain-api env | grep GH_TOKEN

# If GH_TOKEN is empty, remove it from docker-compose.yml environment section
# or set a valid token in .env
```

**Root cause 2 — Clone directory permissions:**
The API container runs as uid 999 (rustbrain). If `/tmp/rustbrain-clones/` on the host is owned by root, the container can't create workspace subdirectories.

**Fix:**
```bash
# Fix ownership from inside the container
docker exec -u root rustbrain-api chown rustbrain:rustbrain /tmp/rustbrain-clones

# Verify
ls -la /tmp/rustbrain-clones/
```

**Recovery after fixing:**
```bash
# Delete the errored workspace
curl -X DELETE http://localhost:8088/workspaces/<workspace-id>

# Recreate it
curl -X POST http://localhost:8088/workspaces \
  -H "Content-Type: application/json" \
  -d '{"github_url": "https://github.com/owner/repo", "name": "my-workspace"}'

# Monitor progress
curl http://localhost:8088/workspaces | python3 -m json.tool
```

### 17. SQL Migration Fails on Startup

**Symptoms:**
- `start.sh` fails at "Phase 2.5: Applying DevOps Migrations" or "Phase 3: Applying SQL Migrations"
- Error: "Failed to apply: 20260421000002_agent_events_seq.sql"
- API container starts but `validate_schema()` rejects with "SCHEMA VALIDATION FAILED"

**Diagnosis:**
```bash
# Check devops migration status
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT version, description, success, applied_at FROM _devops_migrations ORDER BY version;"

# Check sqlx migration status
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT version, description, success, installed_on FROM _sqlx_migrations ORDER BY version;"

# Check for failed migrations in either table
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "SELECT 'devops' AS type, version, description FROM _devops_migrations WHERE success = false
      UNION ALL SELECT 'sqlx', version, description FROM _sqlx_migrations WHERE success = false;"

# Manually re-run the migration scripts
bash scripts/apply-devops-migrations.sh
bash scripts/apply-migrations.sh
```

**Common issues:**

| Symptom | Cause | Fix |
|---------|-------|-----|
| `relation "agent_events" does not exist` | Migration depends on earlier migration that wasn't applied | Apply migrations in order — the script does this automatically |
| `column "seq" of relation "agent_events" already exists` | Migration was applied outside this script | Mark as applied: `INSERT INTO _sqlx_migrations (version, description, success) VALUES (20260421000002, 'agent events seq', true);` |
| `_sqlx_migrations` table missing | Very old database that predates migration tracking | Run `bash scripts/apply-migrations.sh` which creates the table |

**Manual migration application (if script fails):**
```bash
# Apply a single migration by hand
docker exec -i rustbrain-postgres psql -U rustbrain -d rustbrain \
  -v ON_ERROR_STOP=1 < services/api/migrations/20260421000002_agent_events_seq.sql

# Then record it
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "INSERT INTO _sqlx_migrations (version, description, success) VALUES (20260421000002, 'agent events seq', true) ON CONFLICT (version) DO UPDATE SET success = true;"
```

**Reset migration tracking (nuclear option):**
```bash
# Warning: only if you're sure all migrations are already applied
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "TRUNCATE _sqlx_migrations;"
bash scripts/apply-migrations.sh
```

### 18. Missing API Key Warning on Startup

**Symptoms:**
- `start.sh` prints: "WARNING: Missing or unconfigured API keys"
- OpenCode container starts but chat/model features fail
- LiteLLM proxy returns 401 errors

**Diagnosis:**
```bash
# Check current values in .env
grep -E '^(ANTHROPIC_API_KEY|LITELLM_API_KEY)=' .env

# Verify they're passed to the OpenCode container
docker exec rustbrain-opencode env | grep -E '(ANTHROPIC_API_KEY|LITELLM_API_KEY)'
```

**Fix:**
```bash
# Edit .env with real API keys
ANTHROPIC_API_KEY=sk-ant-...
LITELLM_API_KEY=sk-...   # Must not be "your-api-key-here"

# Restart services that use these keys
docker compose up -d opencode
```

**Note:** The warning is non-blocking — databases and the API server start normally. Only AI-powered features (OpenCode, chat) require valid API keys.

**LITELLM_API_KEY format validation:**

The OpenCode container's entrypoint (`configs/opencode/docker-entrypoint.sh`) validates `LITELLM_API_KEY` format at startup. The following checks are performed:

| Check | Condition | Warning |
|-------|-----------|---------|
| Missing | Key not set or empty | "LITELLM_API_KEY is not set" |
| Placeholder | Value is `your-api-key-here` | "LITELLM_API_KEY is still the placeholder value" |
| Whitespace-only | Key contains only whitespace | "LITELLM_API_KEY is whitespace-only" |
| Too short | Less than 8 characters | "LITELLM_API_KEY appears too short" |
| Invalid characters | Contains characters other than `[A-Za-z0-9_-.]` | "LITELLM_API_KEY contains unexpected characters" |

This validation is also non-blocking — the OpenCode server starts regardless, but logs warnings for invalid keys.

### 19. Ingestion OOM / Memory Alerts

**Symptoms:**
- Prometheus alert `IngestionContainerOOMKilled` fires (severity: critical)
- Prometheus alert `IngestionMemoryHigh` fires (severity: warning)
- Prometheus alert `IngestionContainerRestarting` fires (severity: critical)
- Ingestion container exits with code 137 (OOM killed)
- Ingestion pipeline fails mid-run with no output

**Diagnosis:**

```bash
# Check if the ingestion container was OOM killed
docker inspect rustbrain-ingestion --format='{{.State.OOMKilled}}' 2>/dev/null || echo "Container not found"

# Check container memory usage vs limit
docker stats --no-stream rustbrain-ingestion 2>/dev/null || echo "Container not running"

# Check recent container restarts
docker inspect rustbrain-ingestion --format='{{.RestartCount}}' 2>/dev/null || echo "Container not found"

# Check Prometheus alert status
curl -s http://localhost:9090/api/v1/alerts | jq '.data.alerts[] | select(.labels.alertname | startswith("Ingestion"))'

# Check ingestion logs for memory-related errors
docker logs rustbrain-ingestion 2>&1 | grep -iE "(oom|memory|killed|cannot allocate)" | tail -20
```

**Alert reference:**

| Alert | Severity | For | Condition |
|-------|----------|-----|-----------|
| `IngestionContainerOOMKilled` | Critical | 0m | Container OOM kill event detected |
| `IngestionMemoryHigh` | Warning | 5m | Memory usage >85% of 32GB limit |
| `IngestionContainerRestarting` | Critical | 1m | Container restart count increasing in 10min window |

**Fixes:**

1. **Reduce memory usage** by lowering concurrency:
   ```bash
   # In .env
   EMBED_BATCH_SIZE=8
   PARALLEL_JOBS=1
   NEO4J_BATCH_SIZE=500
   BATCH_SIZE=4
   ```

2. **Increase Docker memory limit** (if host has capacity):
   ```yaml
   # In docker-compose.yml, adjust the ingestion service memory limit
   deploy:
     resources:
       limits:
         memory: 48G
   ```

3. **Run ingestion with a smaller memory budget** for constrained systems:
   ```bash
   INGESTION_MEMORY_BUDGET=16GB ./scripts/ingest.sh /path/to/crate
   ```

4. **Split large monorepos** into individual crate ingestion runs:
   ```bash
   # Ingest one crate at a time
   rustbrain-ingestion -c ./crate1 --max-concurrency 2
   rustbrain-ingestion -c ./crate2 --max-concurrency 2
   ```

**Verification after recovery:**

```bash
# Confirm ingestion container is healthy and running
docker ps --filter "name=rustbrain-ingestion" --format "{{.Status}}"

# Verify memory is within bounds
docker stats --no-stream rustbrain-ingestion

# Check that alerts have resolved
curl -s http://localhost:9090/api/v1/alerts | jq '.data.alerts[] | select(.state == "firing" and (.labels.alertname | startswith("Ingestion")))'
# Expected: empty (no firing ingestion alerts)
```

## Full Workspace Cleanup (Fresh Start)

Removes **all** workspaces, their data, Docker volumes, and resets all three databases to a clean state. Use when you want a completely fresh start — e.g., after testing or before a clean ingestion.

### Automated Script

```bash
bash scripts/clean-ingestion.sh
```

This cleans the core ingestion tables but does **not** remove workspace records, schemas, or Docker volumes. For a full workspace cleanup, use the manual procedure below.

### Manual Full Workspace Cleanup

This procedure is destructive and irreversible. Ensure no active workspaces or executions are in progress.

#### 1. Delete Workspace Records (Postgres)

```bash
# Delete all workspace records (cascades to executions, agent_events, workspace_audit_log)
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
  -c "DELETE FROM workspaces;"

# Drop all workspace schemas
for schema in $(docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -t -A \
  -c "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE 'ws_%'"); do
  docker exec rustbrain-postgres psql -U rustbrain -d rustbrain \
    -c "DROP SCHEMA IF EXISTS $schema CASCADE;"
done
```

#### 2. Truncate Core Data Tables (Postgres)

```bash
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "TRUNCATE extracted_items, source_files, call_sites, trait_implementations,
   ingestion_runs, repositories, artifacts, tasks, audit_events,
   workspace_audit_log, bench_case_results, bench_runs, eval_cases,
   validator_runs, pipeline_checkpoints CASCADE;"
```

#### 3. Clear Neo4j Graph

```bash
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n) DETACH DELETE n"
```

#### 4. Delete All Qdrant Collections and Reinitialize

```bash
# Delete workspace-specific collections
for col in $(curl -s http://localhost:6333/collections | jq -r '.result.collections[].name' | grep '^ws_'); do
  curl -X DELETE "http://localhost:6333/collections/$col"
done

# Delete global collections
curl -X DELETE http://localhost:6333/collections/code_embeddings
curl -X DELETE http://localhost:6333/collections/doc_embeddings

# Reinitialize base collections
bash scripts/init-qdrant.sh
```

#### 5. Remove Workspace Docker Volumes

```bash
# List workspace volumes
docker volume ls --filter name=rustbrain-ws

# Remove all workspace volumes
for vol in $(docker volume ls --filter name=rustbrain-ws -q); do
  docker volume rm "$vol"
done
```

#### 6. Clean Host-Side Clone Directory

```bash
# Remove workspace clone data (requires root if created by Docker)
sudo rm -rf /tmp/rustbrain-clones/*
```

#### Verification

After cleanup, verify all stores are empty:

```bash
# Postgres: all data tables should be 0
docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT 'workspaces' as tbl, count(*) FROM workspaces
   UNION ALL SELECT 'extracted_items', count(*) FROM extracted_items
   UNION ALL SELECT 'source_files', count(*) FROM source_files;"

# Neo4j: should be 0 nodes
docker exec rustbrain-neo4j cypher-shell -u neo4j -p <password> \
  "MATCH (n) RETURN count(n) as node_count"

# Qdrant: should only have empty base collections
curl -s http://localhost:6333/collections | jq '.result.collections[].name'
# Expected: ["code_embeddings", "doc_embeddings"]

# Docker: no workspace volumes
docker volume ls --filter name=rustbrain-ws
# Expected: (empty)
```

## Resetting Data

### Reset All Data

```bash
# Stop everything and remove volumes
docker compose down -v

# Remove any orphaned volumes
docker volume prune -f

# Start fresh
bash scripts/start.sh
```

### Reset Specific Database

```bash
# Reset Postgres only
docker compose down -v postgres_data
docker compose up -d postgres

# Reset Neo4j only
docker compose down -v neo4j_data neo4j_logs
docker compose up -d neo4j

# Reset Qdrant only
docker compose down -v qdrant_data
docker compose up -d qdrant
bash scripts/init-qdrant.sh

# Reset Ollama models (re-download)
docker compose down -v ollama_data
docker compose up -d ollama
bash scripts/pull-models.sh
```

### Reset Observability Data

```bash
# Reset Prometheus metrics
docker compose down -v prometheus_data
docker compose up -d prometheus

# Reset Grafana dashboards (uses provisioned ones from configs/)
docker compose down -v grafana_data
docker compose up -d grafana
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
docker compose down
docker compose up -d
```

## Viewing Logs

### All Services

```bash
# Follow all logs
docker compose logs -f

# Last 100 lines per service
docker compose logs --tail=100
```

### Specific Service

```bash
# Follow specific service
docker compose logs -f postgres
docker compose logs -f neo4j
docker compose logs -f ollama
docker compose logs -f qdrant

# With timestamps
docker compose logs -f --timestamps neo4j
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
docker compose ps

# Show resource usage
docker stats

# Execute command in container
docker compose exec postgres psql -U rustbrain -d rustbrain
docker compose exec neo4j cypher-shell -u neo4j -p <your-password>

# Copy file from container
docker cp rustbrain-postgres:/var/lib/postgresql/data ./postgres-backup

# Inspect network
docker network inspect rustbrain_rustbrain-net

# Rebuild container (after config change)
docker compose up -d --force-recreate neo4j
```

## Qdrant Workspace Migration

One-time migration of Qdrant data from global collections to per-workspace collections, as defined in [ADR-005](./adr/ADR-005-multi-tenancy-physical-isolation.md).

### Overview

Migrates data from 3 global collections (`code_embeddings`, `doc_embeddings`, `crate_docs`) into per-workspace collections following the naming convention `ws_<12hex>_<collection_type>`. The `external_docs` collection stays global (not workspace-scoped per ADR-005).

### Prerequisites

- Qdrant running and healthy on `QDRANT_HOST` (default: `http://localhost:6333`)
- Postgres running with the `workspaces` table populated
- `curl` and `jq` available on the host
- `psql` available (or run from inside the Postgres container)

### Running the Migration

```bash
# Dry run — preview what would happen without writing anything
./scripts/migrate-qdrant-workspace.sh --dry-run

# Full migration (does NOT delete source collections)
./scripts/migrate-qdrant-workspace.sh

# Migration with source collection deletion after verification
./scripts/migrate-qdrant-workspace.sh --delete-source

# Custom batch size (default: 500) for large collections
./scripts/migrate-qdrant-workspace.sh --batch-size 1000
```

### What the Script Does

1. **Build crate→workspace mapping** — queries Postgres `workspaces` table and each workspace schema's `source_files` to build a `crate_name → workspace_schema_name` routing table
2. **Create per-workspace collections** — for each workspace that has data, creates `ws_<12hex>_code_embeddings`, `ws_<12hex>_doc_embeddings`, `ws_<12hex>_crate_docs` with 2560-dim cosine similarity
3. **Scroll + upsert data** — reads points from global collections in batches, adds `workspace_id` to payload, upserts into the correct workspace collection
4. **Orphan handling** — points with no workspace match go to `ws_default_<collection_type>` collections
5. **Verification** — compares source and target point counts per collection
6. **Optional cleanup** — with `--delete-source`, removes global collections after successful verification

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `QDRANT_HOST` | `http://localhost:6333` | Qdrant REST API endpoint |
| `DATABASE_URL` | (required) | Postgres connection string for workspace mapping |
| `QDRANT_DEFAULT_WORKSPACE_ID` | `default` | Workspace ID for orphan data with no mapping |
| `EMBEDDING_DIMENSIONS` | `2560` | Vector dimensions for new collections |

### Post-Migration Verification

```bash
# List all collections — should see ws_* collections alongside global ones
curl -s http://localhost:6333/collections | jq '.result.collections[].name'

# Check point count in a workspace collection
curl -s http://localhost:6333/collections/ws_550e8400e29b_code_embeddings | jq '.result.points_count'

# Verify workspace_id is present in migrated points
curl -s -X POST http://localhost:6333/collections/ws_550e8400e29b_code_embeddings/points/scroll \
  -H 'Content-Type: application/json' \
  -d '{"limit": 1, "with_payload": true}' | jq '.result.points[0].payload.workspace_id'
```

### Rollback

The migration is additive — it creates new collections without modifying global ones. To rollback:

1. Delete the per-workspace collections:
   ```bash
   # List ws_* collections
   curl -s http://localhost:6333/collections | jq -r '.result.collections[].name' | grep '^ws_'

   # Delete each one
   for col in $(curl -s http://localhost:6333/collections | jq -r '.result.collections[].name' | grep '^ws_'); do
     curl -X DELETE "http://localhost:6333/collections/$col"
   done
   ```

2. The global collections remain untouched unless `--delete-source` was used.

### Troubleshooting

**Issue: "No workspaces found in Postgres"**

The migration requires workspace records in the `workspaces` table. Create workspaces first via the API:
```bash
curl -X POST http://localhost:8088/workspaces \
  -H 'Content-Type: application/json' \
  -d '{"github_url": "https://github.com/org/repo"}'
```

**Issue: "Qdrant health check failed"**

Verify Qdrant is running and `QDRANT_HOST` is correct:
```bash
curl -s http://localhost:6333/healthz
```

**Issue: "DATABASE_URL not set"**

Set the connection string for the Postgres instance containing the `workspaces` table:
```bash
export DATABASE_URL=postgresql://rustbrain:password@localhost:5432/rustbrain
```

**Issue: Large collection migration is slow**

Increase batch size and check Qdrant resource usage:
```bash
./scripts/migrate-qdrant-workspace.sh --batch-size 2000
docker stats rustbrain-qdrant
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
docker compose exec postgres pg_dump -U rustbrain rustbrain > backup_$(date +%Y%m%d).sql

# Restore
cat backup_20260314.sql | docker compose exec -T postgres psql -U rustbrain rustbrain
```

### Backup Neo4j

```bash
# Dump database
docker compose exec neo4j neo4j-admin database dump neo4j --to-path=/backup/

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

