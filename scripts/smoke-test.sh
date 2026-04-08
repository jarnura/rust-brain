#!/bin/bash
# =============================================================================
# rust-brain — E2E Smoke Test Suite
# =============================================================================
# Validates the full pipeline: database health, graph queries, semantic search,
# MCP connectivity, aggregate search, and API health (including OpenCode).
#
# Usage:  bash scripts/smoke-test.sh
# Exit:   0 = all pass, 1 = any failure
# Target: < 60 seconds
# =============================================================================

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

# Counters
PASSED=0
FAILED=0
TOTAL=0

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
ENV_FILE="${PROJECT_DIR}/.env"

if [[ -f "${ENV_FILE}" ]]; then
    set -a
    source "${ENV_FILE}"
    set +a
fi

API_PORT="${API_PORT:-8088}"
API_URL="http://localhost:${API_PORT}"
MCP_PORT="${MCP_SSE_PORT:-3001}"
MCP_URL="http://localhost:${MCP_PORT}"
TIMEOUT=10

# --- Helpers ----------------------------------------------------------------

report() {
    local label="$1" status="$2" detail="$3"
    TOTAL=$((TOTAL + 1))
    if [[ "${status}" == "PASS" ]]; then
        PASSED=$((PASSED + 1))
        echo -e "  ${GREEN}[PASS]${NC} ${label}: ${detail}"
    else
        FAILED=$((FAILED + 1))
        echo -e "  ${RED}[FAIL]${NC} ${label}: ${detail}"
    fi
}

# --- Check 1: Database Health -----------------------------------------------
# All three stores (Postgres, Neo4j, Qdrant) respond to real queries,
# not just port checks.

check_database_health() {
    echo -e "\n${BOLD}1. Database Health${NC}"

    # Postgres: count rows in extracted_items
    local pg_resp
    pg_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"SELECT count(*) AS cnt FROM extracted_items"}' \
        "${API_URL}/tools/pg_query" 2>/dev/null || echo "")

    if [[ -n "${pg_resp}" ]]; then
        local pg_cnt
        pg_cnt=$(echo "${pg_resp}" | jq -r '.rows[0].cnt // 0' 2>/dev/null || echo "0")
        if [[ "${pg_cnt}" -gt 0 ]]; then
            report "Postgres" "PASS" "${pg_cnt} extracted items"
        else
            report "Postgres" "FAIL" "0 extracted items (empty database?)"
        fi
    else
        report "Postgres" "FAIL" "pg_query endpoint unreachable"
    fi

    # Neo4j: count all nodes
    local neo_resp
    neo_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"MATCH (n) RETURN count(n) AS cnt"}' \
        "${API_URL}/tools/query_graph" 2>/dev/null || echo "")

    if [[ -n "${neo_resp}" ]]; then
        local neo_cnt
        neo_cnt=$(echo "${neo_resp}" | jq -r '.results[0].cnt // 0' 2>/dev/null || echo "0")
        if [[ "${neo_cnt}" -gt 0 ]]; then
            report "Neo4j" "PASS" "${neo_cnt} graph nodes"
        else
            report "Neo4j" "FAIL" "0 graph nodes"
        fi
    else
        report "Neo4j" "FAIL" "query_graph endpoint unreachable"
    fi

    # Qdrant: check via /health dependency status + points count
    local health_resp
    health_resp=$(curl -sf --max-time "${TIMEOUT}" "${API_URL}/health" 2>/dev/null || echo "")

    if [[ -n "${health_resp}" ]]; then
        local qdrant_status qdrant_points
        qdrant_status=$(echo "${health_resp}" | jq -r '.dependencies.qdrant.status // "unknown"' 2>/dev/null || echo "unknown")
        qdrant_points=$(echo "${health_resp}" | jq -r '.dependencies.qdrant.points_count // 0' 2>/dev/null || echo "0")
        if [[ "${qdrant_status}" == "healthy" && "${qdrant_points}" -gt 0 ]]; then
            report "Qdrant" "PASS" "${qdrant_points} vectors, status=${qdrant_status}"
        else
            report "Qdrant" "FAIL" "status=${qdrant_status}, points=${qdrant_points}"
        fi
    else
        report "Qdrant" "FAIL" "/health endpoint unreachable"
    fi
}

# --- Check 2: MCP Tool Invocation ------------------------------------------
# search_code / search_semantic returns >0 results for a known query.

check_mcp_tool_invocation() {
    echo -e "\n${BOLD}2. MCP Tool Invocation (search_semantic)${NC}"

    local resp
    resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"parse function","limit":3}' \
        "${API_URL}/tools/search_semantic" 2>/dev/null || echo "")

    if [[ -n "${resp}" ]]; then
        local total
        total=$(echo "${resp}" | jq -r '.total // 0' 2>/dev/null || echo "0")
        if [[ "${total}" -gt 0 ]]; then
            local first_fqn
            first_fqn=$(echo "${resp}" | jq -r '.results[0].fqn // "unknown"' 2>/dev/null || echo "unknown")
            report "search_semantic" "PASS" "${total} result(s), top: ${first_fqn}"
        else
            report "search_semantic" "FAIL" "0 results for 'parse function'"
        fi
    else
        report "search_semantic" "FAIL" "endpoint unreachable"
    fi
}

# --- Check 3: MCP SSE Connectivity -----------------------------------------
# MCP SSE bridge responds with a session endpoint.

