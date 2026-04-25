#!/bin/bash
# =============================================================================
# rust-brain Clean Ingestion
# =============================================================================
# Reset all databases before fresh ingestion
#
# Usage:
#   ./scripts/clean-ingestion.sh
#   ./scripts/clean-ingestion.sh --workspace-label Workspace_a1b2c3d4e5f6
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

# Defaults
SKIP_QDRANT=false
WORKSPACE_LABEL=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --workspace-label)
            WORKSPACE_LABEL="$2"
            shift 2
            ;;
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
    --workspace-label <LABEL>  Clean workspace-scoped data (e.g. Workspace_a1b2c3d4e5f6)
    --skip-qdrant             Keep Qdrant vectors (only clean Postgres + Neo4j)
    --help, -h                Show this help

WHAT IT DOES:
  Global mode (no --workspace-label):
    1. Truncates Postgres tables in public schema
    2. Clears Neo4j graph (all nodes and relationships)
    3. Deletes Qdrant global collections
    4. Reinitializes Qdrant collections

  Workspace mode (--workspace-label Workspace_<12hex>):
    1. Truncates Postgres tables in ws_<12hex> schema
    2. Clears Neo4j nodes with the workspace label
    3. Deletes Qdrant workspace collections (ws_<12hex>_*)
    4. Reinitializes workspace Qdrant collections

AFTER CLEANING:
    ./scripts/ingest.sh /path/to/crate
    ./scripts/ingest.sh /path/to/crate --workspace-label Workspace_a1b2c3d4e5f6
EOF
            exit 0
            ;;
        *)
            shift
            ;;
    esac
done

# Derive workspace schema and Qdrant suffix from label
WS_SCHEMA=""
WS_SUFFIX=""
if [ -n "$WORKSPACE_LABEL" ]; then
    PREFIX="Workspace_"
    if [[ "$WORKSPACE_LABEL" == ${PREFIX}* ]]; then
        WS_SUFFIX="${WORKSPACE_LABEL#${PREFIX}}"
        WS_SCHEMA="ws_${WS_SUFFIX}"
    else
        echo -e "${RED}ERROR: Invalid workspace label format: ${WORKSPACE_LABEL}${NC}"
        echo "Expected format: Workspace_<12hex> (e.g. Workspace_a1b2c3d4e5f6)"
        exit 1
    fi
fi

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║           RUST-BRAIN — Clean Ingestion                       ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
if [ -n "$WS_SCHEMA" ]; then
    echo "Mode: workspace (${WORKSPACE_LABEL})"
    echo "  PG schema:  ${WS_SCHEMA}"
    echo "  Qdrant prefix: ws_${WS_SUFFIX}_"
else
    echo "Mode: global (default)"
fi
echo ""

