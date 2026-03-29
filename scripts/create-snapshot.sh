#!/bin/bash
# =============================================================================
# rust-brain — Create Snapshot
# =============================================================================
# Exports all three databases into a distributable snapshot bundle.
# The resulting .tar.zst can be shared with teammates or published to
# GitHub Releases for use with run-with-snapshot.sh.
#
# Usage:
#   ./scripts/create-snapshot.sh                    # default name "custom"
#   ./scripts/create-snapshot.sh hyperswitch        # named snapshot
#   ./scripts/create-snapshot.sh hyperswitch a1b2c3d # with git commit
#   ./scripts/create-snapshot.sh --help
#
# Prerequisites:
#   - All three DB services running (postgres, neo4j, qdrant)
#   - zstd installed (apt install zstd / brew install zstd)
#   - jq installed (apt install jq / brew install jq)
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Defaults
SNAPSHOT_NAME="${1:-custom}"
SOURCE_COMMIT="${2:-unknown}"
OUTPUT_DIR="${PROJECT_ROOT}/dist"

# Help
if [[ "${SNAPSHOT_NAME}" == "--help" || "${SNAPSHOT_NAME}" == "-h" ]]; then
  cat <<'EOF'
rust-brain — Create Snapshot

USAGE:
    create-snapshot.sh [NAME] [COMMIT_SHA]

ARGUMENTS:
    NAME          Snapshot name (default: "custom")
    COMMIT_SHA    Source repo git commit (default: "unknown")

EXAMPLES:
    create-snapshot.sh hyperswitch a1b2c3d4
    create-snapshot.sh my-project

OUTPUT:
    dist/rustbrain-snapshot-<name>.tar.zst
    dist/rustbrain-snapshot-<name>.tar.zst.sha256

WHAT'S INCLUDED:
    1. PostgreSQL dump (pg_dump --format=custom)
    2. Neo4j database dump (neo4j-admin database dump) — requires ~3s downtime
    3. Qdrant collection snapshots (code_embeddings + doc_embeddings)
    4. manifest.json with metadata, checksums, and stats
EOF
  exit 0
fi

cd "$PROJECT_ROOT"

# Load environment
if [ -f .env ]; then
  # shellcheck disable=SC1091
  source .env
else
  echo -e "${RED}ERROR: .env file not found${NC}"
  exit 1
fi

# Check prerequisites
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║         RUST-BRAIN — Create Snapshot                         ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

echo -e "${CYAN}=== Checking prerequisites ===${NC}"

for cmd in zstd jq docker; do
  if ! command -v "$cmd" &>/dev/null; then
    echo -e "${RED}ERROR: ${cmd} not found. Install it first.${NC}"
    exit 1
  fi
  echo -e "  ${GREEN}✓${NC} ${cmd}"
done

# Check services running (use docker compose ps to handle prefixed container names)
for svc in postgres neo4j qdrant; do
  SVC_STATUS=$(docker compose ps --status running --format "{{.Service}}" 2>/dev/null | grep -c "^${svc}$" || true)
  if [ "$SVC_STATUS" -eq 0 ]; then
    echo -e "${RED}ERROR: ${svc} service is not running${NC}"
    echo "Start services first: bash scripts/start.sh"
    exit 1
  fi
  echo -e "  ${GREEN}✓${NC} ${svc} running"
done

# Resolve compose project name for volume naming
COMPOSE_PROJECT="${COMPOSE_PROJECT_NAME:-rustbrain}"
NEO4J_VOLUME="${COMPOSE_PROJECT}_neo4j_data"

# Verify the volume exists
if ! docker volume inspect "$NEO4J_VOLUME" &>/dev/null; then
  echo -e "${RED}ERROR: Volume ${NEO4J_VOLUME} not found${NC}"
  echo "  Available volumes: $(docker volume ls --format '{{.Name}}' | grep neo4j)"
  exit 1
fi

# Create working directory (world-writable so Docker containers can write to bind mounts)
WORK_DIR=$(mktemp -d)
chmod 777 "$WORK_DIR"
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir -p "$WORK_DIR/qdrant" "$OUTPUT_DIR"

