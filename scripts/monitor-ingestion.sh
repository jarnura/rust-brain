#!/bin/bash
# =============================================================================
# rust-brain Ingestion Monitor
# =============================================================================
# Quick status check for ingestion progress
#
# Usage:
#   ./scripts/monitor-ingestion.sh
#   watch -n 5 ./scripts/monitor-ingestion.sh
# =============================================================================

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Ingestion Monitor                     ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo "Time: $(date '+%Y-%m-%d %H:%M:%S')"
echo ""

# Container Status
echo "=== Container Status ==="
INGESTION_STATUS=$(docker ps --filter "name=rustbrain-ingestion" --format "{{.Status}}" 2>/dev/null || echo "Not running")
if [[ "$INGESTION_STATUS" == *"Up"* ]]; then
    echo -e "Ingestion:    ${GREEN}● Running${NC} ($INGESTION_STATUS)"
elif [[ "$INGESTION_STATUS" == *"Exited"* ]]; then
    echo -e "Ingestion:    ${RED}● Exited${NC} ($INGESTION_STATUS)"
else
    echo -e "Ingestion:    ${YELLOW}● Not running${NC}"
fi
echo ""

# Memory Usage
echo "=== Memory Usage ==="
docker stats --no-stream --format "table {{.Name}}\t{{.MemUsage}}\t{{.CPUPerc}}" \
    rustbrain-ingestion \
    rustbrain-ollama \
    rustbrain-postgres \
    rustbrain-neo4j \
    rustbrain-qdrant \
    2>/dev/null | head -6 || echo "Containers not accessible"
echo ""

# Progress
echo "=== Progress ==="

# Items count
ITEMS=$(docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -t -c \
    "SELECT COUNT(*) FROM extracted_items;" 2>/dev/null | tr -d ' ' || echo "N/A")
echo "Extracted Items:   ${ITEMS}"

# Source files
FILES=$(docker exec rustbrain-postgres psql -U rustbrain -d rustbrain -t -c \
    "SELECT COUNT(*) FROM source_files;" 2>/dev/null | tr -d ' ' || echo "N/A")
echo "Source Files:      ${FILES}"

# Vectors
VECTORS=$(curl -s http://localhost:6333/collections/code_embeddings 2>/dev/null | \
    jq -r '.result.points_count // "N/A"' || echo "N/A")
echo "Vector Embeddings: ${VECTORS}"

# Graph nodes
NODES=$(curl -s http://localhost:7474/db/neo4j/tx/commit \
    -u neo4j:$(grep NEO4J_PASSWORD .env 2>/dev/null | cut -d= -f2 || echo "password") \
    -H "Content-Type: application/json" \
    -d '{"statements":[{"statement":"MATCH (n) RETURN count(n)"}]}' 2>/dev/null | \
    jq -r '.results[0].data[0].row[0] // "N/A"' || echo "N/A")
echo "Graph Nodes:       ${NODES}"
echo ""

# Last 10 Log Lines
echo "=== Last 10 Log Lines ==="
docker logs rustbrain-ingestion --tail 10 2>&1 || echo "No logs available"
echo ""

# Health Status
echo "=== Service Health ==="
for service in postgres neo4j qdrant ollama; do
    case $service in
        postgres)
            if docker exec rustbrain-postgres pg_isready -U rustbrain -q 2>/dev/null; then
                echo -e "${GREEN}●${NC} Postgres"
            else
                echo -e "${RED}●${NC} Postgres"
            fi
            ;;
        neo4j)
            if curl -s http://localhost:7474 >/dev/null 2>&1; then
                echo -e "${GREEN}●${NC} Neo4j"
            else
                echo -e "${RED}●${NC} Neo4j"
            fi
            ;;
        qdrant)
            if curl -s http://localhost:6333/healthz | grep -q "ok" 2>/dev/null; then
                echo -e "${GREEN}●${NC} Qdrant"
            else
                echo -e "${RED}●${NC} Qdrant"
            fi
            ;;
        ollama)
            if curl -s http://localhost:11434/api/tags >/dev/null 2>&1; then
                echo -e "${GREEN}●${NC} Ollama"
            else
                echo -e "${RED}●${NC} Ollama"
            fi
            ;;
    esac
done

echo ""
echo "============================================================"
echo "Run with: watch -n 5 ./scripts/monitor-ingestion.sh"
echo "============================================================"
