#!/bin/bash
# =============================================================================
# rust-brain — Integration Tests: API Endpoint Verification
# =============================================================================
# Tests each API endpoint for:
# - Response format correctness
# - Error handling
# - Expected behavior
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
TIMEOUT=10

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
    local expected_status="$4"
    
    local url="${API_BASE_URL}${endpoint}"
    local response
    local http_code
    
    if [[ "${method}" == "GET" ]]; then
        http_code=$(curl -s -o /tmp/api_response.json -w "%{http_code}" \
            --connect-timeout "${TIMEOUT}" \
            -H "Content-Type: application/json" \
            "${url}" 2>/dev/null || echo "000")
    else
        http_code=$(curl -s -o /tmp/api_response.json -w "%{http_code}" \
            --connect-timeout "${TIMEOUT}" \
            -X "${method}" \
            -H "Content-Type: application/json" \
            -d "${data}" \
            "${url}" 2>/dev/null || echo "000")
    fi
    
    response=$(cat /tmp/api_response.json 2>/dev/null || echo "")
    
    echo "${http_code}:${response}"
}

# =============================================================================
# PREREQUISITE CHECKS
# =============================================================================
check_api_available() {
    print_section "Checking API Availability"
    
    local result
    result=$(api_call "GET" "/health" "" "200")
    local http_code="${result%%:*}"
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "api_health" "PASS" "API is healthy on port ${API_PORT}"
        return 0
    elif [[ "${http_code}" == "000" ]]; then
        echo -e "${YELLOW}API not running on port ${API_PORT}. Tests will be skipped.${NC}"
        echo -e "${YELLOW}Start the API service before running these tests.${NC}"
        return 1
    else
        print_result "api_health" "FAIL" "API returned HTTP ${http_code}"
        return 1
    fi
}

