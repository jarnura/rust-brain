#!/usr/bin/env bash
# =============================================================================
# rust-brain — Docker Resource Leak Detector (RUSA-197)
# =============================================================================
# Detects orphaned Docker volumes and containers not tracked in Postgres.
# Outputs Prometheus textfile-collector metrics and a human-readable report.
#
# Usage:
#   ./scripts/leak-detector.sh                # dry-run (report only)
#   ./scripts/leak-detector.sh --cleanup       # remove orphans
#   ./scripts/leak-detector.sh --metrics-only  # write metrics file only
#
# Environment:
#   LEAK_DETECTION_DRY_RUN    "true"|"false"  (default: "true")
#   AUDIT_LOG_RETENTION_DAYS  N               (default: 90)
#   POSTGRES_URL              connection string (default: from .env)
#   PROMETHEUS_TEXTFILE_DIR   metrics output dir (default: /var/lib/node_exporter/textfile_collector)
# =============================================================================
set -euo pipefail

DRY_RUN="${LEAK_DETECTION_DRY_RUN:-true}"
CLEANUP=false
METRICS_ONLY=false
PROMETHEUS_TEXTFILE_DIR="${PROMETHEUS_TEXTFILE_DIR:-/var/lib/node_exporter/textfile_collector}"
METRICS_FILE="${PROMETHEUS_TEXTFILE_DIR}/rustbrain_leaks.prom"
RETENTION_DAYS="${AUDIT_LOG_RETENTION_DAYS:-90}"

for arg in "$@"; do
    case "$arg" in
        --cleanup)  CLEANUP=true; DRY_RUN=false ;;
        --metrics-only) METRICS_ONLY=true ;;
        --dry-run)  DRY_RUN=true ;;
        -h|--help)
            echo "Usage: $0 [--cleanup|--dry-run|--metrics-only]"
            echo "  --cleanup      Remove orphaned resources (default: dry-run)"
            echo "  --dry-run      Report only, no removal (default)"
            echo "  --metrics-only Write Prometheus metrics file only"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ -z "${POSTGRES_URL:-}" ]; then
    if [ -f "$PROJECT_DIR/.env" ]; then
        POSTGRES_URL="$(grep '^POSTGRES_URL=' "$PROJECT_DIR/.env" | cut -d= -f2-)"
    fi
    if [ -z "$POSTGRES_URL" ]; then
        POSTGRES_URL="postgresql://rustbrain:rustbrain@localhost:5432/rustbrain"
    fi
fi

ORPHAN_VOLUMES=0
ORPHAN_CONTAINERS=0
CLEANED_VOLUMES=0
CLEANED_CONTAINERS=0
NOW_EPOCH="$(date +%s)"

log() {
    if [ "$METRICS_ONLY" = false ]; then
        echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"
    fi
}

log "=== rust-brain Leak Detector ==="
log "Mode: $([ "$DRY_RUN" = true ] && echo "DRY-RUN" || echo "CLEANUP")"
log "Postgres: ${POSTGRES_URL%%@*}@***"

# --- Orphaned Volumes ---

ALL_VOLUMES="$(docker volume ls -q --filter label=rustbrain.workspace=true 2>/dev/null || true)"

TRACKED_VOLUMES="$(psql "$POSTGRES_URL" -t -A -c \
    "SELECT volume_name FROM workspaces WHERE volume_name IS NOT NULL AND status != 'archived';" 2>/dev/null || true)"

ORPHAN_VOLUME_LIST=""
if [ -n "$ALL_VOLUMES" ]; then
    while IFS= read -r vol; do
        [ -z "$vol" ] && continue
        if ! echo "$TRACKED_VOLUMES" | grep -qxF "$vol"; then
            ORPHAN_VOLUME_LIST="${ORPHAN_VOLUME_LIST}${vol}"$'\n'
        fi
    done <<< "$ALL_VOLUMES"
fi

ORPHAN_VOLUME_LIST="$(echo "$ORPHAN_VOLUME_LIST" | sed '/^$/d')"
ORPHAN_VOLUMES="$(echo "$ORPHAN_VOLUME_LIST" | grep -c . || echo 0)"

if [ "$ORPHAN_VOLUMES" -gt 0 ]; then
    log "ORPHANED VOLUMES ($ORPHAN_VOLUMES):"
    echo "$ORPHAN_VOLUME_LIST" | while IFS= read -r vol; do
        [ -z "$vol" ] && continue
        SIZE="$(docker volume inspect "$vol" --format '{{ .Options.size }}' 2>/dev/null || echo "unknown")"
        log "  - $vol (size: $SIZE)"
        if [ "$CLEANUP" = true ]; then
            if docker volume rm "$vol" 2>/dev/null; then
                CLEANED_VOLUMES=$((CLEANED_VOLUMES + 1))
                log "    REMOVED: $vol"
            else
                log "    FAILED to remove: $vol" >&2
            fi
        fi
    done