echo ""
echo -e "${CYAN}=== Phase 1: PostgreSQL dump ===${NC}"

docker compose exec -T postgres pg_dump \
  -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  --format=custom --compress=6 \
  --no-owner --no-privileges \
  > "${WORK_DIR}/postgres.pgdump"

PG_SIZE=$(du -h "${WORK_DIR}/postgres.pgdump" | cut -f1)
PG_SHA=$(sha256sum "${WORK_DIR}/postgres.pgdump" | cut -d' ' -f1)
echo -e "  ${GREEN}✓${NC} PostgreSQL: ${PG_SIZE}"

echo ""
echo -e "${CYAN}=== Phase 2: Neo4j dump (brief downtime) ===${NC}"

# Cold dump: stop neo4j, dump via ephemeral container, restart
docker compose stop neo4j
echo "  Neo4j stopped"

# Use the same neo4j image to run neo4j-admin database dump
# neo4j-admin needs write access to /snapshot; run as root to avoid permission issues
docker run --rm --user root \
  -v "${NEO4J_VOLUME}:/data" \
  -v "${WORK_DIR}:/snapshot" \
  neo4j:5-community \
  bash -c "neo4j-admin database dump neo4j --to-path=/snapshot/ && chmod 644 /snapshot/neo4j.dump"

docker compose up -d neo4j
echo "  Neo4j restarting..."

# Wait for Neo4j to come back
for i in $(seq 1 60); do
  if docker compose exec -T neo4j cypher-shell -u neo4j -p "${NEO4J_PASSWORD}" \
     "RETURN 1" &>/dev/null 2>&1; then
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo -e "${RED}WARNING: Neo4j slow to restart, continuing...${NC}"
  fi
  sleep 1
done

NEO_SIZE=$(du -h "${WORK_DIR}/neo4j.dump" | cut -f1)
NEO_SHA=$(sha256sum "${WORK_DIR}/neo4j.dump" | cut -d' ' -f1)
echo -e "  ${GREEN}✓${NC} Neo4j: ${NEO_SIZE}"

echo ""
echo -e "${CYAN}=== Phase 3: Qdrant snapshots ===${NC}"

QDRANT_PORT="${QDRANT_REST_PORT:-6333}"
QDRANT_URL="http://localhost:${QDRANT_PORT}"

for collection in code_embeddings doc_embeddings; do
  # Create snapshot
  SNAP_RESP=$(curl -sf -X POST "${QDRANT_URL}/collections/${collection}/snapshots" 2>/dev/null)
  SNAP_NAME=$(echo "$SNAP_RESP" | jq -r '.result.name // empty')

  if [ -z "$SNAP_NAME" ]; then
    echo -e "  ${YELLOW}⚠${NC} ${collection}: no data or snapshot failed, skipping"
    continue
  fi

  # Download snapshot file
  curl -sf -o "${WORK_DIR}/qdrant/${collection}.snapshot" \
    "${QDRANT_URL}/collections/${collection}/snapshots/${SNAP_NAME}"

  QSIZE=$(du -h "${WORK_DIR}/qdrant/${collection}.snapshot" | cut -f1)
  QSHA=$(sha256sum "${WORK_DIR}/qdrant/${collection}.snapshot" | cut -d' ' -f1)
  echo -e "  ${GREEN}✓${NC} ${collection}: ${QSIZE}"

  # Clean up remote snapshot
  curl -sf -X DELETE "${QDRANT_URL}/collections/${collection}/snapshots/${SNAP_NAME}" &>/dev/null || true
done

echo ""
echo -e "${CYAN}=== Phase 4: Gathering stats ===${NC}"

# Query counts from PostgreSQL
TOTAL_ITEMS=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  -tAc "SELECT count(*) FROM extracted_items" 2>/dev/null | tr -d '[:space:]' || echo "0")
FN_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  -tAc "SELECT count(*) FROM extracted_items WHERE item_type='function'" 2>/dev/null | tr -d '[:space:]' || echo "0")
STRUCT_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  -tAc "SELECT count(*) FROM extracted_items WHERE item_type='struct'" 2>/dev/null | tr -d '[:space:]' || echo "0")
TRAIT_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  -tAc "SELECT count(*) FROM extracted_items WHERE item_type='trait'" 2>/dev/null | tr -d '[:space:]' || echo "0")
MODULE_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
  -tAc "SELECT count(*) FROM extracted_items WHERE item_type='module'" 2>/dev/null | tr -d '[:space:]' || echo "0")

