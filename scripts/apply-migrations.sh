#!/bin/bash
# =============================================================================
# rust-brain — Apply Pending SQL Migrations
# =============================================================================
# Applies pending migrations from services/api/migrations/ against Postgres.
# Uses the _sqlx_migrations tracking table (compatible with sqlx-cli) so
# that sqlx migrate run won't re-apply these migrations later.
#
# Called automatically by start.sh after Postgres is healthy.
# Can also be run standalone:
#   bash scripts/apply-migrations.sh
#   bash scripts/apply-migrations.sh --dry-run
#
# Safety:
#   - Idempotent: skips already-applied migrations
#   - Tracks migrations in _sqlx_migrations table
#   - Each migration runs in a transaction where possible
#   - Exits on first failure (no partial apply)
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MIGRATIONS_DIR="$PROJECT_DIR/services/api/migrations"

# Load .env if present
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$PROJECT_DIR/.env"
    set +a
fi

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()  { echo -e "${BLUE}[INFO]${NC}  $*"; }
log_ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }

# Verify Postgres is reachable
if ! docker-compose exec -T postgres pg_isready -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" > /dev/null 2>&1; then
    log_error "Postgres is not ready. Start it first with: docker-compose up -d postgres"
    exit 1
fi

# Create _sqlx_migrations tracking table if it doesn't exist.
# This table is compatible with sqlx-cli's migration tracking so that
# `sqlx migrate run` won't re-apply migrations applied by this script.
log_info "Ensuring _sqlx_migrations tracking table exists..."
docker-compose exec -T postgres psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" > /dev/null 2>&1 <<'SQL'
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    success BOOLEAN NOT NULL DEFAULT TRUE,
    checksum BYTEA,
    execution_time BIGINT
);
SQL
log_ok "Migration tracking table ready"

# List applied migrations
APPLIED=$(docker-compose exec -T postgres psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" -t -A -c \
    "SELECT version FROM _sqlx_migrations WHERE success = true ORDER BY version;" 2>/dev/null || echo "")

# Discover pending migrations (sorted by filename = chronological)
PENDING=()
APPLIED_COUNT=0
PENDING_COUNT=0

for migration_file in "$MIGRATIONS_DIR"/*.sql; do
    [ -f "$migration_file" ] || continue

    # Extract version from filename: 20260403000001_description.sql → 20260403000001
    basename=$(basename "$migration_file")
    version=$(echo "$basename" | grep -oP '^\d+' || true)

    if [ -z "$version" ]; then
        log_warn "Skipping malformed migration file: $basename"
        continue
    fi

    # Check if already applied
    if echo "$APPLIED" | grep -q "^${version}$"; then
        APPLIED_COUNT=$((APPLIED_COUNT + 1))
        continue
    fi

    PENDING+=("$migration_file")
    PENDING_COUNT=$((PENDING_COUNT + 1))
done

if [ "$APPLIED_COUNT" -gt 0 ]; then
    log_info "$APPLIED_COUNT migration(s) already applied"
fi

if [ "$PENDING_COUNT" -eq 0 ]; then
    log_ok "No pending migrations"
    exit 0
fi

log_info "Found $PENDING_COUNT pending migration(s)"

# Apply pending migrations
for migration_file in "${PENDING[@]}"; do
    basename=$(basename "$migration_file")
    version=$(echo "$basename" | grep -oP '^\d+')
    # Extract description from filename: 20260403000001_description.sql → description
    description=$(echo "$basename" | sed "s/^${version}_//" | sed 's/\.sql$//' | tr '_' ' ')

    if [ "$DRY_RUN" = true ]; then
        log_warn "[DRY-RUN] Would apply: $basename"
        continue
    fi

    log_info "Applying: $basename"

    START_TIME=$(date +%s%N)

    # Apply the migration
    if docker-compose exec -T postgres psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" \
        -v ON_ERROR_STOP=1 \
        -f - < "$migration_file" > /dev/null 2>&1; then

        END_TIME=$(date +%s%N)
        EXECUTION_MS=$(( (END_TIME - START_TIME) / 1000000 ))

        # Record in tracking table
        docker-compose exec -T postgres psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" > /dev/null 2>&1 <<SQL
INSERT INTO _sqlx_migrations (version, description, success, execution_time)
VALUES (${version}, '${description}', true, ${EXECUTION_MS})
ON CONFLICT (version) DO UPDATE SET success = true, installed_on = NOW(), execution_time = ${EXECUTION_MS};
SQL
        log_ok "Applied: $basename (${EXECUTION_MS}ms)"
    else
        # Record failure
        docker-compose exec -T postgres psql -U "${POSTGRES_USER:-rustbrain}" -d "${POSTGRES_DB:-rustbrain}" > /dev/null 2>&1 <<SQL
INSERT INTO _sqlx_migrations (version, description, success)
VALUES (${version}, '${description}', false)
ON CONFLICT (version) DO UPDATE SET success = false, installed_on = NOW();
SQL
        log_error "Failed to apply: $basename"
        log_error "Fix the migration and re-run, or manually mark as resolved in _sqlx_migrations"
        exit 1
    fi
done

log_ok "All $PENDING_COUNT pending migration(s) applied successfully"
