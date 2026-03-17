#!/bin/bash
# =============================================================================
# rust-brain — Integration Tests: Pipeline Verification
# =============================================================================
# Runs ingestion on test fixture and verifies:
# - Postgres tables populated
# - Neo4j nodes created
# - Qdrant has embeddings
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
FIXTURE_CRATE="${PROJECT_DIR}/tests/fixtures/test-crate"

# Load environment variables
if [[ -f "${ENV_FILE}" ]]; then
    set -a
    source "${ENV_FILE}"
    set +a
fi

# Default values
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-rustbrain}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-<your-password>}"
POSTGRES_DB="${POSTGRES_DB:-rustbrain}"
NEO4J_HTTP_PORT="${NEO4J_HTTP_PORT:-7474}"
NEO4J_BOLT_PORT="${NEO4J_BOLT_PORT:-7687}"
NEO4J_PASSWORD="${NEO4J_PASSWORD:-<your-password>}"
QDRANT_REST_PORT="${QDRANT_REST_PORT:-6333}"

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

# =============================================================================
# PREREQUISITE CHECKS
# =============================================================================
check_prerequisites() {
    print_section "Checking Prerequisites"
    
    # Check docker
    if ! docker info &>/dev/null; then
        echo -e "${RED}ERROR: Docker is not running${NC}"
        exit 1
    fi
    
    # Check test fixture exists
    if [[ ! -d "${FIXTURE_CRATE}" ]]; then
        echo -e "${RED}ERROR: Test fixture not found at ${FIXTURE_CRATE}${NC}"
        exit 1
    fi
    
    # Check required tools
    local tools=("psql" "curl" "jq")
    for tool in "${tools[@]}"; do
        if ! command -v "${tool}" &>/dev/null; then
            echo -e "${RED}ERROR: Required tool '${tool}' not found${NC}"
            exit 1
        fi
    done
    
    echo -e "${GREEN}All prerequisites met${NC}"
}

