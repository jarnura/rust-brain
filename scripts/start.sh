#!/bin/bash
# =============================================================================
# rust-brain — Startup Script
# =============================================================================
set -euo pipefail

cd "$(dirname "$0")/.."

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Starting Infrastructure               ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# Check for .env file
if [ ! -f .env ]; then
    echo "ERROR: .env file not found. Copy .env.example to .env and configure."
    exit 1
fi

# Load environment
source .env

echo "=== Phase 1: Starting Core Databases ==="
docker-compose up -d postgres neo4j qdrant ollama

echo ""
echo "=== Phase 2: Waiting for databases to be healthy ==="

# Wait for Postgres
echo "Waiting for Postgres..."
for i in {1..60}; do
    if docker-compose exec -T postgres pg_isready -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" > /dev/null 2>&1; then
        echo "✓ Postgres ready"
        break
    fi
    if [ $i -eq 60 ]; then
        echo "ERROR: Postgres not ready after 60 seconds"
        exit 1
    fi
    sleep 1
done

# Wait for Neo4j
echo "Waiting for Neo4j..."
for i in {1..60}; do
    if curl -sf "http://localhost:7474" > /dev/null 2>&1; then
        echo "✓ Neo4j ready"
        break
    fi
    if [ $i -eq 60 ]; then
        echo "ERROR: Neo4j not ready after 60 seconds"
        exit 1
    fi
    sleep 1
done

# Wait for Qdrant
echo "Waiting for Qdrant..."
for i in {1..30}; do
    if curl -sf "http://localhost:6333/healthz" > /dev/null 2>&1; then
        echo "✓ Qdrant ready"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "ERROR: Qdrant not ready after 30 seconds"
        exit 1
    fi
    sleep 1
done

# Wait for Ollama
echo "Waiting for Ollama..."
for i in {1..60}; do
    if curl -sf "http://localhost:11434/api/tags" > /dev/null 2>&1; then
        echo "✓ Ollama ready"
        break
    fi
    if [ $i -eq 60 ]; then
        echo "ERROR: Ollama not ready after 60 seconds"
        exit 1
    fi
    sleep 1
done

echo ""
echo "=== Phase 3: Initializing Qdrant Collections ==="
bash scripts/init-qdrant.sh

echo ""
echo "=== Phase 4: Pulling Ollama Models ==="
bash scripts/pull-models.sh

echo ""
echo "=== Phase 5: Starting Observability Stack ==="
docker-compose up -d postgres-exporter node-exporter prometheus grafana pgweb

echo ""
echo "=== Phase 6: Waiting for Observability Stack ==="
sleep 5

echo ""
echo "=== Phase 7: Running Health Checks ==="
bash scripts/healthcheck.sh

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Infrastructure Ready!                 ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║  Grafana:       http://localhost:3000                        ║"
echo "║  Neo4j Browser: http://localhost:7474                        ║"
echo "║  Qdrant:        http://localhost:6333/dashboard              ║"
echo "║  Pgweb:         http://localhost:8081                        ║"
echo "║  Prometheus:    http://localhost:9090                        ║"
echo "║  Ollama API:    http://localhost:11434                       ║"
echo "╚══════════════════════════════════════════════════════════════╝"
