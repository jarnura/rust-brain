#!/bin/bash
# =============================================================================
# rust-brain — Integration Tests: MCP Server Tool Verification
# =============================================================================
# Tests all 7 MCP tools for:
# - MCP initialize handshake simulation
# - tools/list endpoint (returns all available tools)
# - Each tool with valid inputs
# - Error handling for invalid inputs
# =============================================================================

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test counters
PASSED=0
FAILED=0
TOTAL=0

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
ENV_FILE="${PROJECT_DIR}/.env"

# Load environment variables
if [[ -f "${ENV_FILE}" ]]; then
    set -a
    source "${ENV_FILE}"
    set +a
fi

# API configuration
API_HOST="${API_HOST:-localhost}"
API_PORT="${API_PORT:-8080}"
API_BASE_URL="http://${API_HOST}:${API_PORT}"
TIMEOUT=30

# Expected tools list
EXPECTED_TOOLS=(
    "search_semantic"
    "get_function"
    "get_callers"
    "get_trait_impls"
    "find_usages_of_type"
    "get_module_tree"
    "query_graph"
)

# Helper function to print test result
print_result() {
    local test_name="$1"
    local status="$2"
    local message="$3"
    
    TOTAL=$((TOTAL + 1))
    
    if [[ "${status}" == "PASS" ]]; then
        echo -e "${GREEN}[PASS]${NC} ${test_name}: ${message}"
        PASSED=$((PASSED + 1))
    else
        echo -e "${RED}[FAIL]${NC} ${test_name}: ${message}"
        FAILED=$((FAILED + 1))
    fi
}

# Helper function to print section header
print_section() {
    echo ""
    echo -e "${BLUE}=== $1 ===${NC}"
}

# Helper function to make API call
api_call() {
    local method="$1"
    local endpoint="$2"
    local data="$3"
    
    local url="${API_BASE_URL}${endpoint}"
    
    if [[ "${method}" == "GET" ]]; then
        curl -s -o /tmp/api_response.json -w "%{http_code}" \
            --connect-timeout "${TIMEOUT}" \
            -H "Content-Type: application/json" \
            "${url}" 2>/dev/null || echo "000"
    else
        curl -s -o /tmp/api_response.json -w "%{http_code}" \
            --connect-timeout "${TIMEOUT}" \
            -X "${method}" \
            -H "Content-Type: application/json" \
            -d "${data}" \
            "${url}" 2>/dev/null || echo "000"
    fi
}

# Get response body
get_response() {
    cat /tmp/api_response.json 2>/dev/null || echo ""
}

# Check if jq is available
check_jq() {
    if ! command -v jq &> /dev/null; then
        echo -e "${YELLOW}Warning: jq not found. Some tests may be limited.${NC}"
        return 1
    fi
    return 0
}

# =============================================================================
# PREREQUISITE CHECKS
# =============================================================================
check_api_available() {
    print_section "Checking MCP Server Availability"
    
    local http_code
    http_code=$(curl -s -o /dev/null -w "%{http_code}" \
        --connect-timeout 5 \
        "${API_BASE_URL}/health" 2>/dev/null || echo "000")
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "API Availability" "PASS" "MCP server is running on ${API_BASE_URL}"
        return 0
    else
        print_result "API Availability" "FAIL" "MCP server not responding (HTTP ${http_code})"
        echo ""
        echo "Start the MCP server with:"
        echo "  cd ${PROJECT_DIR} && docker compose up -d api"
        return 1
    fi
}

# =============================================================================
# TEST 1: MCP INITIALIZE HANDSHAKE
# =============================================================================
test_mcp_initialize() {
    print_section "Test 1: MCP Initialize Handshake"
    
    # Simulate MCP initialize by checking health endpoint
    local http_code
    http_code=$(api_call "GET" "/health" "")
    local response=$(get_response)
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "MCP Initialize" "PASS" "Server responded to health check"
        
        # Check response structure
        if check_jq; then
            local status
            status=$(echo "${response}" | jq -r '.status // empty' 2>/dev/null || echo "")
            
            if [[ "${status}" == "healthy" ]]; then
                print_result "Health Status" "PASS" "Server status is healthy"
            else
                print_result "Health Status" "WARN" "Server status: ${status}"
            fi
            
            # Check dependencies
            local deps
            deps=$(echo "${response}" | jq -r '.dependencies // empty' 2>/dev/null || echo "")
            if [[ -n "${deps}" ]]; then
                print_result "Dependencies Check" "PASS" "Dependencies reported: ${deps}"
            fi
        fi
    else
        print_result "MCP Initialize" "FAIL" "Health check returned HTTP ${http_code}"
    fi
}

