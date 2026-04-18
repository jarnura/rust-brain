#!/bin/bash
# =============================================================================
# rust-brain — Migrate Qdrant Collections: Global → Per-Workspace
# =============================================================================
# One-time migration script to move data from global Qdrant collections
# (code_embeddings, doc_embeddings, crate_docs) into per-workspace collections
# following the ADR-005 naming convention: ws_<12hex>_<collection_type>
#
# Usage:
#   ./scripts/migrate-qdrant-workspace.sh              # Live migration
#   ./scripts/migrate-qdrant-workspace.sh --dry-run    # Preview only
#   ./scripts/migrate-qdrant-workspace.sh --delete-source  # Remove global collections after migration
#
# Safety:
#   - Global collections are NEVER deleted unless --delete-source is passed
#   - All operations are idempotent (upserts, PUT for collection creation)
#   - --dry-run shows what would happen without writing
#
# Environment variables (loaded from .env if present):
#   QDRANT_HOST            - Qdrant REST API URL (default: http://localhost:6333)
#   EMBEDDING_DIMENSIONS   - Vector dimensions (default: 2560)
#   DATABASE_URL           - Postgres connection string (required)
#   QDRANT_DEFAULT_WORKSPACE_ID - Workspace UUID for orphan data (default: auto-created)
# =============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# Load .env if present
if [[ -f "$PROJECT_DIR/.env" ]]; then
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
fi

QDRANT_HOST="${QDRANT_HOST:-http://localhost:6333}"
EMBEDDING_DIMENSIONS="${EMBEDDING_DIMENSIONS:-2560}"
DATABASE_URL="${DATABASE_URL:-}"
DEFAULT_WORKSPACE_ID="${QDRANT_DEFAULT_WORKSPACE_ID:-}"
BATCH_SIZE="${MIGRATION_BATCH_SIZE:-500}"
DRY_RUN=false
DELETE_SOURCE=false

# Collections to migrate (external_docs stays global per ADR-005)
COLLECTIONS=("code_embeddings" "doc_embeddings" "crate_docs")

# Payload indexes per collection type (matching init-qdrant.sh)
declare -A CODE_INDEXES=(
    ["fqn"]="keyword"
    ["crate_name"]="keyword"
    ["module_path"]="keyword"
    ["item_type"]="keyword"
    ["visibility"]="keyword"
    ["file_path"]="keyword"
    ["has_generics"]="bool"
)
declare -A DOC_INDEXES=(
    ["source_fqn"]="keyword"
    ["content_type"]="keyword"
    ["crate_name"]="keyword"
)
declare -A CRATE_DOCS_INDEXES=(
    ["fqn"]="keyword"
    ["crate_name"]="keyword"
    ["symbol_type"]="keyword"
)

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case $1 in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --delete-source)
            DELETE_SOURCE=true
            shift
            ;;
        --batch-size)
            BATCH_SIZE="$2"
            shift 2
            ;;
        --help|-h)
            head -n 25 "$0" | tail -n +2 | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "Unknown argument: $1"
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Utility functions
# ---------------------------------------------------------------------------
log_info()  { echo -e "${BLUE}[INFO]${NC}  $*"; }
log_ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

dry_run_msg() {
    if [[ "$DRY_RUN" == true ]]; then
        log_warn "[DRY-RUN] $*"
    fi
}

# Execute or skip based on dry-run
run_cmd() {
    if [[ "$DRY_RUN" == true ]]; then
        log_warn "[DRY-RUN] Would execute: $*"
        return 0
    fi
    eval "$@"
}

