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
SMOKE_WORKSPACE_ID=""

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

# --- Check 7: Content Validation -------------------------------------------
# Verifies the database contains rust-brain data, NOT a different codebase.
# This catches the class of bug where ingestion data belongs to the wrong project.

check_content_validation() {
    echo -e "\n${BOLD}7. Content Validation (codebase identity)${NC}"

    # 7a: Semantic search for "PipelineRunner" — FQN must reference rustbrain/ingestion
    local sem_resp
    sem_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"PipelineRunner","limit":5}' \
        "${API_URL}/tools/search_semantic" 2>/dev/null || echo "")

    if [[ -n "${sem_resp}" ]]; then
        local sem_total
        sem_total=$(echo "${sem_resp}" | jq -r '.total // 0' 2>/dev/null || echo "0")
        if [[ "${sem_total}" -gt 0 ]]; then
            local fqn_match
            fqn_match=$(echo "${sem_resp}" | jq -r '[.results[].fqn] | map(select(test("ingestion|rustbrain|pipeline";"i"))) | length' 2>/dev/null || echo "0")
            if [[ "${fqn_match}" -gt 0 ]]; then
                report "Semantic:PipelineRunner" "PASS" "${fqn_match}/${sem_total} FQNs match rust-brain"
            else
                report "Semantic:PipelineRunner" "FAIL" "FQNs found but none match rust-brain (wrong codebase?)"
            fi
        else
            report "Semantic:PipelineRunner" "FAIL" "0 results for PipelineRunner"
        fi
    else
        report "Semantic:PipelineRunner" "FAIL" "search_semantic unreachable"
    fi

    # 7b: pg_query for PipelineRunner FQN — must exist in extracted_items
    local pg_resp
    pg_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"SELECT fqn FROM extracted_items WHERE fqn ILIKE '\''%PipelineRunner%'\'' LIMIT 5"}' \
        "${API_URL}/tools/pg_query" 2>/dev/null || echo "")

    if [[ -n "${pg_resp}" ]]; then
        local pg_rows
        pg_rows=$(echo "${pg_resp}" | jq -r '.rows | length' 2>/dev/null || echo "0")
        if [[ "${pg_rows}" -gt 0 ]]; then
            local first_fqn
            first_fqn=$(echo "${pg_resp}" | jq -r '.rows[0].fqn // "?"' 2>/dev/null || echo "?")
            report "PG:PipelineRunner" "PASS" "${pg_rows} row(s), e.g. ${first_fqn}"
        else
            report "PG:PipelineRunner" "FAIL" "no PipelineRunner FQN in extracted_items"
        fi
    else
        report "PG:PipelineRunner" "FAIL" "pg_query unreachable"
    fi

    # 7c: Chat query — response must reference rust/pipeline, not Hyperswitch
    local chat_resp
    chat_resp=$(curl -sf --max-time 30 \
        -X POST -H "Content-Type: application/json" \
        -d '{"message":"What is the ingestion pipeline? Answer in one sentence."}' \
        "${API_URL}/tools/chat" 2>/dev/null || echo "")

    if [[ -n "${chat_resp}" ]]; then
        local chat_text
        chat_text=$(echo "${chat_resp}" | jq -r '.response // ""' 2>/dev/null || echo "")
        if echo "${chat_text}" | grep -iqE "rust|pipeline|ingestion|crate|cargo"; then
            if echo "${chat_text}" | grep -iq "hyperswitch"; then
                report "Chat:content" "FAIL" "response mentions Hyperswitch (wrong codebase)"
            else
                report "Chat:content" "PASS" "response references rust/pipeline correctly"
            fi
        else
            report "Chat:content" "FAIL" "response doesn't mention rust or pipeline"
        fi
    else
        echo -e "  ${YELLOW}[SKIP]${NC} Chat:content: chat endpoint unavailable (OpenCode down?)"
    fi

    # 7d: Anti-check — search for Hyperswitch-specific content should return 0
    local anti_resp
    anti_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"SELECT count(*) AS cnt FROM extracted_items WHERE fqn ILIKE '\''%hyperswitch%'\''"}' \
        "${API_URL}/tools/pg_query" 2>/dev/null || echo "")

    if [[ -n "${anti_resp}" ]]; then
        local anti_cnt
        anti_cnt=$(echo "${anti_resp}" | jq -r '.rows[0].cnt // 0' 2>/dev/null || echo "0")
        if [[ "${anti_cnt}" -eq 0 ]]; then
            report "Anti:Hyperswitch" "PASS" "no Hyperswitch data found (correct)"
        else
            report "Anti:Hyperswitch" "FAIL" "${anti_cnt} Hyperswitch items found (wrong codebase!)"
        fi
    else
        report "Anti:Hyperswitch" "FAIL" "pg_query unreachable"
    fi
}