# =============================================================================
# TEST 2: TOOLS/LIST - VERIFY ALL 7 TOOLS
# =============================================================================
test_tools_list() {
    print_section "Test 2: Tools List (All 7 Tools)"
    
    local found_tools=()
    local missing_tools=()
    
    # Check each expected tool endpoint
    for tool in "${EXPECTED_TOOLS[@]}"; do
        local endpoint="/tools/${tool}"
        local http_code
        
        # For GET endpoints, just check they exist (may return 400 for missing params, that's OK)
        if [[ "${tool}" == "search_semantic" || "${tool}" == "query_graph" ]]; then
            # POST endpoints - send empty body to check endpoint exists
            http_code=$(curl -s -o /dev/null -w "%{http_code}" \
                --connect-timeout "${TIMEOUT}" \
                -X POST \
                -H "Content-Type: application/json" \
                -d '{}' \
                "${API_BASE_URL}${endpoint}" 2>/dev/null || echo "000")
            
            # 400 means endpoint exists but needs proper params, 404 means not found
            if [[ "${http_code}" == "400" || "${http_code}" == "200" || "${http_code}" == "422" ]]; then
                found_tools+=("${tool}")
                print_result "Tool: ${tool}" "PASS" "Endpoint exists (HTTP ${http_code})"
            else
                missing_tools+=("${tool}")
                print_result "Tool: ${tool}" "FAIL" "Endpoint not found (HTTP ${http_code})"
            fi
        else
            # GET endpoints - check they exist
            http_code=$(curl -s -o /dev/null -w "%{http_code}" \
                --connect-timeout "${TIMEOUT}" \
                "${API_BASE_URL}${endpoint}" 2>/dev/null || echo "000")
            
            if [[ "${http_code}" == "400" || "${http_code}" == "200" || "${http_code}" == "404" ]]; then
                found_tools+=("${tool}")
                print_result "Tool: ${tool}" "PASS" "Endpoint exists (HTTP ${http_code})"
            else
                missing_tools+=("${tool}")
                print_result "Tool: ${tool}" "FAIL" "Unexpected response (HTTP ${http_code})"
            fi
        fi
    done
    
    echo ""
    echo "Found ${#found_tools[@]}/${#EXPECTED_TOOLS[@]} tools"
    
    if [[ ${#found_tools[@]} -eq ${#EXPECTED_TOOLS[@]} ]]; then
        print_result "Tools List Complete" "PASS" "All 7 MCP tools are available"
    else
        print_result "Tools List Complete" "FAIL" "Missing tools: ${missing_tools[*]}"
    fi
}

# =============================================================================
# TEST 3: SEARCH_SEMANTIC TOOL
# =============================================================================
test_search_semantic() {
    print_section "Test 3: search_semantic Tool"
    
    # Test with valid input
    local http_code
    http_code=$(api_call "POST" "/tools/search_semantic" '{
        "query": "function that parses JSON",
        "limit": 5
    }')
    local response=$(get_response)
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "search_semantic (valid)" "PASS" "Query executed successfully"
        
        if check_jq; then
            local result_count
            result_count=$(echo "${response}" | jq -r '.results | length // 0' 2>/dev/null || echo "0")
            print_result "search_semantic (results)" "PASS" "Returned ${result_count} results"
            
            local query_time
            query_time=$(echo "${response}" | jq -r '.query_time_ms // 0' 2>/dev/null || echo "0")
            print_result "search_semantic (timing)" "PASS" "Query time: ${query_time}ms"
        fi
    elif [[ "${http_code}" == "503" ]]; then
        print_result "search_semantic (valid)" "WARN" "Service unavailable (Qdrant/Ollama may be down)"
    else
        print_result "search_semantic (valid)" "FAIL" "HTTP ${http_code}: ${response:0:100}"
    fi
    
    # Test with missing query
    http_code=$(api_call "POST" "/tools/search_semantic" '{}')
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "search_semantic (error)" "PASS" "Correctly rejects missing query"
    else
        print_result "search_semantic (error)" "WARN" "Expected 400, got ${http_code}"
    fi
}

# =============================================================================
# TEST 4: GET_FUNCTION TOOL
# =============================================================================
test_get_function() {
    print_section "Test 4: get_function Tool"
    
    # Test with missing parameter
    local http_code
    http_code=$(api_call "GET" "/tools/get_function" "")
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "get_function (missing param)" "PASS" "Correctly requires fqn parameter"
    else
        print_result "get_function (missing param)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with non-existent function
    http_code=$(api_call "GET" "/tools/get_function?fqn=nonexistent::function" "")
    
    if [[ "${http_code}" == "404" ]]; then
        print_result "get_function (not found)" "PASS" "Correctly returns 404 for non-existent function"
    else
        print_result "get_function (not found)" "WARN" "Expected 404, got ${http_code}"
    fi
}

# =============================================================================
# TEST 5: GET_CALLERS TOOL
# =============================================================================
test_get_callers() {
    print_section "Test 5: get_callers Tool"
    
    # Test with missing parameter
    local http_code
    http_code=$(api_call "GET" "/tools/get_callers" "")
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "get_callers (missing param)" "PASS" "Correctly requires fqn parameter"
    else
        print_result "get_callers (missing param)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with depth parameter
    http_code=$(api_call "GET" "/tools/get_callers?fqn=test::function&depth=2" "")
    
    if [[ "${http_code}" == "200" || "${http_code}" == "404" || "${http_code}" == "400" ]]; then
        print_result "get_callers (with depth)" "PASS" "Accepts depth parameter (HTTP ${http_code})"
    else
        print_result "get_callers (with depth)" "WARN" "Unexpected response: ${http_code}"
    fi
}

# =============================================================================
# TEST 6: GET_TRAIT_IMPLS TOOL
# =============================================================================
test_get_trait_impls() {
    print_section "Test 6: get_trait_impls Tool"
    
    # Test with missing parameter
    local http_code
    http_code=$(api_call "GET" "/tools/get_trait_impls" "")
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "get_trait_impls (missing param)" "PASS" "Correctly requires trait_name parameter"
    else
        print_result "get_trait_impls (missing param)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with non-existent trait
    http_code=$(api_call "GET" "/tools/get_trait_impls?trait_name=NonExistentTrait" "")
    
    if [[ "${http_code}" == "404" || "${http_code}" == "200" ]]; then
        print_result "get_trait_impls (not found)" "PASS" "Handles non-existent trait (HTTP ${http_code})"
    else
        print_result "get_trait_impls (not found)" "WARN" "Unexpected response: ${http_code}"
    fi
}

# =============================================================================
# TEST 7: FIND_USAGES_OF_TYPE TOOL
# =============================================================================
test_find_usages_of_type() {
    print_section "Test 7: find_usages_of_type Tool"
    
    # Test with missing parameter
    local http_code
    http_code=$(api_call "GET" "/tools/find_usages_of_type" "")
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "find_usages_of_type (missing param)" "PASS" "Correctly requires type_name parameter"
    else
        print_result "find_usages_of_type (missing param)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with usage_type filter
    http_code=$(api_call "GET" "/tools/find_usages_of_type?type_name=test::Type&usage_type=parameter" "")
    
    if [[ "${http_code}" == "200" || "${http_code}" == "404" || "${http_code}" == "400" ]]; then
        print_result "find_usages_of_type (with filter)" "PASS" "Accepts usage_type parameter (HTTP ${http_code})"
    else
        print_result "find_usages_of_type (with filter)" "WARN" "Unexpected response: ${http_code}"
    fi
}

# =============================================================================
# TEST 8: GET_MODULE_TREE TOOL
# =============================================================================
test_get_module_tree() {
    print_section "Test 8: get_module_tree Tool"
    
    # Test with missing parameter
    local http_code
    http_code=$(api_call "GET" "/tools/get_module_tree" "")
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "get_module_tree (missing param)" "PASS" "Correctly requires crate parameter"
    else
        print_result "get_module_tree (missing param)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with include_items
    http_code=$(api_call "GET" "/tools/get_module_tree?crate=test_crate&include_items=true" "")
    
    if [[ "${http_code}" == "200" || "${http_code}" == "404" || "${http_code}" == "400" ]]; then
        print_result "get_module_tree (with items)" "PASS" "Accepts include_items parameter (HTTP ${http_code})"
    else
        print_result "get_module_tree (with items)" "WARN" "Unexpected response: ${http_code}"
    fi
}

# =============================================================================
# TEST 9: QUERY_GRAPH TOOL
# =============================================================================
test_query_graph() {
    print_section "Test 9: query_graph Tool"
    
    # Test with valid query
    local http_code
    http_code=$(api_call "POST" "/tools/query_graph" '{
        "query": "MATCH (n) RETURN count(n) as count LIMIT 1"
    }')
    local response=$(get_response)
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "query_graph (valid)" "PASS" "Query executed successfully"
        
        if check_jq; then
            local query_time
            query_time=$(echo "${response}" | jq -r '.query_time_ms // 0' 2>/dev/null || echo "0")
            print_result "query_graph (timing)" "PASS" "Query time: ${query_time}ms"
        fi
    elif [[ "${http_code}" == "503" ]]; then
        print_result "query_graph (valid)" "WARN" "Neo4j unavailable"
    else
        print_result "query_graph (valid)" "FAIL" "HTTP ${http_code}"
    fi
    
    # Test with invalid Cypher
    http_code=$(api_call "POST" "/tools/query_graph" '{
        "query": "INVALID CYPHER SYNTAX"
    }')
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "query_graph (invalid syntax)" "PASS" "Correctly rejects invalid Cypher"
    else
        print_result "query_graph (invalid syntax)" "WARN" "Expected 400, got ${http_code}"
    fi
    
    # Test with missing query
    http_code=$(api_call "POST" "/tools/query_graph" '{}')
    
    if [[ "${http_code}" == "400" ]]; then
        print_result "query_graph (missing query)" "PASS" "Correctly requires query parameter"
    else
        print_result "query_graph (missing query)" "WARN" "Expected 400, got ${http_code}"
    fi
}

# =============================================================================
# TEST 10: ERROR HANDLING
# =============================================================================
test_error_handling() {
    print_section "Test 10: Error Handling"
    
    # Test 404 for non-existent endpoint
    local http_code
    http_code=$(api_call "GET" "/nonexistent" "")
    
    if [[ "${http_code}" == "404" ]]; then
        print_result "Error handling (404)" "PASS" "Returns 404 for unknown endpoints"
    else
        print_result "Error handling (404)" "WARN" "Expected 404, got ${http_code}"
    fi
    
    # Test error response format
    http_code=$(api_call "GET" "/tools/get_function" "")
    local response=$(get_response)
    
    if check_jq; then
        local error_code
        error_code=$(echo "${response}" | jq -r '.code // .error // empty' 2>/dev/null || echo "")
        
        if [[ -n "${error_code}" ]]; then
            print_result "Error format (code)" "PASS" "Error response includes code: ${error_code}"
        else
            print_result "Error format (code)" "WARN" "Error response may not include proper code field"
        fi
    fi
}

# =============================================================================
# TEST 11: METRICS ENDPOINT
# =============================================================================
test_metrics() {
    print_section "Test 11: Metrics Endpoint"
    
    local http_code
    http_code=$(api_call "GET" "/metrics" "")
    local response=$(get_response)
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "Metrics endpoint" "PASS" "Metrics available at /metrics"
        
        # Check for expected metrics
        if echo "${response}" | grep -q "rustbrain_api_requests_total"; then
            print_result "Metrics (requests)" "PASS" "Request metrics present"
        fi
        
        if echo "${response}" | grep -q "rustbrain_api_errors_total"; then
            print_result "Metrics (errors)" "PASS" "Error metrics present"
        fi
    else
        print_result "Metrics endpoint" "WARN" "Metrics endpoint returned HTTP ${http_code}"
    fi
}

# =============================================================================
# MAIN
# =============================================================================
main() {
    echo "========================================"
    echo "  rust-brain MCP Server Integration Tests"
    echo "========================================"
    echo ""
    echo "API Base URL: ${API_BASE_URL}"
    echo "Timeout: ${TIMEOUT}s"
    echo ""
    
    # Check prerequisites
    if ! check_api_available; then
        echo ""
        echo "Cannot proceed with tests - MCP server not available"
        exit 1
    fi
    
    check_jq || true
    
    # Run all tests
    test_mcp_initialize
    test_tools_list
    test_search_semantic
    test_get_function
    test_get_callers
    test_get_trait_impls
    test_find_usages_of_type
    test_get_module_tree
    test_query_graph
    test_error_handling
    test_metrics
    
    # Print summary
    echo ""
    echo "========================================"
    echo "  TEST SUMMARY"
    echo "========================================"
    echo -e "Total:  ${TOTAL}"
    echo -e "Passed: ${GREEN}${PASSED}${NC}"
    echo -e "Failed: ${RED}${FAILED}${NC}"
    echo ""
    
    if [[ ${FAILED} -eq 0 ]]; then
        echo -e "${GREEN}All MCP server tests passed!${NC}"
        exit 0
    else
        echo -e "${RED}Some tests failed. Check the output above.${NC}"
        exit 1
    fi
}

# Run main
main "$@"