echo -e "  Items: ${TOTAL_ITEMS} total (${FN_COUNT} fn, ${STRUCT_COUNT} struct, ${TRAIT_COUNT} trait, ${MODULE_COUNT} mod)"

# Qdrant point counts
CODE_POINTS=$(curl -sf "${QDRANT_URL}/collections/code_embeddings" 2>/dev/null \
  | jq -r '.result.points_count // 0')
DOC_POINTS=$(curl -sf "${QDRANT_URL}/collections/doc_embeddings" 2>/dev/null \
  | jq -r '.result.points_count // 0')
echo -e "  Embeddings: ${CODE_POINTS} code + ${DOC_POINTS} doc"

echo ""
echo -e "${CYAN}=== Phase 5: Generating manifest ===${NC}"

# Compute schema hash from init files (some may have restricted permissions from Docker)
SCHEMA_HASH=$(cat scripts/init-db.sql scripts/init-qdrant.sh 2>/dev/null; \
  docker compose exec -T neo4j cat /var/lib/neo4j/import/init.cypher 2>/dev/null || true)
SCHEMA_HASH=$(echo "$SCHEMA_HASH" | sha256sum | cut -c1-8)

# Compute individual artifact checksums
CODE_SNAP_SHA=""
CODE_SNAP_SIZE=0
DOC_SNAP_SHA=""
DOC_SNAP_SIZE=0

if [ -f "${WORK_DIR}/qdrant/code_embeddings.snapshot" ]; then
  CODE_SNAP_SHA=$(sha256sum "${WORK_DIR}/qdrant/code_embeddings.snapshot" | cut -d' ' -f1)
  CODE_SNAP_SIZE=$(stat -c%s "${WORK_DIR}/qdrant/code_embeddings.snapshot" 2>/dev/null \
    || stat -f%z "${WORK_DIR}/qdrant/code_embeddings.snapshot" 2>/dev/null || echo "0")
fi
if [ -f "${WORK_DIR}/qdrant/doc_embeddings.snapshot" ]; then
  DOC_SNAP_SHA=$(sha256sum "${WORK_DIR}/qdrant/doc_embeddings.snapshot" | cut -d' ' -f1)
  DOC_SNAP_SIZE=$(stat -c%s "${WORK_DIR}/qdrant/doc_embeddings.snapshot" 2>/dev/null \
    || stat -f%z "${WORK_DIR}/qdrant/doc_embeddings.snapshot" 2>/dev/null || echo "0")
fi

PG_DUMP_SIZE=$(stat -c%s "${WORK_DIR}/postgres.pgdump" 2>/dev/null \
  || stat -f%z "${WORK_DIR}/postgres.pgdump" 2>/dev/null || echo "0")
NEO_DUMP_SIZE=$(stat -c%s "${WORK_DIR}/neo4j.dump" 2>/dev/null \
  || stat -f%z "${WORK_DIR}/neo4j.dump" 2>/dev/null || echo "0")