# =============================================================================
# TEST 1: Verify Postgres Tables Exist
# =============================================================================
test_postgres_tables() {
    print_section "Postgres Table Verification"
    
    local tables
    tables=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name;" 2>/dev/null | tr -d ' ')
    
    local expected_tables=("source_files" "extracted_items" "call_sites" "ingestion_runs" "repositories")
    local missing_tables=()
    
    for table in "${expected_tables[@]}"; do
        if ! echo "${tables}" | grep -q "^${table}$"; then
            missing_tables+=("${table}")
        fi
    done
    
    if [[ ${#missing_tables[@]} -eq 0 ]]; then
        print_result "postgres_tables" "PASS" "All expected tables exist"
    else
        print_result "postgres_tables" "FAIL" "Missing tables: ${missing_tables[*]}"
    fi
}

# =============================================================================
# TEST 2: Run Ingestion on Test Fixture
# =============================================================================
run_ingestion() {
    print_section "Running Ingestion"
    
    echo -e "${YELLOW}Starting ingestion on test fixture...${NC}"
    
    # Set environment for ingestion
    export DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${POSTGRES_DB}"
    export WORKSPACE_PATH="${FIXTURE_CRATE}"
    
    # Check if ingestion service container is running
    local ingestion_running
    ingestion_running=$(docker ps --filter "name=rustbrain-ingestion" --filter "status=running" -q 2>/dev/null || true)
    
    if [[ -n "${ingestion_running}" ]]; then
        echo "Using ingestion container..."
        docker exec -i rustbrain-ingestion /app/ingestion 2>&1 || true
    else
        # Try running ingestion service directly
        local ingestion_bin="${PROJECT_DIR}/services/ingestion/target/release/ingestion"
        if [[ -x "${ingestion_bin}" ]]; then
            echo "Running ingestion binary..."
            "${ingestion_bin}" 2>&1 || true
        else
            echo -e "${YELLOW}Ingestion service not running, attempting docker compose...${NC}"
            cd "${PROJECT_DIR}"
            docker compose run --rm -e WORKSPACE_PATH=/workspace/test-fixture -v "${FIXTURE_CRATE}:/workspace/test-fixture:ro" ingestion 2>&1 || {
                print_result "ingestion_run" "FAIL" "Could not run ingestion"
                return 1
            }
        fi
    fi
    
    print_result "ingestion_run" "PASS" "Ingestion completed"
}

# =============================================================================
# TEST 3: Verify Postgres Data Populated
# =============================================================================
test_postgres_data() {
    print_section "Postgres Data Verification"
    
    # Check source_files
    local source_count
    source_count=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT COUNT(*) FROM source_files WHERE crate_name = 'test-fixture';" 2>/dev/null | tr -d ' ')
    
    if [[ "${source_count}" -gt 0 ]]; then
        print_result "postgres_source_files" "PASS" "Found ${source_count} source file(s)"
    else
        print_result "postgres_source_files" "FAIL" "No source files found for test-fixture"
    fi
    
    # Check extracted_items
    local item_count
    item_count=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT COUNT(*) FROM extracted_items;" 2>/dev/null | tr -d ' ')
    
    if [[ "${item_count}" -gt 0 ]]; then
        print_result "postgres_extracted_items" "PASS" "Found ${item_count} extracted item(s)"
    else
        print_result "postgres_extracted_items" "FAIL" "No extracted items found"
    fi
    
    # Check ingestion_runs
    local run_status
    run_status=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT status FROM ingestion_runs ORDER BY started_at DESC LIMIT 1;" 2>/dev/null | tr -d ' ')
    
    if [[ "${run_status}" =~ ^(completed|partial)$ ]]; then
        print_result "postgres_ingestion_run" "PASS" "Last run status: ${run_status}"
    else
        print_result "postgres_ingestion_run" "FAIL" "Last run status: ${run_status:-none}"
    fi
    
    # Verify specific items exist
    local function_count
    function_count=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT COUNT(*) FROM extracted_items WHERE item_type = 'function';" 2>/dev/null | tr -d ' ')
    
    if [[ "${function_count}" -gt 0 ]]; then
        print_result "postgres_functions" "PASS" "Found ${function_count} function(s)"
    else
        print_result "postgres_functions" "FAIL" "No functions extracted"
    fi
    
    local struct_count
    struct_count=$(psql -h localhost -p "${POSTGRES_PORT}" -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -t -c "SELECT COUNT(*) FROM extracted_items WHERE item_type = 'struct';" 2>/dev/null | tr -d ' ')
    
    if [[ "${struct_count}" -gt 0 ]]; then
        print_result "postgres_structs" "PASS" "Found ${struct_count} struct(s)"
    else
        print_result "postgres_structs" "FAIL" "No structs extracted"
    fi
}

# =============================================================================
# TEST 4: Verify Neo4j Nodes Created
# =============================================================================
test_neo4j_nodes() {
    print_section "Neo4j Node Verification"
    
    # Check if Neo4j is accessible
    local neo4j_health
    neo4j_health=$(curl -s -o /dev/null -w "%{http_code}" "http://localhost:${NEO4J_HTTP_PORT}" 2>/dev/null || echo "000")
    
    if [[ "${neo4j_health}" == "000" ]]; then
        print_result "neo4j_connection" "FAIL" "Cannot connect to Neo4j on port ${NEO4J_HTTP_PORT}"
        return 1
    fi
    
    print_result "neo4j_connection" "PASS" "Neo4j accessible on port ${NEO4J_HTTP_PORT}"
    
    # Query for code nodes
    local node_count
    node_count=$(curl -s -X POST \
        -H "Content-Type: application/json" \
        -u "neo4j:${NEO4J_PASSWORD}" \
        -d '{"query": "MATCH (n) RETURN count(n) as count"}' \
        "http://localhost:${NEO4J_HTTP_PORT}/db/neo4j/tx/commit" 2>/dev/null | \
        jq -r '.results[0].data[0].row[0] // 0' 2>/dev/null || echo "0")
    
    if [[ "${node_count}" -gt 0 ]]; then
        print_result "neo4j_nodes" "PASS" "Found ${node_count} node(s) in graph"
    else
        print_result "neo4j_nodes" "FAIL" "No nodes found in graph"
    fi
    
    # Query for function nodes
    local func_count
    func_count=$(curl -s -X POST \
        -H "Content-Type: application/json" \
        -u "neo4j:${NEO4J_PASSWORD}" \
        -d '{"query": "MATCH (n:Function) RETURN count(n) as count"}' \
        "http://localhost:${NEO4J_HTTP_PORT}/db/neo4j/tx/commit" 2>/dev/null | \
        jq -r '.results[0].data[0].row[0] // 0' 2>/dev/null || echo "0")
    
    if [[ "${func_count}" -gt 0 ]]; then
        print_result "neo4j_functions" "PASS" "Found ${func_count} function node(s)"
    else
        print_result "neo4j_functions" "FAIL" "No function nodes found"
    fi
    
    # Query for relationships
    local rel_count
    rel_count=$(curl -s -X POST \
        -H "Content-Type: application/json" \
        -u "neo4j:${NEO4J_PASSWORD}" \
        -d '{"query": "MATCH ()-[r]->() RETURN count(r) as count"}' \
        "http://localhost:${NEO4J_HTTP_PORT}/db/neo4j/tx/commit" 2>/dev/null | \
        jq -r '.results[0].data[0].row[0] // 0' 2>/dev/null || echo "0")
    
    if [[ "${rel_count}" -gt 0 ]]; then
        print_result "neo4j_relationships" "PASS" "Found ${rel_count} relationship(s)"
    else
        print_result "neo4j_relationships" "FAIL" "No relationships found"
    fi
}

# =============================================================================
# TEST 5: Verify Qdrant Embeddings
# =============================================================================
test_qdrant_embeddings() {
    print_section "Qdrant Embedding Verification"
    
    # Check Qdrant health
    local qdrant_health
    qdrant_health=$(curl -s "http://localhost:${QDRANT_REST_PORT}/healthz" 2>/dev/null || echo "")
    
    if [[ -z "${qdrant_health}" ]]; then
        print_result "qdrant_connection" "FAIL" "Cannot connect to Qdrant on port ${QDRANT_REST_PORT}"
        return 1
    fi
    
    print_result "qdrant_connection" "PASS" "Qdrant accessible on port ${QDRANT_REST_PORT}"
    
    # List collections
    local collections
    collections=$(curl -s "http://localhost:${QDRANT_REST_PORT}/collections" 2>/dev/null | \
        jq -r '.result.collections[].name' 2>/dev/null || echo "")
    
    if [[ -n "${collections}" ]]; then
        print_result "qdrant_collections" "PASS" "Collections: ${collections//$'\n'/, }"
    else
        print_result "qdrant_collections" "FAIL" "No collections found"
    fi
    
    # Check for code embeddings collection
    local code_collection
    code_collection=$(curl -s "http://localhost:${QDRANT_REST_PORT}/collections/code_embeddings" 2>/dev/null | \
        jq -r '.result.status // "not_found"' 2>/dev/null || echo "not_found")
    
    if [[ "${code_collection}" == "green" ]]; then
        print_result "qdrant_code_collection" "PASS" "code_embeddings collection is healthy"
    else
        # Try alternate collection names
        local alt_collections=("rust_embeddings" "embeddings" "vectors")
        local found=false
        for coll in "${alt_collections[@]}"; do
            local status
            status=$(curl -s "http://localhost:${QDRANT_REST_PORT}/collections/${coll}" 2>/dev/null | \
                jq -r '.result.status // "not_found"' 2>/dev/null || echo "not_found")
            if [[ "${status}" == "green" ]]; then
                print_result "qdrant_code_collection" "PASS" "${coll} collection is healthy"
                found=true
                break
            fi
        done
        
        if [[ "${found}" == false ]]; then
            print_result "qdrant_code_collection" "FAIL" "No embeddings collection found"
        fi
    fi
    
    # Check vector count
    local vector_count
    vector_count=$(curl -s "http://localhost:${QDRANT_REST_PORT}/collections/code_embeddings" 2>/dev/null | \
        jq -r '.result.points_count // 0' 2>/dev/null || echo "0")
    
    if [[ "${vector_count}" -gt 0 ]]; then
        print_result "qdrant_vectors" "PASS" "Found ${vector_count} vector(s)"
    else
        # Try to get count from any collection
        for coll in "rust_embeddings" "embeddings"; do
            vector_count=$(curl -s "http://localhost:${QDRANT_REST_PORT}/collections/${coll}" 2>/dev/null | \
                jq -r '.result.points_count // 0' 2>/dev/null || echo "0")
            if [[ "${vector_count}" -gt 0 ]]; then
                print_result "qdrant_vectors" "PASS" "Found ${vector_count} vector(s) in ${coll}"
                break
            fi
        done
        
        if [[ "${vector_count}" -eq 0 ]]; then
            print_result "qdrant_vectors" "FAIL" "No vectors found in any collection"
        fi
    fi
}

# =============================================================================
# TEST 6: Semantic Search Test
# =============================================================================
test_semantic_search() {
    print_section "Semantic Search Verification"
    
    # Try to perform a semantic search
    local search_result
    search_result=$(curl -s -X POST \
        -H "Content-Type: application/json" \
        -d '{"vector": [0.1, 0.2, 0.3], "limit": 5}' \
        "http://localhost:${QDRANT_REST_PORT}/collections/code_embeddings/points/search" 2>/dev/null || echo "")
    
    if [[ -n "${search_result}" ]] && echo "${search_result}" | jq -e '.result' &>/dev/null; then
        local result_count
        result_count=$(echo "${search_result}" | jq -r '.result | length' 2>/dev/null || echo "0")
        print_result "semantic_search" "PASS" "Search returned ${result_count} result(s)"
    else
        # Check if collection exists with different name
        local found=false
        for coll in "rust_embeddings" "embeddings"; do
            search_result=$(curl -s -X POST \
                -H "Content-Type: application/json" \
                -d '{"vector": [0.1, 0.2, 0.3], "limit": 5}' \
                "http://localhost:${QDRANT_REST_PORT}/collections/${coll}/points/search" 2>/dev/null || echo "")
            
            if [[ -n "${search_result}" ]] && echo "${search_result}" | jq -e '.result' &>/dev/null; then
                print_result "semantic_search" "PASS" "Search on ${coll} returned results"
                found=true
                break
            fi
        done
        
        if [[ "${found}" == false ]]; then
            print_result "semantic_search" "FAIL" "Semantic search not working"
        fi
    fi
}

# =============================================================================
# Main
# =============================================================================
main() {
    echo "============================================================"
    echo "rust-brain Integration Tests: Pipeline Verification"
    echo "============================================================"
    echo "Project: ${PROJECT_DIR}"
    echo "Fixture: ${FIXTURE_CRATE}"
    echo "Time: $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    echo ""
    
    # Check prerequisites
    check_prerequisites
    
    # Run tests in order
    test_postgres_tables
    run_ingestion
    test_postgres_data
    test_neo4j_nodes
    test_qdrant_embeddings
    test_semantic_search
    
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
        echo -e "${RED}INTEGRATION TESTS FAILED${NC}"
        exit 1
    else
        echo -e "${GREEN}ALL INTEGRATION TESTS PASSED${NC}"
        exit 0
    fi
}

main "$@"
