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

# macOS: auto-apply GPU-free Docker override (Docker Desktop has no NVIDIA support)
if [ "$(uname -s)" = "Darwin" ] && [ ! -f docker-compose.override.yml ]; then
  if [ -f docker-compose.macos.yml ]; then
    cp docker-compose.macos.yml docker-compose.override.yml
    echo "✓ Applied macOS override (no NVIDIA GPU)"
  fi
fi

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
echo "=== Phase 2.5: Applying DevOps Migrations ==="
bash scripts/apply-devops-migrations.sh

echo ""
echo "=== Phase 3: Applying SQL Migrations ==="
bash scripts/apply-migrations.sh

echo ""
echo "=== Phase 4: Initializing Qdrant Collections ==="
bash scripts/init-qdrant.sh

echo ""
echo "=== Phase 5: Pulling Ollama Models ==="
bash scripts/pull-models.sh

echo ""
echo "=== Phase 6: Validating API Keys ==="

# Validate critical API keys before starting services that depend on them
MISSING_KEYS=()

if [ -z "${ANTHROPIC_API_KEY:-}" ]; then
    MISSING_KEYS+=("ANTHROPIC_API_KEY")
fi

if [ -z "${LITELLM_API_KEY:-}" ] || [ "${LITELLM_API_KEY}" = "your-api-key-here" ]; then
    MISSING_KEYS+=("LITELLM_API_KEY")
fi

if [ ${#MISSING_KEYS[@]} -gt 0 ]; then
    echo "⚠  WARNING: Missing or unconfigured API keys:"
    for key in "${MISSING_KEYS[@]}"; do
        echo "   - $key"
    done
    echo "   OpenCode and LiteLLM features will not work without valid keys."
    echo "   Set them in .env before starting."
    echo ""
    echo "   Continuing startup — databases and API will be available,"
    echo "   but AI-powered features (OpenCode, chat) will fail."
    echo ""
fi

echo ""
echo "=== Phase 7: Starting Observability + Audit Stack ==="
docker-compose up -d postgres-exporter node-exporter prometheus grafana pgweb audit

echo ""
echo "=== Phase 8: Waiting for Observability Stack ==="
sleep 5

echo ""
echo "=== Phase 9: Running Health Checks ==="
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
