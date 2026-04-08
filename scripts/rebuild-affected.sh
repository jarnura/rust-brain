#!/bin/bash
# =============================================================================
# rust-brain — Rebuild Affected Containers
# =============================================================================
# Detects changed files and rebuilds only the Docker services whose source
# changed. Accepts file paths via arguments, stdin, or falls back to
# git diff HEAD~1.
#
# Usage:
#   scripts/rebuild-affected.sh                   # auto-detect via git diff
#   scripts/rebuild-affected.sh --dry-run         # show what would rebuild
#   scripts/rebuild-affected.sh services/api/...  # explicit paths
#   git diff --name-only HEAD~2 | scripts/rebuild-affected.sh
# =============================================================================
set -euo pipefail

cd "$(dirname "$0")/.."

DRY_RUN=false
CHANGED_FILES=()

# ---------------------------------------------------------------------------
# Parse arguments
# ---------------------------------------------------------------------------
for arg in "$@"; do
    if [ "$arg" = "--dry-run" ]; then
        DRY_RUN=true
    else
        CHANGED_FILES+=("$arg")
    fi
done

# ---------------------------------------------------------------------------
# Collect changed files from stdin if piped and no explicit paths given
# ---------------------------------------------------------------------------
if [ ${#CHANGED_FILES[@]} -eq 0 ] && [ ! -t 0 ]; then
    while IFS= read -r line; do
        [ -n "$line" ] && CHANGED_FILES+=("$line")
    done
fi

# ---------------------------------------------------------------------------
# Fall back to git diff HEAD~1
# ---------------------------------------------------------------------------
if [ ${#CHANGED_FILES[@]} -eq 0 ]; then
    while IFS= read -r line; do
        [ -n "$line" ] && CHANGED_FILES+=("$line")
    done < <(git diff --name-only HEAD~1 2>/dev/null || true)
fi

if [ ${#CHANGED_FILES[@]} -eq 0 ]; then
    echo "No changed files detected. Nothing to rebuild."
    exit 0
fi

# ---------------------------------------------------------------------------
# Map changed paths → Docker Compose services
# ---------------------------------------------------------------------------
declare -A SERVICES_MAP

for file in "${CHANGED_FILES[@]}"; do
    case "$file" in
        services/api/*)
            SERVICES_MAP[api]=1
            ;;
        services/mcp/*)
            SERVICES_MAP[mcp]=1
            SERVICES_MAP[mcp-sse]=1
            ;;
        services/ingestion/*)
            SERVICES_MAP[ingestion]=1
            ;;
        frontend/*)
            SERVICES_MAP[playground-ui]=1
            ;;
        crates/rustbrain-common/*)
            SERVICES_MAP[api]=1
            SERVICES_MAP[mcp]=1
            SERVICES_MAP[mcp-sse]=1
            SERVICES_MAP[ingestion]=1
            ;;
        configs/opencode/*)
            SERVICES_MAP[opencode]=1
            ;;
    esac
done

# Deduplicated, sorted service list
SERVICES=($(echo "${!SERVICES_MAP[@]}" | tr ' ' '\n' | sort))

if [ ${#SERVICES[@]} -eq 0 ]; then
    echo "Changed files do not affect any source-built service. Nothing to rebuild."
    exit 0
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo "=== Rebuild Affected Services ==="
echo ""
echo "Changed files (${#CHANGED_FILES[@]}):"
for f in "${CHANGED_FILES[@]}"; do
    echo "  $f"
done
echo ""
echo "Services to rebuild: ${SERVICES[*]}"
echo ""

# ---------------------------------------------------------------------------
# Build and restart
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" = true ]; then
    echo "[dry-run] Would run:"
    echo "  docker compose build ${SERVICES[*]}"
    echo "  docker compose up -d ${SERVICES[*]}"
    exit 0
fi

echo "=== Building ==="
if ! docker compose build "${SERVICES[@]}"; then
    echo ""
    echo "ERROR: docker compose build failed."
    exit 1
fi

echo ""
echo "=== Restarting ==="
if ! docker compose up -d "${SERVICES[@]}"; then
    echo ""
    echo "ERROR: docker compose up failed."
    exit 1
fi

echo ""
echo "=== Done ==="
echo "Rebuilt and restarted: ${SERVICES[*]}"
