#!/bin/bash
# =============================================================================
# rustbrain-ingestion wrapper script
# =============================================================================
# This script ensures ingestion ALWAYS runs in a container with proper memory
# limits. NEVER run rustbrain-ingestion directly on the host.
#
# Usage:
#   ./scripts/ingest.sh /path/to/repo [options]
#   ./scripts/ingest.sh --help
#
# Examples:
#   ./scripts/ingest.sh ~/projects/my-repo
#   ./scripts/ingest.sh ~/projects/my-repo --memory-budget 8GB
#   ./scripts/ingest.sh ~/projects/my-repo --dry-run
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Default values
MEMORY_BUDGET="${INGESTION_MEMORY_BUDGET:-32GB}"
WORKSPACE_LABEL=""
WORKSPACE_PATH="${1:-.}"
shift || true

# Help message
show_help() {
    cat << EOF
rustbrain-ingestion - Memory-bounded code ingestion

USAGE:
    ingest.sh <workspace-path> [OPTIONS]

OPTIONS:
    --workspace-label <LABEL>  Workspace label (e.g. Workspace_a1b2c3d4e5f6)
     --memory-budget <SIZE>   Memory budget (default: 32GB, max: 62GB)
    --dry-run                Parse only, no database writes
    --resume <run-id>        Resume from checkpoint
    --verbose                Enable debug logging
    --help                   Show this help

EXAMPLES:
    ingest.sh ~/projects/my-repo
    ingest.sh ~/projects/large-repo --memory-budget 32GB
    ingest.sh ~/projects/my-repo --dry-run --verbose

MEMORY SAFETY:
    This script enforces container execution with memory limits.
    Direct host execution is disabled to prevent OOM crashes.

    Memory budget breakdown (32GB default):
    Stages run SEQUENTIALLY — peak memory = max(stage) + overhead.
    - Discover stage:  512MB   (peak stage)
    - Expand stage:    4GB
    - Parse stage:     6GB     (peak stage)
    - Typecheck stage: 2GB
    - Graph stage:     4GB
    - Embed stage:     3GB
    - Overhead:        ~2GB    (runtime, DB pools, OS, cargo expand cache)
    Peak memory: ~6GB + ~2GB overhead ≈ 8GB (well within 32GB limit)
EOF
}

# Parse options
EXTRA_ARGS=()
while [[ $# -gt 0 ]]; do
    case $1 in
        --help|-h)
            show_help
            exit 0
            ;;
        --memory-budget)
            MEMORY_BUDGET="$2"
            shift 2
            ;;
        --workspace-label)
            WORKSPACE_LABEL="$2"
            EXTRA_ARGS+=("--workspace-label" "$2")
            shift 2
            ;;
        --verbose|-v)
            EXTRA_ARGS+=("--verbose")
            shift
            ;;
        --dry-run)
            EXTRA_ARGS+=("--dry-run")
            shift
            ;;
        --resume)
            EXTRA_ARGS+=("--resume" "$2")
            shift 2
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

# Validate workspace path
if [[ ! -d "$WORKSPACE_PATH" ]]; then
    echo "ERROR: Workspace path does not exist: $WORKSPACE_PATH"
    exit 1
fi

# Convert to absolute path
WORKSPACE_PATH="$(cd "$WORKSPACE_PATH" && pwd)"

echo "============================================================"
echo "rustbrain-ingestion (containerized)"
echo "============================================================"
echo "Workspace:     $WORKSPACE_PATH"
echo "Label:         ${WORKSPACE_LABEL:-global (default)}"
echo "Memory budget: $MEMORY_BUDGET"
echo "============================================================"

# Check if docker is available
if ! command -v docker &> /dev/null; then
    echo "ERROR: Docker is required but not installed."
    exit 1
fi

# Check if docker compose is available
if docker compose version &> /dev/null; then
    COMPOSE_CMD="docker compose"
elif docker-compose version &> /dev/null; then
    COMPOSE_CMD="docker-compose"
else
    echo "ERROR: Docker Compose is required but not installed."
    exit 1
fi

# macOS: auto-apply GPU-free Docker override
if [ "$(uname -s)" = "Darwin" ] && [ ! -f "$PROJECT_ROOT/docker-compose.override.yml" ]; then
  if [ -f "$PROJECT_ROOT/docker-compose.macos.yml" ]; then
    cp "$PROJECT_ROOT/docker-compose.macos.yml" "$PROJECT_ROOT/docker-compose.override.yml"
    echo "Applied macOS override (no NVIDIA GPU)"
  fi
fi

# Build the ingestion image if needed
echo "Checking ingestion image..."
$COMPOSE_CMD -f "$PROJECT_ROOT/docker-compose.yml" build ingestion

# When --workspace-label is set, append search_path to DATABASE_URL so the
# ingestion pipeline writes into the workspace-scoped Postgres schema rather
# than the default public schema.  Mirrors what the API does via
# append_search_path() in services/api/src/handlers/workspace.rs.
INGESTION_DB_URL="${DATABASE_URL:-}"
if [ -n "$WORKSPACE_LABEL" ]; then
    WS_SUFFIX="${WORKSPACE_LABEL#Workspace_}"
    WS_SCHEMA="ws_${WS_SUFFIX}"
    SEARCH_PATH_PARAM="options=--search_path%3D${WS_SCHEMA},public"
    if [[ "$INGESTION_DB_URL" == *'?'* ]]; then
        INGESTION_DB_URL="${INGESTION_DB_URL}&${SEARCH_PATH_PARAM}"
    else
        INGESTION_DB_URL="${INGESTION_DB_URL}?${SEARCH_PATH_PARAM}"
    fi
fi

# Run ingestion in container with memory limits
# --rm ensures container is removed after run (no accumulation)
echo "Starting ingestion..."
$COMPOSE_CMD -f "$PROJECT_ROOT/docker-compose.yml" run --rm \
    -e INGESTION_MEMORY_BUDGET="$MEMORY_BUDGET" \
    ${INGESTION_DB_URL:+-e DATABASE_URL="$INGESTION_DB_URL"} \
    -v "$WORKSPACE_PATH:/workspace/target-repo" \
    ingestion \
    --crate-path /workspace/target-repo \
    ${EXTRA_ARGS[@]:-}

echo "============================================================"
echo "Ingestion complete!"
echo "============================================================"
