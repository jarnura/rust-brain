# rust-brain Runbook

Operational guide for starting, stopping, monitoring, and troubleshooting the rust-brain infrastructure.

## Quick Reference

| Action | Command |
|--------|---------|
| Start all services | `bash scripts/start.sh` |
| Stop all services | `bash scripts/stop.sh` |
| Health check | `bash scripts/healthcheck.sh` |
| View logs | `docker-compose logs -f [service]` |
| Reset all data | `docker-compose down -v && bash scripts/start.sh` |

## Starting the System

### Full Startup

```bash
cd /home/jarnura/projects/hyperswitch/rust-brain

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

### Manual Health Checks

#### Postgres

```bash
# Via psql
docker-compose exec postgres psql -U rustbrain -d rustbrain -c "SELECT 1"

# Via pg_isready
docker-compose exec postgres pg_isready -U rustbrain -d rustbrain
```

#### Neo4j

```bash
# HTTP check
curl -s http://localhost:7474

# Cypher query
docker-compose exec neo4j cypher-shell -u neo4j -p rustbrain_dev_2024 "RETURN 1"
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
  -d '{"model": "nomic-embed-text", "input": "test"}' | jq '.embeddings[0] | length'
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

### 4. Qdrant Collection Not Found

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

### 5. Postgres Connection Refused

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

### 6. Prometheus Targets Down

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

## Port Reference Table

| Service | Port | Protocol | Purpose |
|---------|------|----------|---------|
| Postgres | 5432 | TCP | SQL database |
| Pgweb | 8081 | HTTP | Postgres web UI |
| Neo4j HTTP | 7474 | HTTP | Neo4j Browser |
| Neo4j Bolt | 7687 | Bolt | Cypher protocol |
| Qdrant REST | 6333 | HTTP | Vector DB API |
| Qdrant gRPC | 6334 | gRPC | Vector DB gRPC |
| Ollama | 11434 | HTTP | LLM API |
| Prometheus | 9090 | HTTP | Metrics & UI |
| Grafana | 3000 | HTTP | Dashboards |
| Node Exporter | 9100 | HTTP | Host metrics |
| Tool API | 8088 | HTTP | Agent endpoints (Phase 3) |

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
docker-compose exec neo4j cypher-shell -u neo4j -p rustbrain_dev_2024

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
| `POSTGRES_PASSWORD` | rustbrain_dev_2024 | Database password |
| `POSTGRES_DB` | rustbrain | Database name |
| `NEO4J_PASSWORD` | rustbrain_dev_2024 | Neo4j password |
| `GRAFANA_PASSWORD` | rustbrain | Grafana admin password |
| `EMBEDDING_MODEL` | nomic-embed-text | Embedding model |
| `EMBEDDING_DIMENSIONS` | 768 | Vector dimensions |
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
