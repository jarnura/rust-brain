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
MEMORY_BUDGET="${INGESTION_MEMORY_BUDGET:-16GB}"
WORKSPACE_PATH="${1:-.}"
shift || true

# Help message
show_help() {
    cat << EOF
rustbrain-ingestion - Memory-bounded code ingestion

USAGE:
    ingest.sh <workspace-path> [OPTIONS]

OPTIONS:
    --memory-budget <SIZE>   Memory budget (default: 16GB, max: 62GB)
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

    Memory budget breakdown (16GB default):
    - Discover stage:  512MB
    - Expand stage:    2GB
    - Parse stage:     3GB
    - Typecheck stage: 1GB
    - Graph stage:     2GB
    - Embed stage:     1.5GB
    - Overhead:        6GB (runtime, DB pools, OS)
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

# Run ingestion in container with memory limits
# --rm ensures container is removed after run (no accumulation)
echo "Starting ingestion..."
$COMPOSE_CMD -f "$PROJECT_ROOT/docker-compose.yml" run --rm \
    -e INGESTION_MEMORY_BUDGET="$MEMORY_BUDGET" \
    -v "$WORKSPACE_PATH:/workspace/target-repo" \
    ingestion \
    --crate-path /workspace/target-repo \
    ${EXTRA_ARGS[@]:-}

echo "============================================================"
echo "Ingestion complete!"
echo "============================================================"
