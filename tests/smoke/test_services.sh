#!/bin/bash
# =============================================================================
# rust-brain — Smoke Tests: Service Health Checks
# =============================================================================
# Verifies all HTTP endpoints are listening and responding.
# Exit 1 if any service fails.
# =============================================================================

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counters
PASSED=0
FAILED=0
TOTAL=0

# Timeout for HTTP requests (seconds)
TIMEOUT=5

# Get script directory for loading .env
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
ENV_FILE="${PROJECT_DIR}/.env"

# Load environment variables
if [[ -f "${ENV_FILE}" ]]; then
    set -a
    source "${ENV_FILE}"
    set +a
fi

# Default ports if not set in .env
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
PGWEB_PORT="${PGWEB_PORT:-8081}"
NEO4J_HTTP_PORT="${NEO4J_HTTP_PORT:-7474}"
NEO4J_BOLT_PORT="${NEO4J_BOLT_PORT:-7687}"
QDRANT_REST_PORT="${QDRANT_REST_PORT:-6333}"
QDRANT_GRPC_PORT="${QDRANT_GRPC_PORT:-6334}"
OLLAMA_PORT="${OLLAMA_PORT:-11434}"
PROMETHEUS_PORT="${PROMETHEUS_PORT:-9090}"
GRAFANA_PORT="${GRAFANA_PORT:-3000}"
NODE_EXPORTER_PORT="${NODE_EXPORTER_PORT:-9100}"

# Helper function to print test result
print_result() {
    local service="$1"
    local status="$2"
    local message="$3"
    
    TOTAL=$((TOTAL + 1))
    
    if [[ "${status}" == "PASS" ]]; then
        echo -e "${GREEN}[PASS]${NC} ${service}: ${message}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}[FAIL]${NC} ${service}: ${message}"
        FAILED=$((FAILED + 1))
    fi
}

# Helper function to print warning (still counts as pass if service is optional)
print_warning() {
    local service="$1"
    local message="$2"
    echo -e "${YELLOW}[WARN]${NC} ${service}: ${message}"
}

# =============================================================================
# TEST 1: PostgreSQL Database
# =============================================================================
test_postgres() {
    local service="PostgreSQL"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    # Check if port is listening
    if nc -z localhost "${POSTGRES_PORT}" 2>/dev/null; then
        # Try to connect using psql if available
        if command -v psql &>/dev/null; then
            if psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" -c "SELECT 1" &>/dev/null; then
                print_result "${service}" "PASS" "Port ${POSTGRES_PORT} listening, connection successful"
            else
                print_result "${service}" "FAIL" "Port listening but connection failed (check credentials)"
            fi
        else
            print_result "${service}" "PASS" "Port ${POSTGRES_PORT} listening (psql not available for deep check)"
        fi
    else
        print_result "${service}" "FAIL" "Port ${POSTGRES_PORT} not listening"
    fi
}

# =============================================================================
# TEST 2: pgweb (Postgres Web UI)
# =============================================================================
test_pgweb() {
    local service="pgweb"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${PGWEB_PORT}" 2>/dev/null || echo "000")
    
    if [[ "${response}" =~ ^(200|302|401|403)$ ]]; then
        print_result "${service}" "PASS" "HTTP ${response} on port ${PGWEB_PORT}"
    elif [[ "${response}" == "000" ]]; then
        print_result "${service}" "FAIL" "Connection refused on port ${PGWEB_PORT}"
    else
        print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${PGWEB_PORT}"
    fi
}

# =============================================================================
# TEST 3: Neo4j HTTP
# =============================================================================
test_neo4j_http() {
    local service="Neo4j HTTP"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${NEO4J_HTTP_PORT}" 2>/dev/null || echo "000")
    
    if [[ "${response}" =~ ^(200|201|401)$ ]]; then
        print_result "${service}" "PASS" "HTTP ${response} on port ${NEO4J_HTTP_PORT}"
    elif [[ "${response}" == "000" ]]; then
        print_result "${service}" "FAIL" "Connection refused on port ${NEO4J_HTTP_PORT}"
    else
        print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${NEO4J_HTTP_PORT}"
    fi
}

# =============================================================================
# TEST 4: Neo4j Bolt
# =============================================================================
test_neo4j_bolt() {
    local service="Neo4j Bolt"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    if nc -z localhost "${NEO4J_BOLT_PORT}" 2>/dev/null; then
        print_result "${service}" "PASS" "Port ${NEO4J_BOLT_PORT} listening"
    else
        print_result "${service}" "FAIL" "Port ${NEO4J_BOLT_PORT} not listening"
    fi
}

# =============================================================================
# TEST 5: Qdrant REST API
# =============================================================================
test_qdrant() {
    local service="Qdrant"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    # Check health endpoint
    local response
    response=$(curl -s --connect-timeout "${TIMEOUT}" "http://localhost:${QDRANT_REST_PORT}/healthz" 2>/dev/null || echo "")
    
    if [[ "${response}" =~ "ok" ]] || [[ "${response}" =~ "healthz" ]]; then
        print_result "${service}" "PASS" "Health check passed on port ${QDRANT_REST_PORT}"
    else
        # Try the collections endpoint as fallback
        response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${QDRANT_REST_PORT}/collections" 2>/dev/null || echo "000")
        if [[ "${response}" =~ ^(200|201)$ ]]; then
            print_result "${service}" "PASS" "Collections endpoint accessible on port ${QDRANT_REST_PORT}"
        elif [[ "${response}" == "000" ]]; then
            print_result "${service}" "FAIL" "Connection refused on port ${QDRANT_REST_PORT}"
        else
            print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${QDRANT_REST_PORT}"
        fi
    fi
}