check_mcp_connectivity() {
    echo -e "\n${BOLD}3. MCP SSE Bridge${NC}"

    local resp
    resp=$(curl -sf --max-time 5 "${MCP_URL}/sse" 2>/dev/null | head -5 || echo "")

    if echo "${resp}" | grep -q "endpoint"; then
        local session_path
        session_path=$(echo "${resp}" | grep "^data:" | head -1 | sed 's/^data: //')
        report "MCP-SSE" "PASS" "session: ${session_path}"
    else
        report "MCP-SSE" "FAIL" "no session endpoint returned"
    fi
}

# --- Check 4: Full Query Pipeline (aggregate_search) -----------------------
# "Find all implementations of the Connector trait" returns real code results
# from the cross-DB fan-out (Qdrant + Postgres + Neo4j).

check_full_query_pipeline() {
    echo -e "\n${BOLD}4. Full Query Pipeline (aggregate_search)${NC}"

    local resp
    resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"Find all implementations of the Connector trait","limit":3}' \
        "${API_URL}/tools/aggregate_search" 2>/dev/null || echo "")

    if [[ -n "${resp}" ]]; then
        local total
        total=$(echo "${resp}" | jq -r '.total // 0' 2>/dev/null || echo "0")
        if [[ "${total}" -gt 0 ]]; then
            local first_name first_kind
            first_name=$(echo "${resp}" | jq -r '.results[0].name // "?"' 2>/dev/null || echo "?")
            first_kind=$(echo "${resp}" | jq -r '.results[0].kind // "?"' 2>/dev/null || echo "?")
            report "aggregate_search" "PASS" "${total} result(s), top: ${first_name} (${first_kind})"
        else
            report "aggregate_search" "FAIL" "0 results for Connector trait query"
        fi
    else
        report "aggregate_search" "FAIL" "endpoint unreachable"
    fi
}

# --- Check 5: Graph Query --------------------------------------------------
# MATCH (n:Trait) RETURN count(n) > 0

check_graph_query() {
    echo -e "\n${BOLD}5. Graph Query (Trait node count)${NC}"

    local resp
    resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"MATCH (n:Trait) RETURN count(n) AS cnt"}' \
        "${API_URL}/tools/query_graph" 2>/dev/null || echo "")

    if [[ -n "${resp}" ]]; then
        local cnt
        cnt=$(echo "${resp}" | jq -r '.results[0].cnt // 0' 2>/dev/null || echo "0")
        if [[ "${cnt}" -gt 0 ]]; then
            report "Trait count" "PASS" "${cnt} Trait nodes in graph"
        else
            report "Trait count" "FAIL" "0 Trait nodes (graph empty?)"
        fi
    else
        report "Trait count" "FAIL" "query_graph endpoint unreachable"
    fi
}

# --- Check 6: Provider / OpenCode Health ------------------------------------
# Verifies OpenCode is reachable and all API dependencies are healthy.

check_provider_connectivity() {
    echo -e "\n${BOLD}6. Provider Connectivity (API + OpenCode)${NC}"

    local health_resp
    health_resp=$(curl -sf --max-time "${TIMEOUT}" "${API_URL}/health" 2>/dev/null || echo "")

    if [[ -z "${health_resp}" ]]; then
        report "API /health" "FAIL" "endpoint unreachable"
        return
    fi

    local api_status
    api_status=$(echo "${health_resp}" | jq -r '.status // "unknown"' 2>/dev/null || echo "unknown")

    if [[ "${api_status}" == "healthy" ]]; then
        report "API" "PASS" "status=${api_status}"
    else
        report "API" "FAIL" "status=${api_status}"
    fi

    # OpenCode dependency
    local oc_status oc_error
    oc_status=$(echo "${health_resp}" | jq -r '.dependencies.opencode.status // "missing"' 2>/dev/null || echo "missing")
    oc_error=$(echo "${health_resp}" | jq -r '.dependencies.opencode.error // null' 2>/dev/null || echo "null")

    if [[ "${oc_status}" == "healthy" ]]; then
        report "OpenCode" "PASS" "status=${oc_status}"
    else
        report "OpenCode" "FAIL" "status=${oc_status}, error=${oc_error}"
    fi
}

# --- Main -------------------------------------------------------------------

main() {
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║         rust-brain — E2E Smoke Test Suite                   ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""
    echo "API:  ${API_URL}"
    echo "MCP:  ${MCP_URL}"
    echo "Time: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"

    local START_TIME
    START_TIME=$(date +%s)

    check_database_health
    check_mcp_tool_invocation
    check_mcp_connectivity
    check_full_query_pipeline
    check_graph_query
    check_provider_connectivity

    local END_TIME DURATION
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))

    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo -e "║  Total: ${TOTAL}  |  Passed: ${GREEN}${PASSED}${NC}  |  Failed: ${RED}${FAILED}${NC}  |  ${DURATION}s        ║"

    if [[ ${FAILED} -eq 0 ]]; then
        echo -e "║  ${GREEN}ALL CHECKS PASSED${NC}                                          ║"
    else
        echo -e "║  ${RED}${FAILED} CHECK(S) FAILED${NC}                                          ║"
    fi

    echo "╚══════════════════════════════════════════════════════════════╝"

    exit "${FAILED}"
}

main "$@"