else
    log "No orphaned volumes found."
fi

# --- Orphaned Containers ---

ALL_CONTAINERS="$(docker ps -a --filter "name=rustbrain-exec-" --format '{{.ID}} {{.Names}}' 2>/dev/null || true)"

TRACKED_CONTAINER_IDS="$(psql "$POSTGRES_URL" -t -A -c \
    "SELECT DISTINCT container_id FROM executions WHERE container_id IS NOT NULL AND status IN ('running', 'pending');" 2>/dev/null || true)"

ORPHAN_CONTAINER_LIST=""
if [ -n "$ALL_CONTAINERS" ]; then
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        cid="$(echo "$line" | awk '{print $1}')"
        cname="$(echo "$line" | awk '{print $2}')"
        if ! echo "$TRACKED_CONTAINER_IDS" | grep -qxF "$cid"; then
            ORPHAN_CONTAINER_LIST="${ORPHAN_CONTAINER_LIST}${cid} ${cname}"$'\n'
        fi
    done <<< "$ALL_CONTAINERS"
fi

ORPHAN_CONTAINER_LIST="$(echo "$ORPHAN_CONTAINER_LIST" | sed '/^$/d')"
ORPHAN_CONTAINERS="$(echo "$ORPHAN_CONTAINER_LIST" | grep -c . || echo 0)"

if [ "$ORPHAN_CONTAINERS" -gt 0 ]; then
    log "ORPHANED CONTAINERS ($ORPHAN_CONTAINERS):"
    echo "$ORPHAN_CONTAINER_LIST" | while IFS= read -r line; do
        [ -z "$line" ] && continue
        log "  - $line"
        cid="$(echo "$line" | awk '{print $1}')"
        if [ "$CLEANUP" = true ]; then
            if docker rm -f "$cid" 2>/dev/null; then
                CLEANED_CONTAINERS=$((CLEANED_CONTAINERS + 1))
                log "    REMOVED: $cid"
            else
                log "    FAILED to remove: $cid" >&2
            fi
        fi
    done
else
    log "No orphaned containers found."
fi

# --- Audit Log Retention ---

if [ "$METRICS_ONLY" = false ]; then
    log ""
    log "Audit log retention: pruning entries older than $RETENTION_DAYS days..."
    DELETED="$(psql "$POSTGRES_URL" -t -A -c \
        "DELETE FROM workspace_audit_log WHERE created_at < now() - interval '${RETENTION_DAYS} days';" 2>/dev/null || echo "0")"
    log "Pruned $DELETED audit log entries (retention: ${RETENTION_DAYS}d)"
fi

# --- Prometheus Metrics ---

mkdir -p "$PROMETHEUS_TEXTFILE_DIR" 2>/dev/null || true

cat > "${METRICS_FILE}.tmp" <<EOF
# HELP rustbrain_leak_orphan_volumes_total Number of Docker volumes not tracked in Postgres
# TYPE rustbrain_leak_orphan_volumes_total gauge
rustbrain_leak_orphan_volumes_total ${ORPHAN_VOLUMES}

# HELP rustbrain_leak_orphan_containers_total Number of Docker containers not tracked in Postgres
# TYPE rustbrain_leak_orphan_containers_total gauge
rustbrain_leak_orphan_containers_total ${ORPHAN_CONTAINERS}

# HELP rustbrain_leak_detection_timestamp_seconds Unix timestamp of last leak detection run
# TYPE rustbrain_leak_detection_timestamp_seconds gauge
rustbrain_leak_detection_timestamp_seconds ${NOW_EPOCH}

# HELP rustbrain_leak_cleanup_volumes_removed_total Number of orphaned volumes removed in this run
# TYPE rustbrain_leak_cleanup_volumes_removed_total gauge
rustbrain_leak_cleanup_volumes_removed_total ${CLEANED_VOLUMES}

# HELP rustbrain_leak_cleanup_containers_removed_total Number of orphaned containers removed in this run
# TYPE rustbrain_leak_cleanup_containers_removed_total gauge
rustbrain_leak_cleanup_containers_removed_total ${CLEANED_CONTAINERS}
EOF

mv "${METRICS_FILE}.tmp" "$METRICS_FILE"

log ""
log "Summary: ${ORPHAN_VOLUMES} orphan volumes, ${ORPHAN_CONTAINERS} orphan containers"
if [ "$CLEANUP" = true ]; then
    log "Cleaned: ${CLEANED_VOLUMES} volumes, ${CLEANED_CONTAINERS} containers"
fi
log "Metrics written to: $METRICS_FILE"

if [ "$ORPHAN_VOLUMES" -gt 0 ] || [ "$ORPHAN_CONTAINERS" -gt 0 ]; then
    if [ "$DRY_RUN" = true ]; then
        log "Re-run with --cleanup to remove orphans, or set LEAK_DETECTION_DRY_RUN=false"
    fi
    exit 1
fi

exit 0