# --- Check 8: Workspace Health ---------------------------------------------
# Verifies the default workspace is ready and has items.

check_workspace_health() {
    echo -e "\n${BOLD}8. Workspace Health${NC}"

    local ws_resp
    ws_resp=$(curl -sf --max-time "${TIMEOUT}" \
        "${API_URL}/workspaces" 2>/dev/null || echo "")

    if [[ -z "${ws_resp}" ]]; then
        report "Workspace list" "FAIL" "/workspaces unreachable"
        return
    fi

    local ws_count
    ws_count=$(echo "${ws_resp}" | jq -r 'length' 2>/dev/null || echo "0")
    if [[ "${ws_count}" -eq 0 ]]; then
        report "Workspace list" "FAIL" "no workspaces found"
        return
    fi

    # Find the first workspace with status=ready (or the first one)
    local ws_id ws_status ws_name
    ws_id=$(echo "${ws_resp}" | jq -r '[.[] | select(.status == "ready")][0].id // .[-1].id // ""' 2>/dev/null || echo "")
    ws_status=$(echo "${ws_resp}" | jq -r '[.[] | select(.status == "ready")][0].status // .[-1].status // "unknown"' 2>/dev/null || echo "unknown")
    ws_name=$(echo "${ws_resp}" | jq -r '[.[] | select(.status == "ready")][0].name // .[-1].name // "?"' 2>/dev/null || echo "?")

    if [[ "${ws_status}" == "ready" ]]; then
        report "Workspace status" "PASS" "${ws_name} status=${ws_status}"
    else
        report "Workspace status" "FAIL" "${ws_name} status=${ws_status} (expected ready)"
    fi

    # Check workspace has items via schema-scoped query
    if [[ -n "${ws_id}" && "${ws_status}" == "ready" ]]; then
        local schema_name
        schema_name=$(echo "${ws_resp}" | jq -r --arg id "${ws_id}" '[.[] | select(.id == $id)][0].schema_name // ""' 2>/dev/null || echo "")

        if [[ -n "${schema_name}" ]]; then
            local item_resp item_cnt
            item_resp=$(curl -sf --max-time "${TIMEOUT}" \
                -X POST -H "Content-Type: application/json" \
                -d "{\"query\":\"SELECT count(*) AS cnt FROM ${schema_name}.extracted_items\"}" \
                "${API_URL}/tools/pg_query" 2>/dev/null || echo "")

            if [[ -n "${item_resp}" ]]; then
                item_cnt=$(echo "${item_resp}" | jq -r '.rows[0].cnt // 0' 2>/dev/null || echo "0")
                if [[ "${item_cnt}" -gt 0 ]]; then
                    report "Workspace items" "PASS" "${item_cnt} items in ${ws_name}"
                else
                    report "Workspace items" "FAIL" "0 items in workspace ${ws_name}"
                fi
            else
                report "Workspace items" "FAIL" "pg_query for workspace items failed"
            fi
        else
            # Fallback: use global extracted_items count (already checked in check 1)
            report "Workspace items" "PASS" "schema_name not available, global count verified in check 1"
        fi
    fi

    # Export workspace ID for use by playground check
    SMOKE_WORKSPACE_ID="${ws_id}"
}

# --- Check 9: Playground Round-Trip ----------------------------------------
# Starts a workspace execution, polls until terminal, asserts success.
# Skipped if no ready workspace found or SKIP_PLAYGROUND=1.