# ---------------------------------------------------------------------------
# Prerequisite checks
# ---------------------------------------------------------------------------
check_prerequisites() {
    log_info "Checking prerequisites..."

    # Check required tools
    for cmd in curl jq; do
        if ! command -v "$cmd" &>/dev/null; then
            log_error "Required tool not found: $cmd"
            exit 1
        fi
    done
    log_ok "Required tools available (curl, jq)"

    # Check Qdrant health
    for i in {1..30}; do
        if curl -sf "${QDRANT_HOST}/healthz" >/dev/null 2>&1; then
            log_ok "Qdrant is healthy at ${QDRANT_HOST}"
            break
        fi
        if [[ $i -eq 30 ]]; then
            log_error "Qdrant not healthy after 30 seconds at ${QDRANT_HOST}"
            exit 1
        fi
        sleep 1
    done

    # Check DATABASE_URL
    if [[ -z "$DATABASE_URL" ]]; then
        log_error "DATABASE_URL is not set. Export it or add to .env"
        exit 1
    fi
    log_ok "DATABASE_URL is set"

    # Check psql is available for Postgres queries
    if ! command -v psql &>/dev/null; then
        log_error "psql not found. Install postgresql-client."
        exit 1
    fi

    # Verify Postgres connectivity
    if ! psql "$DATABASE_URL" -c "SELECT 1" >/dev/null 2>&1; then
        log_error "Cannot connect to Postgres with DATABASE_URL"
        exit 1
    fi
    log_ok "Postgres is accessible"

    # Verify global collections exist
    local collections_json
    collections_json=$(curl -sf "${QDRANT_HOST}/collections")
    for col in "${COLLECTIONS[@]}"; do
        if echo "$collections_json" | jq -e ".result.collections[] | select(.name == \"$col\")" >/dev/null 2>&1; then
            local count
            count=$(curl -sf "${QDRANT_HOST}/collections/${col}" | jq '.result.points_count // 0')
            log_ok "Source collection '${col}' exists (${count} points)"
        else
            log_warn "Source collection '${col}' not found — skipping"
        fi
    done
}

# ---------------------------------------------------------------------------
# Build crate_name → workspace mapping from Postgres
# ---------------------------------------------------------------------------
build_workspace_map() {
    log_info "Building crate_name → workspace mapping from Postgres..."

    # Query workspaces table
    local ws_rows
    ws_rows=$(psql "$DATABASE_URL" -t -A -F'|' -c "
        SELECT id, schema_name, name, status
        FROM workspaces
        WHERE status != 'archived'
        ORDER BY created_at;
    " 2>/dev/null)

    if [[ -z "$ws_rows" ]]; then
        log_warn "No active workspaces found in workspaces table"
        log_info "All data will be migrated to default workspace (ws_default)"
        return
    fi

    # For each workspace, find distinct crate_names in its schema
    local ws_count=0
    while IFS='|' read -r ws_id ws_schema ws_name ws_status; do
        [[ -z "$ws_schema" ]] && continue

        ws_count=$((ws_count + 1))
        log_info "  Workspace: ${ws_name} (${ws_schema}) — status: ${ws_status}"

        # Check if workspace schema has source_files with crate_names
        local crate_names
        crate_names=$(psql "$DATABASE_URL" -t -A -c "
            SELECT DISTINCT crate_name FROM ${ws_schema}.source_files;
        " 2>/dev/null || echo "")

        if [[ -n "$crate_names" ]]; then
            while IFS= read -r crate; do
                [[ -z "$crate" ]] && continue
                echo "${crate}|${ws_schema}|${ws_id}|${ws_name}"
            done <<< "$crate_names"
        else
            log_warn "    No source_files found in schema ${ws_schema}"
        fi
    done <<< "$ws_rows"

    log_ok "Found ${ws_count} active workspaces"
}

# Store the mapping in a temp file for lookup
MAPPING_FILE=""

create_mapping_file() {
    MAPPING_FILE=$(mktemp /tmp/rustbrain-qdrant-migration-XXXXXX.map)
    build_workspace_map > "$MAPPING_FILE"

    local map_count
    map_count=$(wc -l < "$MAPPING_FILE" | tr -d ' ')
    log_ok "Built mapping file with ${map_count} crate_name → workspace entries"

    if [[ "$map_count" -eq 0 ]]; then
        log_warn "No crate_name → workspace mappings found"
    fi
}

# Look up workspace for a given crate_name
lookup_workspace() {
    local crate_name="$1"
    # Search mapping file for exact crate_name match
    local match
    match=$(grep "^${crate_name}|" "$MAPPING_FILE" 2>/dev/null | head -1 || true)
    if [[ -n "$match" ]]; then
        echo "$match"
        return 0
    fi
    return 1
}

# ---------------------------------------------------------------------------
# Qdrant collection operations
# ---------------------------------------------------------------------------
create_workspace_collection() {
    local collection_name="$1"
    local collection_type="$2"  # code_embeddings, doc_embeddings, crate_docs

    # Check if collection already exists
    if curl -sf "${QDRANT_HOST}/collections/${collection_name}" >/dev/null 2>&1; then
        log_info "  Collection '${collection_name}' already exists"
        return 0
    fi

    log_info "  Creating collection '${collection_name}' (${EMBEDDING_DIMENSIONS}-dim Cosine)"

    if [[ "$DRY_RUN" == true ]]; then
        dry_run_msg "Create collection '${collection_name}'"
        return 0
    fi

    # Create collection with same vector config as global
    curl -sf -X PUT "${QDRANT_HOST}/collections/${collection_name}" \
        -H "Content-Type: application/json" \
        -d "{
            \"vectors\": {
                \"size\": ${EMBEDDING_DIMENSIONS},
                \"distance\": \"Cosine\"
            },
            \"optimizers_config\": {
                \"indexing_threshold\": 20000
            }
        }" >/dev/null || {
            log_error "Failed to create collection '${collection_name}'"
            return 1
        }

    log_ok "  Created collection '${collection_name}'"
}

