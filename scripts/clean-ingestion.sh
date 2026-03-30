#!/bin/bash
# =============================================================================
# rust-brain Clean Ingestion
# =============================================================================
# Reset all databases before fresh ingestion
#
# Usage:
#   ./scripts/clean-ingestion.sh
#   ./scripts/clean-ingestion.sh --skip-qdrant  # Keep vectors
#
# After cleaning, run: ./scripts/ingest.sh /path/to/crate
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# Load environment (export so child scripts like init-qdrant.sh inherit vars)
cd "$PROJECT_ROOT"
set -a  # auto-export all sourced variables
source .env 2>/dev/null || true
set +a

# Override Docker-internal hostnames with localhost for host-side execution.
# .env uses service names (postgres, neo4j, qdrant) which only resolve inside Docker.
export QDRANT_HOST="http://localhost:${QDRANT_REST_PORT:-6333}"

# Parse arguments
SKIP_QDRANT=false
for arg in "$@"; do
    case $arg in
        --skip-qdrant)
            SKIP_QDRANT=true
            shift
            ;;
        --help|-h)
            cat << EOF
rust-brain Clean Ingestion

USAGE:
    clean-ingestion.sh [OPTIONS]

OPTIONS:
    --skip-qdrant    Keep Qdrant vectors (only clean Postgres + Neo4j)
    --help, -h       Show this help

WHAT IT DOES:
    1. Truncates Postgres tables (extracted_items, source_files, etc.)
    2. Clears Neo4j graph (all nodes and relationships)
    3. Deletes Qdrant collections (unless --skip-qdrant)
    4. Reinitializes Qdrant collections

AFTER CLEANING:
    ./scripts/ingest.sh /path/to/crate
EOF
            exit 0
            ;;
    esac
done

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Clean Ingestion                       ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# Resolve container names (handles hash-prefixed names like "abc123_rustbrain-postgres")
resolve_container() {
    docker ps --format "{{.Names}}" | grep "$1" | head -1
}

# Check services are running
echo -e "${YELLOW}Checking services...${NC}"
PG_CONTAINER=$(resolve_container "rustbrain-postgres")
NEO4J_CONTAINER=$(resolve_container "rustbrain-neo4j")

if [ -z "$PG_CONTAINER" ]; then
    echo -e "${RED}ERROR: rustbrain-postgres is not running${NC}"
    echo "Start services first: bash scripts/start.sh"
    exit 1
fi

echo -e "  Postgres: ${PG_CONTAINER}"
echo -e "  Neo4j:    ${NEO4J_CONTAINER:-not found}"

# Reset Postgres
echo ""
echo -e "${YELLOW}Clearing Postgres...${NC}"
docker exec "$PG_CONTAINER" psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" -c \
    "TRUNCATE extracted_items, source_files, call_sites, trait_implementations, ingestion_runs CASCADE;" 2>/dev/null && \
    echo -e "${GREEN}✓ Postgres cleared${NC}" || \
    echo -e "${RED}✗ Failed to clear Postgres${NC}"

# Reset Neo4j
echo ""
echo -e "${YELLOW}Clearing Neo4j...${NC}"
if [ -n "$NEO4J_CONTAINER" ]; then
    docker exec "$NEO4J_CONTAINER" cypher-shell -u neo4j -p "${NEO4J_PASSWORD:-password}" \
        "MATCH (n) DETACH DELETE n" 2>/dev/null && \
        echo -e "${GREEN}✓ Neo4j cleared${NC}" || \
        echo -e "${RED}✗ Failed to clear Neo4j${NC}"
else
    echo -e "${RED}✗ Neo4j container not found${NC}"
fi

# Reset Qdrant
if [ "$SKIP_QDRANT" = false ]; then
    echo ""
    echo -e "${YELLOW}Clearing Qdrant...${NC}"
    
    curl -s -X DELETE http://localhost:6333/collections/code_embeddings 2>/dev/null || true
    curl -s -X DELETE http://localhost:6333/collections/doc_embeddings 2>/dev/null || true
    
    echo -e "${GREEN}✓ Qdrant collections deleted${NC}"
    
    # Reinitialize Qdrant
    echo ""
    echo -e "${YELLOW}Reinitializing Qdrant collections...${NC}"
    bash scripts/init-qdrant.sh 2>/dev/null && \
        echo -e "${GREEN}✓ Qdrant reinitialized${NC}" || \
        echo -e "${RED}✗ Failed to reinitialize Qdrant${NC}"
else
    echo ""
    echo -e "${YELLOW}Skipping Qdrant (--skip-qdrant)${NC}"
fi

# Summary
echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║                    CLEAN COMPLETE                            ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Databases reset:"
echo "  • Postgres: items, source_files cleared"
echo "  • Neo4j: all nodes and relationships deleted"
if [ "$SKIP_QDRANT" = false ]; then
    echo "  • Qdrant: collections recreated"
else
    echo "  • Qdrant: skipped (--skip-qdrant)"
fi
echo ""
echo -e "${GREEN}Ready for fresh ingestion:${NC}"
echo "  ./scripts/ingest.sh /path/to/crate"
echo ""
