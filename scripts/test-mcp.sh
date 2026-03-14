#!/bin/bash
# =============================================================================
# rust-brain — MCP Server Smoke Test
# =============================================================================
# Quick verification that the MCP server is running and responsive.
# Run this after starting services to verify basic functionality.
# =============================================================================

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
ENV_FILE="${PROJECT_DIR}/.env"

# Load environment
if [[ -f "${ENV_FILE}" ]]; then
    set -a
    source "${ENV_FILE}"
    set +a
fi

API_HOST="${API_HOST:-localhost}"
API_PORT="${API_PORT:-8080}"
API_URL="http://${API_HOST}:${API_PORT}"
TIMEOUT=10

echo "========================================"
echo "  rust-brain MCP Server Smoke Test"
echo "========================================"
echo ""
echo "API URL: ${API_URL}"
echo ""

ERRORS=0

# Test 1: Health Check
echo -n "Health check... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "${API_URL}/health" 2>/dev/null || echo "000")

if [[ "${HTTP_CODE}" == "200" ]]; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED (HTTP ${HTTP_CODE})${NC}"
    ERRORS=$((ERRORS + 1))
fi

# Test 2: Tools Available
echo -n "Tools endpoint... "

# Check POST tools
SEMANTIC_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    --connect-timeout "${TIMEOUT}" \
    -X POST \
    -H "Content-Type: application/json" \
    -d '{"query":"test"}' \
    "${API_URL}/tools/search_semantic" 2>/dev/null || echo "000")

GRAPH_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    --connect-timeout "${TIMEOUT}" \
    -X POST \
    -H "Content-Type: application/json" \
    -d '{"query":"MATCH (n) RETURN n LIMIT 1"}' \
    "${API_URL}/tools/query_graph" 2>/dev/null || echo "000")

# Check GET tools
FUNC_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    --connect-timeout "${TIMEOUT}" \
    "${API_URL}/tools/get_function" 2>/dev/null || echo "000")

if [[ "${SEMANTIC_CODE}" =~ ^(200|400|503)$ ]] && \
   [[ "${GRAPH_CODE}" =~ ^(200|400|503)$ ]] && \
   [[ "${FUNC_CODE}" =~ ^(200|400|404)$ ]]; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED${NC}"
    ERRORS=$((ERRORS + 1))
fi

# Test 3: Metrics
echo -n "Metrics endpoint... "
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    --connect-timeout "${TIMEOUT}" \
    "${API_URL}/metrics" 2>/dev/null || echo "000")

if [[ "${HTTP_CODE}" == "200" ]]; then
    echo -e "${GREEN}OK${NC}"
else
    echo -e "${RED}FAILED (HTTP ${HTTP_CODE})${NC}"
    ERRORS=$((ERRORS + 1))
fi

# Test 4: Response Time
echo -n "Response time... "
START_TIME=$(date +%s%3N)
curl -s -o /dev/null --connect-timeout "${TIMEOUT}" "${API_URL}/health" 2>/dev/null
END_TIME=$(date +%s%3N)
DURATION=$((END_TIME - START_TIME))

if [[ ${DURATION} -lt 1000 ]]; then
    echo -e "${GREEN}${DURATION}ms${NC}"
else
    echo -e "${YELLOW}${DURATION}ms (slow)${NC}"
fi

# Summary
echo ""
if [[ ${ERRORS} -eq 0 ]]; then
    echo -e "${GREEN}✓ MCP server is healthy and responsive${NC}"
    exit 0
else
    echo -e "${RED}✗ ${ERRORS} checks failed${NC}"
    echo ""
    echo "Troubleshooting:"
    echo "  1. Check if services are running: docker compose ps"
    echo "  2. Check logs: docker compose logs api"
    echo "  3. Run full health check: ./scripts/healthcheck.sh"
    exit 1
fi