create_payload_indexes() {
    local collection_name="$1"
    local collection_type="$2"

    log_info "  Creating payload indexes for '${collection_name}'"

    if [[ "$DRY_RUN" == true ]]; then
        dry_run_msg "Create indexes for '${collection_name}'"
        return 0
    fi

    local -n indexes_ref
    case "$collection_type" in
        code_embeddings) indexes_ref="CODE_INDEXES" ;;
        doc_embeddings)  indexes_ref="DOC_INDEXES" ;;
        crate_docs)      indexes_ref="CRATE_DOCS_INDEXES" ;;
        *)               return 0 ;;
    esac

    for field in "${!indexes_ref[@]}"; do
        local schema="${indexes_ref[$field]}"
        curl -sf -X PUT "${QDRANT_HOST}/collections/${collection_name}/index" \
            -H "Content-Type: application/json" \
            -d "{\"field_name\": \"${field}\", \"field_schema\": \"${schema}\"}" >/dev/null 2>&1 || true
    done

    log_ok "  Created indexes for '${collection_name}'"
}

# ---------------------------------------------------------------------------
# Scroll + upsert migration
# ---------------------------------------------------------------------------
migrate_collection() {
    local source_collection="$1"
    local total_source=0
    local total_migrated=0
    local total_orphans=0

    # Check source collection exists
    if ! curl -sf "${QDRANT_HOST}/collections/${source_collection}" >/dev/null 2>&1; then
        log_warn "Source collection '${source_collection}' not found — skipping"
        return 0
    fi

    total_source=$(curl -sf "${QDRANT_HOST}/collections/${source_collection}" | jq '.result.points_count // 0')
    log_info "Migrating '${source_collection}' (${total_source} points)"

    # Track which workspace collections we've already created
    local -A created_collections=()

    # Scroll through all points
    local offset="null"
    local batch_num=0

    while true; do
        batch_num=$((batch_num + 1))

        # Build scroll request
        local scroll_body
        if [[ "$offset" == "null" ]]; then
            scroll_body=$(jq -n --argjson limit "$BATCH_SIZE" '{
                limit: $limit,
                with_payload: true,
                with_vector: true
            }')
        else
            scroll_body=$(jq -n --argjson limit "$BATCH_SIZE" --arg offset "$offset" '{
                limit: $limit,
                with_payload: true,
                with_vector: true,
                offset: $offset
            }')
        fi

        local scroll_result
        scroll_result=$(curl -sf -X POST "${QDRANT_HOST}/collections/${source_collection}/points/scroll" \
            -H "Content-Type: application/json" \
            -d "$scroll_body" 2>/dev/null) || {
            log_error "Scroll request failed for '${source_collection}' at offset ${offset}"
            break
        }

        local points
        points=$(echo "$scroll_result" | jq '.result.points // []')
        local point_count
        point_count=$(echo "$points" | jq 'length')

        if [[ "$point_count" -eq 0 ]]; then
            break
        fi

        log_info "  Batch ${batch_num}: ${point_count} points (offset: ${offset})"

        # Group points by workspace
        # For each point, look up workspace via crate_name payload field
        local -A workspace_points=()

        for point_idx in $(seq 0 $((point_count - 1))); do
            local point_json
            point_json=$(echo "$points" | jq ".[$point_idx]")

            local crate_name
            crate_name=$(echo "$point_json" | jq -r '.payload.crate_name // empty')

            local ws_schema="ws_default"
            local ws_id="default"
            local ws_name="default"

            # Try to find workspace mapping
            if [[ -n "$crate_name" ]]; then
                local mapping
                if mapping=$(lookup_workspace "$crate_name"); then
                    ws_schema=$(echo "$mapping" | cut -d'|' -f2)
                    ws_id=$(echo "$mapping" | cut -d'|' -f3)
                    ws_name=$(echo "$mapping" | cut -d'|' -f4)
                else
                    total_orphans=$((total_orphans + 1))
                fi
            else
                total_orphans=$((total_orphans + 1))
            fi

            # Add workspace_id to payload
            local enriched_point
            enriched_point=$(echo "$point_json" | jq --arg ws_id "$ws_id" \
                '.payload.workspace_id = $ws_id')

            # Append to workspace bucket
            local key="${ws_schema}|${source_collection}"
            if [[ -z "${workspace_points[$key]:-}" ]]; then
                workspace_points["$key"]="$enriched_point"
            else
                workspace_points["$key"]="${workspace_points[$key]}
${enriched_point}"
            fi
        done

        # Upsert each workspace's batch
        for key in "${!workspace_points[@]}"; do
            local ws_schema
            ws_schema=$(echo "$key" | cut -d'|' -f1)
            local src_col
            src_col=$(echo "$key" | cut -d'|' -f2)
            local target_collection="${ws_schema}_${src_col}"

            # Create collection if needed
            if [[ -z "${created_collections[$target_collection]:-}" ]]; then
                create_workspace_collection "$target_collection" "$src_col"
                create_payload_indexes "$target_collection" "$src_col"
                created_collections["$target_collection"]=1
            fi

            # Build upsert request from points
            local points_array
            points_array=$(echo "${workspace_points[$key]}" | jq -s '.')

            local upsert_request
            upsert_request=$(jq -n --argjson points "$points_array" '{
                points: $points
            }')

            if [[ "$DRY_RUN" == true ]]; then
                local pt_count
                pt_count=$(echo "$points_array" | jq 'length')
                dry_run_msg "Upsert ${pt_count} points to '${target_collection}'"
            else
                local upsert_result
                upsert_result=$(curl -sf -X PUT \
                    "${QDRANT_HOST}/collections/${target_collection}/points?wait=true" \
                    -H "Content-Type: application/json" \
                    -d "$upsert_request" 2>/dev/null) || {
                    log_error "Upsert failed for '${target_collection}'"
                    continue
                }
            fi
        done

        total_migrated=$((total_migrated + point_count))

        # Get next page offset
        offset=$(echo "$scroll_result" | jq -r '.result.next_page_offset // "null"')

        if [[ "$offset" == "null" ]]; then
            break
        fi

        # Progress indicator
        if [[ $((batch_num % 10)) -eq 0 ]]; then
            local pct=0
            if [[ "$total_source" -gt 0 ]]; then
                pct=$((total_migrated * 100 / total_source))
            fi
            log_info "  Progress: ${total_migrated}/${total_source} (${pct}%)"
        fi
    done

    log_ok "Migrated '${source_collection}': ${total_migrated}/${total_source} points, ${total_orphans} orphans → ws_default"
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------
verify_migration() {
    log_info ""
    log_info "=== Verification ==="

    for src_col in "${COLLECTIONS[@]}"; do
        # Check source collection exists
        if ! curl -sf "${QDRANT_HOST}/collections/${src_col}" >/dev/null 2>&1; then
            continue
        fi

        local src_count
        src_count=$(curl -sf "${QDRANT_HOST}/collections/${src_col}" | jq '.result.points_count // 0')

        # Sum counts across all workspace collections for this type
        local total_target=0
        local collections_json
        collections_json=$(curl -sf "${QDRANT_HOST}/collections")
        local ws_collections
        ws_collections=$(echo "$collections_json" | jq -r ".result.collections[].name | select(endswith(\"_${src_col}\"))")

        while IFS= read -r ws_col; do
            [[ -z "$ws_col" ]] && continue
            local ws_count
            ws_count=$(curl -sf "${QDRANT_HOST}/collections/${ws_col}" | jq '.result.points_count // 0')
            total_target=$((total_target + ws_count))
            log_info "  ${ws_col}: ${ws_count} points"
        done <<< "$ws_collections"

        if [[ "$total_target" -eq "$src_count" ]]; then
            log_ok "  ${src_col}: ${src_count} → ${total_target} ✓ (match)"
        else
            log_warn "  ${src_col}: ${src_count} → ${total_target} ✗ (mismatch — orphans may account for difference)"
        fi
    done
}

# ---------------------------------------------------------------------------
# Delete source collections (optional, dangerous)
# ---------------------------------------------------------------------------
delete_source_collections() {
    if [[ "$DELETE_SOURCE" != true ]]; then
        return 0
    fi

    if [[ "$DRY_RUN" == true ]]; then
        for src_col in "${COLLECTIONS[@]}"; do
            dry_run_msg "Delete global collection '${src_col}'"
        done
        return 0
    fi

    log_warn ""
    log_warn "=== DELETING SOURCE COLLECTIONS ==="
    log_warn "This is irreversible! Global collections will be permanently removed."
    log_warn ""

    for src_col in "${COLLECTIONS[@]}"; do
        if curl -sf "${QDRANT_HOST}/collections/${src_col}" >/dev/null 2>&1; then
            log_info "  Deleting '${src_col}'..."
            curl -sf -X DELETE "${QDRANT_HOST}/collections/${src_col}" >/dev/null 2>&1 || {
                log_error "  Failed to delete '${src_col}'"
                continue
            }
            log_ok "  Deleted '${src_col}'"
        fi
    done
}

# ---------------------------------------------------------------------------
# Summary report
# ---------------------------------------------------------------------------
print_summary() {
    log_info ""
    log_info "=== Migration Summary ==="
    log_info "  Qdrant Host:        ${QDRANT_HOST}"
    log_info "  Vector Dimensions:  ${EMBEDDING_DIMENSIONS}"
    log_info "  Batch Size:         ${BATCH_SIZE}"
    log_info "  Dry Run:            ${DRY_RUN}"
    log_info "  Delete Source:      ${DELETE_SOURCE}"
    log_info ""

    # List all workspace collections
    local collections_json
    collections_json=$(curl -sf "${QDRANT_HOST}/collections" 2>/dev/null || echo '{"result":{"collections":[]}}')

    local ws_cols
    ws_cols=$(echo "$collections_json" | jq -r '.result.collections[] | select(.name | startswith("ws_")) | .name')

    if [[ -n "$ws_cols" ]]; then
        log_info "  Per-workspace collections created:"
        while IFS= read -r col; do
            [[ -z "$col" ]] && continue
            local count
            count=$(curl -sf "${QDRANT_HOST}/collections/${col}" 2>/dev/null | jq '.result.points_count // 0')
            log_info "    ${col}: ${count} points"
        done <<< "$ws_cols"
    else
        log_warn "  No per-workspace collections found"
    fi

    log_info ""
    log_info "  Global collections (preserved as fallback):"
    for src_col in "${COLLECTIONS[@]}"; do
        if curl -sf "${QDRANT_HOST}/collections/${src_col}" >/dev/null 2>&1; then
            local count
            count=$(curl -sf "${QDRANT_HOST}/collections/${src_col}" 2>/dev/null | jq '.result.points_count // 0')
            log_info "    ${src_col}: ${count} points"
        fi
    done

    if [[ "$DELETE_SOURCE" == true && "$DRY_RUN" != true ]]; then
        log_warn "  Global collections have been DELETED"
    fi
}

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
cleanup() {
    if [[ -n "$MAPPING_FILE" && -f "$MAPPING_FILE" ]]; then
        rm -f "$MAPPING_FILE"
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║  RUST-BRAIN — Qdrant Migration: Global → Per-Workspace     ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    echo ""

    if [[ "$DRY_RUN" == true ]]; then
        log_warn "DRY-RUN MODE — no data will be written"
    fi

    check_prerequisites

    echo ""
    log_info "=== Phase 1: Build workspace mapping ==="
    create_mapping_file

    echo ""
    log_info "=== Phase 2: Migrate collections ==="
    for src_col in "${COLLECTIONS[@]}"; do
        migrate_collection "$src_col"
    done

    echo ""
    log_info "=== Phase 3: Verify ==="
    if [[ "$DRY_RUN" != true ]]; then
        verify_migration
    else
        dry_run_msg "Skip verification (dry-run mode)"
    fi

    echo ""
    log_info "=== Phase 4: Cleanup ==="
    delete_source_collections

    print_summary

    echo ""
    if [[ "$DRY_RUN" == true ]]; then
        log_ok "Dry-run complete. Re-run without --dry-run to execute migration."
    else
        log_ok "Migration complete. Verify results and run with --delete-source to remove global collections."
    fi
}

main