# =============================================================================
# TEST 6: Qdrant gRPC
# =============================================================================
test_qdrant_grpc() {
    local service="Qdrant gRPC"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    if nc -z localhost "${QDRANT_GRPC_PORT}" 2>/dev/null; then
        print_result "${service}" "PASS" "Port ${QDRANT_GRPC_PORT} listening"
    else
        print_result "${service}" "FAIL" "Port ${QDRANT_GRPC_PORT} not listening"
    fi
}

# =============================================================================
# TEST 7: Ollama API
# =============================================================================
test_ollama() {
    local service="Ollama"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s --connect-timeout "${TIMEOUT}" "http://localhost:${OLLAMA_PORT}/api/tags" 2>/dev/null || echo "")
    
    if [[ -n "${response}" ]]; then
        # Check if response is valid JSON with models array
        if echo "${response}" | jq -e '.models' &>/dev/null; then
            print_result "${service}" "PASS" "API responding on port ${OLLAMA_PORT}"
        else
            # Still passes if we got a response (might be empty models)
            print_result "${service}" "PASS" "API responding on port ${OLLAMA_PORT} (may need model pull)"
        fi
    else
        response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${OLLAMA_PORT}/api/version" 2>/dev/null || echo "000")
        if [[ "${response}" =~ ^(200|201)$ ]]; then
            print_result "${service}" "PASS" "API responding on port ${OLLAMA_PORT}"
        elif [[ "${response}" == "000" ]]; then
            print_result "${service}" "FAIL" "Connection refused on port ${OLLAMA_PORT}"
        else
            print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${OLLAMA_PORT}"
        fi
    fi
}

# =============================================================================
# TEST 8: Prometheus
# =============================================================================
test_prometheus() {
    local service="Prometheus"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${PROMETHEUS_PORT}/-/healthy" 2>/dev/null || echo "000")
    
    if [[ "${response}" == "200" ]]; then
        print_result "${service}" "PASS" "Health check passed on port ${PROMETHEUS_PORT}"
    elif [[ "${response}" == "000" ]]; then
        print_result "${service}" "FAIL" "Connection refused on port ${PROMETHEUS_PORT}"
    else
        print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${PROMETHEUS_PORT}"
    fi
}

# =============================================================================
# TEST 9: Grafana
# =============================================================================
test_grafana() {
    local service="Grafana"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${GRAFANA_PORT}/api/health" 2>/dev/null || echo "000")
    
    if [[ "${response}" == "200" ]]; then
        print_result "${service}" "PASS" "Health check passed on port ${GRAFANA_PORT}"
    elif [[ "${response}" == "000" ]]; then
        print_result "${service}" "FAIL" "Connection refused on port ${GRAFANA_PORT}"
    else
        print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${GRAFANA_PORT}"
    fi
}

# =============================================================================
# TEST 10: Node Exporter
# =============================================================================
test_node_exporter() {
    local service="Node Exporter"
    echo -e "\n${YELLOW}Testing ${service}...${NC}"
    
    local response
    response=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout "${TIMEOUT}" "http://localhost:${NODE_EXPORTER_PORT}/metrics" 2>/dev/null || echo "000")
    
    if [[ "${response}" == "200" ]]; then
        print_result "${service}" "PASS" "Metrics endpoint accessible on port ${NODE_EXPORTER_PORT}"
    elif [[ "${response}" == "000" ]]; then
        print_result "${service}" "FAIL" "Connection refused on port ${NODE_EXPORTER_PORT}"
    else
        print_result "${service}" "FAIL" "Unexpected HTTP ${response} on port ${NODE_EXPORTER_PORT}"
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    echo "============================================================"
    echo "rust-brain Smoke Tests: Service Health Checks"
    echo "============================================================"
    echo "Project: ${PROJECT_DIR}"
    echo "Time: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    echo ""
    
    # Check if docker is running
    if ! docker info &>/dev/null; then
        echo -e "${RED}ERROR: Docker is not running${NC}"
        exit 1
    fi
    
    # Check if jq is available (optional, for better JSON parsing)
    if ! command -v jq &>/dev/null; then
        print_warning "jq" "Not installed, some checks may be less detailed"
    fi
    
    # Run all tests
    test_postgres
    test_pgweb
    test_neo4j_http
    test_neo4j_bolt
    test_qdrant
    test_qdrant_grpc
    test_ollama
    test_prometheus
    test_grafana
    test_node_exporter
    
    # Print summary
    echo ""
    echo "============================================================"
    echo "Summary"
    echo "============================================================"
    echo -e "Total:  ${TOTAL}"
    echo -e "Passed: ${GREEN}${PASSED}${NC}"
    echo -e "Failed: ${RED}${FAILED}${NC}"
    echo ""
    
    if [[ ${FAILED} -gt 0 ]]; then
        echo -e "${RED}SMOKE TESTS FAILED${NC}"
        exit 1
    else
        echo -e "${GREEN}ALL SMOKE TESTS PASSED${NC}"
        exit 0
    fi
}

main "$@"
