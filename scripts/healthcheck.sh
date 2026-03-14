#!/bin/bash
# =============================================================================
# rust-brain — Health Check Script
# =============================================================================
set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

ERRORS=0

check_service() {
    local name=$1
    local url=$2
    
    printf "%-20s" "$name"
    
    if curl -sf --max-time 5 "$url" > /dev/null 2>&1; then
        echo -e "${GREEN}✓ OK${NC}"
        return 0
    else
        echo -e "${RED}✗ FAILED${NC}"
        ERRORS=$((ERRORS + 1))
        return 1
    fi
}

check_port() {
    local name=$1
    local host=$2
    local port=$3
    
    printf "%-20s" "$name"
    
    if nc -z -w5 "$host" "$port" 2>/dev/null; then
        echo -e "${GREEN}✓ OK${NC} ($host:$port)"
        return 0
    else
        echo -e "${RED}✗ FAILED${NC} ($host:$port)"
        ERRORS=$((ERRORS + 1))
        return 1
    fi
}

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Health Check                          ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

echo "=== HTTP Endpoints ==="
check_service "Postgres (pgweb)" "http://localhost:8081"
check_service "Neo4j Browser" "http://localhost:7474"
check_service "Qdrant Dashboard" "http://localhost:6333/healthz"
check_service "Ollama API" "http://localhost:11434/api/tags"
check_service "Prometheus" "http://localhost:9090/-/healthy"
check_service "Grafana" "http://localhost:3000/api/health"

echo ""
echo "=== TCP Ports ==="
check_port "Postgres" "localhost" "5432"
check_port "Neo4j Bolt" "localhost" "7687"
check_port "Qdrant gRPC" "localhost" "6334"

echo ""
echo "=== Container Status ==="
docker-compose ps 2>/dev/null || echo "Docker not available or no containers running"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
if [ $ERRORS -eq 0 ]; then
    echo -e "║ Status: ${GREEN}ALL SERVICES HEALTHY${NC}                               ║"
else
    echo -e "║ Status: ${RED}$ERRORS SERVICE(S) FAILED${NC}                                  ║"
fi
echo "╚══════════════════════════════════════════════════════════════╝"

exit $ERRORS