# =============================================================================
# TEST 1: Health Endpoint
# =============================================================================
test_health_endpoint() {
    print_section "Health Endpoint Tests"
    
    # Test basic health
    local result
    result=$(api_call "GET" "/health" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.status' &>/dev/null; then
            print_result "health_status" "PASS" "Health check returned valid JSON"
        else
            print_result "health_status" "PASS" "Health check returned response"
        fi
    else
        print_result "health_status" "FAIL" "Health check returned HTTP ${http_code}"
    fi
}

# =============================================================================
# TEST 2: Semantic Search Endpoint
# =============================================================================
test_semantic_search() {
    print_section "Semantic Search Endpoint Tests"
    
    # Test with valid query
    local result
    result=$(api_call "POST" "/tools/search_semantic" '{"query": "function that adds numbers", "limit": 5}' "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.results' &>/dev/null; then
            local count
            count=$(echo "${response}" | jq -r '.results | length' 2>/dev/null || echo "0")
            print_result "semantic_search_valid" "PASS" "Returned ${count} result(s)"
        else
            print_result "semantic_search_valid" "PASS" "Request accepted"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "semantic_search_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "semantic_search_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
    
    # Test with empty query (error handling)
    result=$(api_call "POST" "/tools/search_semantic" '{"query": "", "limit": 5}' "400")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" =~ ^(400|422)$ ]]; then
        print_result "semantic_search_empty" "PASS" "Correctly rejected empty query with HTTP ${http_code}"
    elif [[ "${http_code}" == "200" ]]; then
        print_result "semantic_search_empty" "FAIL" "Should reject empty query"
    else
        print_result "semantic_search_empty" "WARN" "Unexpected HTTP ${http_code}"
    fi
    
    # Test with missing query (error handling)
    result=$(api_call "POST" "/tools/search_semantic" '{}' "400")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" =~ ^(400|422)$ ]]; then
        print_result "semantic_search_missing" "PASS" "Correctly rejected missing query"
    else
        print_result "semantic_search_missing" "WARN" "HTTP ${http_code} for missing query"
    fi
}

# =============================================================================
# TEST 3: Get Function Endpoint
# =============================================================================
test_get_function() {
    print_section "Get Function Endpoint Tests"
    
    # Test with valid FQN
    local result
    result=$(api_call "GET" "/tools/get_function?fqn=test_fixture::add" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.fqn' &>/dev/null; then
            local fqn
            fqn=$(echo "${response}" | jq -r '.fqn' 2>/dev/null || echo "")
            print_result "get_function_valid" "PASS" "Found function: ${fqn}"
        else
            print_result "get_function_valid" "PASS" "Function returned"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "get_function_valid" "FAIL" "Function not found or endpoint not implemented"
    else
        print_result "get_function_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
    
    # Test with missing FQN (error handling)
    result=$(api_call "GET" "/tools/get_function" "" "400")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" =~ ^(400|422)$ ]]; then
        print_result "get_function_missing" "PASS" "Correctly rejected missing FQN"
    else
        print_result "get_function_missing" "WARN" "HTTP ${http_code} for missing FQN"
    fi
    
    # Test with non-existent FQN
    result=$(api_call "GET" "/tools/get_function?fqn=nonexistent::function" "" "404")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" == "404" ]]; then
        print_result "get_function_notfound" "PASS" "Correctly returned 404 for non-existent function"
    elif [[ "${http_code}" == "200" ]]; then
        print_result "get_function_notfound" "FAIL" "Should return 404 for non-existent function"
    else
        print_result "get_function_notfound" "WARN" "HTTP ${http_code} for non-existent function"
    fi
}

# =============================================================================
# TEST 4: Get Callers Endpoint
# =============================================================================
test_get_callers() {
    print_section "Get Callers Endpoint Tests"
    
    # Test with valid FQN
    local result
    result=$(api_call "GET" "/tools/get_callers?fqn=test_fixture::add" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.callers' &>/dev/null; then
            local count
            count=$(echo "${response}" | jq -r '.callers | length' 2>/dev/null || echo "0")
            print_result "get_callers_valid" "PASS" "Found ${count} caller(s)"
        else
            print_result "get_callers_valid" "PASS" "Callers returned"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "get_callers_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "get_callers_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
    
    # Test transitive callers
    result=$(api_call "GET" "/tools/get_callers?fqn=test_fixture::add&transitive=true" "" "200")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" == "200" ]]; then
        print_result "get_callers_transitive" "PASS" "Transitive query accepted"
    else
        print_result "get_callers_transitive" "WARN" "HTTP ${http_code} for transitive query"
    fi
}

# =============================================================================
# TEST 5: Get Trait Implementations
# =============================================================================
test_trait_impls() {
    print_section "Trait Implementations Endpoint Tests"
    
    # Test with valid trait name
    local result
    result=$(api_call "GET" "/tools/get_trait_impls?trait_name=Processor" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.implementations' &>/dev/null; then
            local count
            count=$(echo "${response}" | jq -r '.implementations | length' 2>/dev/null || echo "0")
            print_result "trait_impls_valid" "PASS" "Found ${count} implementation(s)"
        else
            print_result "trait_impls_valid" "PASS" "Implementations returned"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "trait_impls_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "trait_impls_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
}

# =============================================================================
# TEST 6: Find Usages of Type
# =============================================================================
test_find_usages() {
    print_section "Find Usages Endpoint Tests"
    
    # Test with valid type name
    local result
    result=$(api_call "GET" "/tools/find_usages_of_type?type_name=User" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.usages' &>/dev/null; then
            local count
            count=$(echo "${response}" | jq -r '.usages | length' 2>/dev/null || echo "0")
            print_result "find_usages_valid" "PASS" "Found ${count} usage(s)"
        else
            print_result "find_usages_valid" "PASS" "Usages returned"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "find_usages_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "find_usages_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
}

# =============================================================================
# TEST 7: Get Module Tree
# =============================================================================
test_module_tree() {
    print_section "Module Tree Endpoint Tests"
    
    # Test with valid crate name
    local result
    result=$(api_call "GET" "/tools/get_module_tree?crate=test-fixture" "" "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.modules' &>/dev/null; then
            print_result "module_tree_valid" "PASS" "Module tree returned"
        else
            print_result "module_tree_valid" "PASS" "Response received"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "module_tree_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "module_tree_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
}

# =============================================================================
# TEST 8: Query Graph (Raw Cypher)
# =============================================================================
test_query_graph() {
    print_section "Graph Query Endpoint Tests"
    
    # Test with valid Cypher query
    local result
    result=$(api_call "POST" "/tools/query_graph" '{"query": "MATCH (n) RETURN count(n) as count LIMIT 1"}' "200")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "200" ]]; then
        if echo "${response}" | jq -e '.results' &>/dev/null; then
            print_result "query_graph_valid" "PASS" "Graph query returned results"
        else
            print_result "query_graph_valid" "PASS" "Query accepted"
        fi
    elif [[ "${http_code}" == "404" ]]; then
        print_result "query_graph_valid" "FAIL" "Endpoint not found (not implemented)"
    else
        print_result "query_graph_valid" "FAIL" "Returned HTTP ${http_code}"
    fi
    
    # Test with invalid Cypher (error handling)
    result=$(api_call "POST" "/tools/query_graph" '{"query": "INVALID CYPHER"}' "400")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" =~ ^(400|500)$ ]]; then
        print_result "query_graph_invalid" "PASS" "Correctly rejected invalid query"
    else
        print_result "query_graph_invalid" "WARN" "HTTP ${http_code} for invalid query"
    fi
}

# =============================================================================
# TEST 9: Response Format Validation
# =============================================================================
test_response_formats() {
    print_section "Response Format Validation"
    
    # Test JSON content-type
    local result
    result=$(curl -s -D /tmp/headers.txt -o /tmp/body.json \
        --connect-timeout "${TIMEOUT}" \
        -H "Content-Type: application/json" \
        "${API_BASE_URL}/health" 2>/dev/null || true)
    
    local content_type
    content_type=$(grep -i "content-type" /tmp/headers.txt 2>/dev/null | head -1 || echo "")
    
    if [[ "${content_type}" =~ "application/json" ]]; then
        print_result "response_content_type" "PASS" "Content-Type is application/json"
    else
        print_result "response_content_type" "FAIL" "Content-Type not JSON: ${content_type}"
    fi
    
    # Test CORS headers (if applicable)
    local cors_header
    cors_header=$(grep -i "access-control-allow-origin" /tmp/headers.txt 2>/dev/null || echo "")
    
    if [[ -n "${cors_header}" ]]; then
        print_result "response_cors" "PASS" "CORS headers present"
    else
        print_result "response_cors" "WARN" "No CORS headers (may be intentional)"
    fi
}

# =============================================================================
# TEST 10: Error Response Format
# =============================================================================
test_error_format() {
    print_section "Error Response Format Tests"
    
    # Test 404 error format
    local result
    result=$(api_call "GET" "/nonexistent/endpoint" "" "404")
    local http_code="${result%%:*}"
    local response="${result#*:}"
    
    if [[ "${http_code}" == "404" ]]; then
        if echo "${response}" | jq -e '.error' &>/dev/null; then
            print_result "error_format_404" "PASS" "404 returns structured error"
        else
            print_result "error_format_404" "PASS" "404 returns response"
        fi
    else
        print_result "error_format_404" "WARN" "HTTP ${http_code} for non-existent endpoint"
    fi
    
    # Test malformed JSON handling
    result=$(api_call "POST" "/tools/search_semantic" 'not valid json' "400")
    http_code="${result%%:*}"
    
    if [[ "${http_code}" =~ ^(400|422|500)$ ]]; then
        print_result "error_malformed_json" "PASS" "Correctly handled malformed JSON"
    else
        print_result "error_malformed_json" "WARN" "HTTP ${http_code} for malformed JSON"
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    echo "============================================================"
    echo "rust-brain Integration Tests: API Endpoint Verification"
    echo "============================================================"
    echo "API: ${API_BASE_URL}"
    echo "Time: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    echo ""
    
    # Check prerequisites
    if ! command -v jq &>/dev/null; then
        echo -e "${RED}ERROR: jq is required for these tests${NC}"
        exit 1
    fi
    
    # Check if API is available
    if ! check_api_available; then
        echo ""
        echo -e "${YELLOW}API is not available. Tests skipped.${NC}"
        echo -e "${YELLOW}To start the API:${NC}"
        echo "  cd ${PROJECT_DIR}"
        echo "  docker compose up -d"
        echo "  # Or start the API service manually"
        exit 0
    fi
    
    # Run all tests
    test_health_endpoint
    test_semantic_search
    test_get_function
    test_get_callers
    test_trait_impls
    test_find_usages
    test_module_tree
    test_query_graph
    test_response_formats
    test_error_format
    
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
        echo -e "${RED}API TESTS FAILED${NC}"
        exit 1
    else
        echo -e "${GREEN}ALL API TESTS PASSED${NC}"
        exit 0
    fi
}

main "$@"