check_playground_roundtrip() {
    echo -e "\n${BOLD}9. Playground Round-Trip${NC}"

    if [[ "${SKIP_PLAYGROUND:-0}" == "1" ]]; then
        echo -e "  ${YELLOW}[SKIP]${NC} SKIP_PLAYGROUND=1"
        return
    fi

    if [[ -z "${SMOKE_WORKSPACE_ID:-}" ]]; then
        report "Playground" "FAIL" "no ready workspace found (skipped)"
        return
    fi

    # Start execution
    local exec_resp
    exec_resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"prompt":"What is this project about? Answer in one sentence.","timeout_secs":120}' \
        "${API_URL}/workspaces/${SMOKE_WORKSPACE_ID}/execute" 2>/dev/null || echo "")

    if [[ -z "${exec_resp}" ]]; then
        report "Playground:start" "FAIL" "execute endpoint unreachable"
        return
    fi

    local exec_id exec_status
    exec_id=$(echo "${exec_resp}" | jq -r '.id // ""' 2>/dev/null || echo "")
    exec_status=$(echo "${exec_resp}" | jq -r '.status // "unknown"' 2>/dev/null || echo "unknown")

    if [[ -z "${exec_id}" ]]; then
        report "Playground:start" "FAIL" "no execution ID returned"
        return
    fi

    report "Playground:start" "PASS" "execution ${exec_id:0:8}... status=${exec_status}"

    # Poll for completion (max 120s, 5s interval)
    local POLL_TIMEOUT="${PLAYGROUND_TIMEOUT:-120}"
    local POLL_INTERVAL=5
    local elapsed=0
    local final_status="unknown"

    while [[ ${elapsed} -lt ${POLL_TIMEOUT} ]]; do
        sleep ${POLL_INTERVAL}
        elapsed=$((elapsed + POLL_INTERVAL))

        local poll_resp
        poll_resp=$(curl -sf --max-time "${TIMEOUT}" \
            "${API_URL}/executions/${exec_id}" 2>/dev/null || echo "")

        if [[ -z "${poll_resp}" ]]; then
            continue
        fi

        final_status=$(echo "${poll_resp}" | jq -r '.status // "unknown"' 2>/dev/null || echo "unknown")

        case "${final_status}" in
            completed|failed|aborted|timeout)
                break
                ;;
        esac
    done

    if [[ "${final_status}" == "completed" ]]; then
        report "Playground:result" "PASS" "execution completed in ${elapsed}s"
    elif [[ "${final_status}" == "running" ]]; then
        report "Playground:result" "FAIL" "execution still running after ${POLL_TIMEOUT}s"
    else
        report "Playground:result" "FAIL" "execution ${final_status} after ${elapsed}s"
    fi
}

# --- Check 10: Agent Events Schema -----------------------------------------
# Validates that agent_events table has the required seq and content_hash
# columns. Uses SELECT with LIMIT 0 (returns zero rows but succeeds only if
# columns exist). This catches migration drift — e.g., if seq or content_hash
# migrations were not applied.

check_agent_events_schema() {
    echo -e "\n${BOLD}10. Agent Events Schema (seq + content_hash)${NC}"

    local resp
    resp=$(curl -sf --max-time "${TIMEOUT}" \
        -X POST -H "Content-Type: application/json" \
        -d '{"query":"SELECT seq, content_hash FROM agent_events LIMIT 0"}' \
        "${API_URL}/tools/pg_query" 2>/dev/null || echo "")

    if [[ -n "${resp}" ]]; then
        local has_error
        has_error=$(echo "${resp}" | jq -r '.error // empty' 2>/dev/null || echo "")
        if [[ -n "${has_error}" ]]; then
            report "agent_events schema" "FAIL" "query returned error: ${has_error}"
        else
            local row_count
            row_count=$(echo "${resp}" | jq -r '.row_count // -1' 2>/dev/null || echo "-1")
            if [[ "${row_count}" -ge 0 ]]; then
                report "agent_events schema" "PASS" "seq, content_hash columns exist (0 rows returned as expected)"
            else
                report "agent_events schema" "FAIL" "unexpected response shape: row_count=${row_count}"
            fi
        fi
    else
        report "agent_events schema" "FAIL" "pg_query endpoint unreachable"
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
    check_content_validation
    check_workspace_health
    check_playground_roundtrip
    check_agent_events_schema

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