cat > "${WORK_DIR}/manifest.json" <<MANIFEST
{
  "version": "1.0.0",
  "format_version": 1,
  "created_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "source": {
    "name": "${SNAPSHOT_NAME}",
    "commit": "${SOURCE_COMMIT}",
    "indexed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  },
  "embedding": {
    "model": "${EMBEDDING_MODEL:-nomic-embed-text}",
    "dimensions": ${EMBEDDING_DIMENSIONS:-768},
    "distance": "Cosine",
    "provider": "ollama"
  },
  "schema": {
    "version": "1.0.0",
    "postgres_migrations": ["001", "002", "003"],
    "hash": "${SCHEMA_HASH}"
  },
  "artifacts": {
    "postgres": {
      "file": "postgres.pgdump",
      "format": "pg_dump_custom",
      "size_bytes": ${PG_DUMP_SIZE},
      "sha256": "${PG_SHA}",
      "pg_version": "16"
    },
    "neo4j": {
      "file": "neo4j.dump",
      "format": "neo4j_admin_dump",
      "size_bytes": ${NEO_DUMP_SIZE},
      "sha256": "${NEO_SHA}",
      "neo4j_version": "5"
    },
    "qdrant": {
      "code_embeddings": {
        "file": "qdrant/code_embeddings.snapshot",
        "size_bytes": ${CODE_SNAP_SIZE},
        "sha256": "${CODE_SNAP_SHA}",
        "points_count": ${CODE_POINTS}
      },
      "doc_embeddings": {
        "file": "qdrant/doc_embeddings.snapshot",
        "size_bytes": ${DOC_SNAP_SIZE},
        "sha256": "${DOC_SNAP_SHA}",
        "points_count": ${DOC_POINTS}
      }
    }
  },
  "stats": {
    "total_items": ${TOTAL_ITEMS},
    "functions": ${FN_COUNT},
    "structs": ${STRUCT_COUNT},
    "traits": ${TRAIT_COUNT},
    "modules": ${MODULE_COUNT},
    "code_embeddings": ${CODE_POINTS},
    "doc_embeddings": ${DOC_POINTS}
  },
  "compatibility": {
    "min_rustbrain_version": "0.1.0",
    "docker_compose_version": "2.20.0"
  }
}
MANIFEST

echo -e "  ${GREEN}✓${NC} manifest.json generated"

echo ""
echo -e "${CYAN}=== Phase 6: Integrity check ===${NC}"

# Sanity: Qdrant points should roughly match Postgres items
if [ "$CODE_POINTS" -gt 0 ] && [ "$TOTAL_ITEMS" -gt 0 ]; then
  RATIO=$((CODE_POINTS * 100 / TOTAL_ITEMS))
  if [ "$RATIO" -lt 50 ] || [ "$RATIO" -gt 200 ]; then
    echo -e "  ${YELLOW}⚠${NC} Qdrant/Postgres count mismatch: ${CODE_POINTS} vectors vs ${TOTAL_ITEMS} items (${RATIO}%)"
    echo -e "  ${YELLOW}  This may indicate an incomplete ingestion${NC}"
  else
    echo -e "  ${GREEN}✓${NC} Data consistency check passed (${RATIO}% coverage)"
  fi
else
  echo -e "  ${YELLOW}⚠${NC} Skipping integrity check (insufficient data)"
fi

echo ""
echo -e "${CYAN}=== Phase 7: Bundling ===${NC}"

OUTPUT_FILE="${OUTPUT_DIR}/rustbrain-snapshot-${SNAPSHOT_NAME}.tar.zst"

tar -cf - -C "$WORK_DIR" . | zstd -3 -T0 -o "$OUTPUT_FILE"

TOTAL_SIZE=$(du -h "$OUTPUT_FILE" | cut -f1)

# Generate checksum file
sha256sum "$OUTPUT_FILE" > "${OUTPUT_FILE}.sha256"

echo -e "  ${GREEN}✓${NC} Bundle: ${TOTAL_SIZE}"

echo ""
echo "╔══════════════════════════════════════════════════════════════╗"
echo "║                 SNAPSHOT CREATED                             ║"
echo "╠══════════════════════════════════════════════════════════════╣"
echo "║                                                              ║"
printf "║  %-58s ║\n" "File: ${OUTPUT_FILE}"
printf "║  %-58s ║\n" "Size: ${TOTAL_SIZE}"
printf "║  %-58s ║\n" "Items: ${TOTAL_ITEMS}"
printf "║  %-58s ║\n" "Embeddings: $((CODE_POINTS + DOC_POINTS))"
echo "║                                                              ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""
echo "Share with teammates:"
echo "  1. Upload to GitHub Releases or shared storage"
echo "  2. They run:"
echo "     ./scripts/run-with-snapshot.sh --snapshot-url=<URL>"
echo ""