# Resolve container names (handles hash-prefixed names like "abc123_rustbrain-postgres")
resolve_container() {
    # Match exact container name at end of line to avoid false positives
    # e.g. "rustbrain-postgres" should NOT match "rustbrain-postgres-exporter"
    # Handles hash-prefixed names like "abc123_rustbrain-postgres"
    docker ps --format "{{.Names}}" | grep -E "(^|_)$1$" | head -1
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
if [ -n "$WS_SCHEMA" ]; then
    # Workspace mode: truncate tables in workspace schema
    docker exec "$PG_CONTAINER" psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" -c \
        "SET search_path TO ${WS_SCHEMA}, public; TRUNCATE extracted_items, source_files, call_sites, trait_implementations, ingestion_runs CASCADE;" 2>/dev/null && \
        echo -e "${GREEN}✓ Postgres workspace schema (${WS_SCHEMA}) cleared${NC}" || \
        echo -e "${RED}✗ Failed to clear Postgres workspace schema (${WS_SCHEMA})${NC}"
else
    # Global mode: truncate tables in public schema
    docker exec "$PG_CONTAINER" psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" -c \
        "TRUNCATE extracted_items, source_files, call_sites, trait_implementations, ingestion_runs CASCADE;" 2>/dev/null && \
        echo -e "${GREEN}✓ Postgres cleared${NC}" || \
        echo -e "${RED}✗ Failed to clear Postgres${NC}"
fi

# Reset Neo4j
echo ""
echo -e "${YELLOW}Clearing Neo4j...${NC}"
if [ -n "$NEO4J_CONTAINER" ]; then
    if [ -n "$WORKSPACE_LABEL" ]; then
        # Workspace mode: only delete nodes with the workspace label
        docker exec "$NEO4J_CONTAINER" cypher-shell -u neo4j -p "${NEO4J_PASSWORD:-password}" \
            "MATCH (n:${WORKSPACE_LABEL}) DETACH DELETE n" 2>/dev/null && \
            echo -e "${GREEN}✓ Neo4j workspace nodes (${WORKSPACE_LABEL}) cleared${NC}" || \
            echo -e "${RED}✗ Failed to clear Neo4j workspace nodes${NC}"
    else
        # Global mode: delete all nodes
        docker exec "$NEO4J_CONTAINER" cypher-shell -u neo4j -p "${NEO4J_PASSWORD:-password}" \
            "MATCH (n) DETACH DELETE n" 2>/dev/null && \
            echo -e "${GREEN}✓ Neo4j cleared${NC}" || \
            echo -e "${RED}✗ Failed to clear Neo4j${NC}"
    fi
else
    echo -e "${RED}✗ Neo4j container not found${NC}"
fi

# Reset Qdrant
if [ "$SKIP_QDRANT" = false ]; then
    echo ""
    echo -e "${YELLOW}Clearing Qdrant...${NC}"

    if [ -n "$WS_SUFFIX" ]; then
        # Workspace mode: delete workspace-scoped collections
        curl -s -X DELETE "http://localhost:6333/collections/ws_${WS_SUFFIX}_code_embeddings" 2>/dev/null || true
        curl -s -X DELETE "http://localhost:6333/collections/ws_${WS_SUFFIX}_doc_embeddings" 2>/dev/null || true
        echo -e "${GREEN}✓ Qdrant workspace collections deleted (ws_${WS_SUFFIX}_*)${NC}"
    else
        # Global mode: delete global collections
        curl -s -X DELETE http://localhost:6333/collections/code_embeddings 2>/dev/null || true
        curl -s -X DELETE http://localhost:6333/collections/doc_embeddings 2>/dev/null || true
        echo -e "${GREEN}✓ Qdrant global collections deleted${NC}"
    fi

    # Reinitialize Qdrant
    echo ""
    echo -e "${YELLOW}Reinitializing Qdrant collections...${NC}"
    if [ -n "$WS_SUFFIX" ]; then
        # For workspace mode, the ingestion pipeline will auto-create collections
        # when it starts via QdrantConfig::for_workspace(). Just verify the old ones are gone.
        echo -e "${GREEN}✓ Workspace Qdrant collections will be created on next ingestion${NC}"
    else
        bash scripts/init-qdrant.sh 2>/dev/null && \
            echo -e "${GREEN}✓ Qdrant reinitialized${NC}" || \
            echo -e "${RED}✗ Failed to reinitialize Qdrant${NC}"
    fi
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
if [ -n "$WS_SCHEMA" ]; then
    echo "Workspace mode (${WORKSPACE_LABEL}):"
    echo "  • Postgres: ${WS_SCHEMA} schema cleared"
    echo "  • Neo4j: ${WORKSPACE_LABEL} label nodes deleted"
    if [ "$SKIP_QDRANT" = false ]; then
        echo "  • Qdrant: ws_${WS_SUFFIX}_* collections deleted"
    else
        echo "  • Qdrant: skipped (--skip-qdrant)"
    fi
    echo ""
    echo -e "${GREEN}Ready for workspace ingestion:${NC}"
    echo "  ./scripts/ingest.sh /path/to/crate --workspace-label ${WORKSPACE_LABEL}"
else
    echo "Databases reset (global):"
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
fi
echo ""
