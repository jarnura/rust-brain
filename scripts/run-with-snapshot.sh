#!/bin/bash
# =============================================================================
# rust-brain — Run with Pre-built Snapshot
# =============================================================================
# Downloads a pre-built snapshot of all three databases, restores them,
# and starts the API + MCP-SSE server. No ingestion or Ollama required.
#
# Usage:
#   ./scripts/run-with-snapshot.sh
#   ./scripts/run-with-snapshot.sh --force-refresh
#   ./scripts/run-with-snapshot.sh --snapshot-url=https://example.com/snap.tar.zst
#   ./scripts/run-with-snapshot.sh --local=/path/to/snapshot.tar.zst
#   ./scripts/run-with-snapshot.sh --help
#
# Prerequisites:
#   - Docker (>= 24.0) with Compose V2 plugin
#   - ~8GB free RAM, ~4GB free disk
#   - zstd (auto-installs on Debian/macOS if missing)
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# Defaults
# GitHub repo for gh release download (works for both public and private repos)
GITHUB_REPO="${GITHUB_REPO:-jarnura/rust-brain}"
RELEASE_TAG="${RELEASE_TAG:-snapshot-latest}"
SNAPSHOT_ASSET="rustbrain-snapshot-hyperswitch.tar.zst"
# Fallback URL for direct download (only works for public repos)
DEFAULT_SNAPSHOT_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}/${SNAPSHOT_ASSET}"
SNAPSHOT_URL="${SNAPSHOT_URL:-$DEFAULT_SNAPSHOT_URL}"
SNAPSHOT_DIR="${PROJECT_ROOT}/.snapshots"
SNAPSHOT_MARKER="${SNAPSHOT_DIR}/.restored"
FORCE_REFRESH=false
LOCAL_SNAPSHOT=""

# Parse arguments
for arg in "$@"; do
  case "$arg" in
    --force-refresh)
      FORCE_REFRESH=true
      ;;
    --snapshot-url=*)
      SNAPSHOT_URL="${arg#*=}"
      ;;
    --local=*)
      LOCAL_SNAPSHOT="${arg#*=}"
      ;;
    --help|-h)
      cat <<'EOF'
rust-brain — Run with Pre-built Snapshot

USAGE:
    run-with-snapshot.sh [OPTIONS]

OPTIONS:
    --force-refresh             Re-download and restore even if snapshot exists
                                (required when upgrading to a new snapshot version)
    --snapshot-url=URL          Custom snapshot download URL
    --local=/path/to/snap.zst  Use a local snapshot file (skip download)
    --help, -h                 Show this help

PREREQUISITES:
    - Docker (>= 24.0) with Compose V2 plugin
    - ~8GB free RAM, ~4GB free disk
    - Ports 5432, 7474, 7687, 6333, 8088, 3001 available
    - zstd (brew install zstd on macOS, apt install zstd on Linux)

WHAT THIS DOES:
    1. Downloads the pre-built snapshot (~3 GB, split into 2 parts)
    2. Starts PostgreSQL, Neo4j, and Qdrant containers
    3. Restores all three databases from the snapshot
    4. Starts the API server and MCP-SSE server
    5. No Ollama, no ingestion, no GPU needed

UPGRADING:
    When a new snapshot is released, run with --force-refresh:
        ./scripts/run-with-snapshot.sh --force-refresh
    Without this flag, the script reuses the cached local snapshot.

AFTER STARTUP:
    - Playground UI:  http://localhost:8088/
    - MCP-SSE:        http://localhost:3001/sse
    - Neo4j Browser:  http://localhost:7474

CONNECT YOUR IDE:
    Add to ~/.claude.json or .claude.json:
    {
      "mcpServers": {
        "rust-brain": {
          "type": "sse",
          "url": "http://localhost:3001/sse"
        }
      }
    }
EOF
      exit 0
      ;;
    *)
      echo -e "${RED}Unknown option: ${arg}${NC}"
      echo "Run with --help for usage"
      exit 1
      ;;
  esac
done

cd "$PROJECT_ROOT"

# ═══════════════════════════════════════════════════════════════════════════════
# STEP 0: Prerequisites
# ═══════════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║         RUST-BRAIN — Run with Snapshot                       ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""

echo -e "${CYAN}=== Checking prerequisites ===${NC}"

# Docker
if ! command -v docker &>/dev/null; then
  echo -e "${RED}ERROR: Docker not found.${NC}"
  echo "  Install: https://docs.docker.com/get-docker/"
  exit 1
fi
DOCKER_VERSION=$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo "unknown")
echo -e "  ${GREEN}✓${NC} Docker ${DOCKER_VERSION}"

# Docker daemon
if ! docker info &>/dev/null 2>&1; then
  echo -e "${RED}ERROR: Docker daemon not running.${NC}"
  echo "  Start Docker Desktop or run: sudo systemctl start docker"
  exit 1
fi

# Compose V2
if ! docker compose version &>/dev/null; then
  echo -e "${RED}ERROR: Docker Compose V2 not found.${NC}"
  echo "  Install: https://docs.docker.com/compose/install/"
  exit 1
fi
COMPOSE_VERSION=$(docker compose version --short 2>/dev/null || echo "unknown")
echo -e "  ${GREEN}✓${NC} Docker Compose ${COMPOSE_VERSION}"

# zstd
if ! command -v zstd &>/dev/null; then
  echo -e "${YELLOW}  Installing zstd...${NC}"
  if command -v apt-get &>/dev/null; then
    sudo apt-get install -y zstd >/dev/null 2>&1
  elif command -v brew &>/dev/null; then
    brew install zstd >/dev/null 2>&1
  else
    echo -e "${RED}ERROR: zstd not found. Install: https://github.com/facebook/zstd${NC}"
    exit 1
  fi
fi
echo -e "  ${GREEN}✓${NC} zstd"

# jq
if ! command -v jq &>/dev/null; then
  echo -e "${YELLOW}  Installing jq...${NC}"
  if command -v apt-get &>/dev/null; then
    sudo apt-get install -y jq >/dev/null 2>&1
  elif command -v brew &>/dev/null; then
    brew install jq >/dev/null 2>&1
  else
    echo -e "${YELLOW}⚠ jq not found — manifest parsing will be limited${NC}"
  fi
fi

# .env file
if [ ! -f .env ]; then
  if [ -f .env.example ]; then
    cp .env.example .env
    # Set safe defaults for snapshot mode (no placeholder passwords)
    sed -i.bak \
      -e 's/<your-password>/rustbrain_dev_2024/g' \
      -e 's/<your-readonly-password>/rustbrain_readonly_dev_2024/g' \
      -e 's/your-api-key-here//g' \
      .env 2>/dev/null || true
    rm -f .env.bak
    echo -e "  ${GREEN}✓${NC} Created .env from .env.example with snapshot defaults"
  else
    echo -e "${RED}ERROR: No .env or .env.example found${NC}"
    exit 1
  fi
fi

# macOS: auto-apply GPU-free Docker override (Docker Desktop has no NVIDIA support)
if [ "$(uname -s)" = "Darwin" ] && [ ! -f docker-compose.override.yml ]; then
  if [ -f docker-compose.macos.yml ]; then
    cp docker-compose.macos.yml docker-compose.override.yml
    echo -e "  ${GREEN}✓${NC} Applied macOS override (no NVIDIA GPU)"
  else
    # Create minimal override inline
    cat > docker-compose.override.yml <<'OVERRIDE'
services:
  ollama:
    deploy:
      resources:
        limits:
          memory: 16G
OVERRIDE
    echo -e "  ${GREEN}✓${NC} Created macOS override (no NVIDIA GPU)"
  fi
fi

# shellcheck disable=SC1091
source .env

# Disk space check (need ~2GB)
AVAIL_KB=$(df -k "$PROJECT_ROOT" 2>/dev/null | tail -1 | awk '{print $4}')
if [ -n "$AVAIL_KB" ] && [ "$AVAIL_KB" -lt 2000000 ] 2>/dev/null; then
  echo -e "${YELLOW}⚠ Less than 2GB disk space available${NC}"
fi

# Port availability warnings
for port_var in POSTGRES_PORT:5432 NEO4J_HTTP_PORT:7474 NEO4J_BOLT_PORT:7687 \
                QDRANT_REST_PORT:6333 API_PORT:8088 MCP_SSE_PORT:3001; do
  port_name="${port_var%%:*}"
  port_default="${port_var##*:}"
  port_val="${!port_name:-$port_default}"
  if command -v lsof &>/dev/null && lsof -i ":${port_val}" &>/dev/null 2>&1; then
    echo -e "  ${YELLOW}⚠${NC} Port ${port_val} (${port_name}) is already in use"
  fi
done

echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# STEP 1: Obtain Snapshot
# ═══════════════════════════════════════════════════════════════════════════════

mkdir -p "$SNAPSHOT_DIR"
SNAPSHOT_FILE="${SNAPSHOT_DIR}/rustbrain-snapshot.tar.zst"
NEED_RESTORE=false

if [ "$FORCE_REFRESH" = true ] || [ ! -f "$SNAPSHOT_MARKER" ]; then
  NEED_RESTORE=true

  if [ -n "$LOCAL_SNAPSHOT" ]; then
    # Use local file
    if [ ! -f "$LOCAL_SNAPSHOT" ]; then
      echo -e "${RED}ERROR: Local snapshot not found: ${LOCAL_SNAPSHOT}${NC}"
      exit 1
    fi
    echo -e "${CYAN}=== Using local snapshot ===${NC}"
    cp "$LOCAL_SNAPSHOT" "$SNAPSHOT_FILE"
    echo -e "  ${GREEN}✓${NC} Copied $(du -h "$SNAPSHOT_FILE" | cut -f1)"

  elif [ ! -f "$SNAPSHOT_FILE" ] || [ "$FORCE_REFRESH" = true ]; then
    echo -e "${CYAN}=== Downloading snapshot (~3 GB) ===${NC}"
    echo ""

    DOWNLOAD_OK=false
    BASE_URL="https://github.com/${GITHUB_REPO}/releases/download/${RELEASE_TAG}"

    # Method 1: curl with progress bar (preferred — works everywhere, shows progress)
    echo -e "  Downloading split parts with progress bar..."
    echo ""
    PARTS_DOWNLOADED=0

    for SUFFIX in aa ab ac ad ae; do
      PART_URL="${BASE_URL}/${SNAPSHOT_ASSET}.part-${SUFFIX}"
      PART_FILE="${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-${SUFFIX}"

      echo -e "  ${BOLD}part-${SUFFIX}${NC}:"
      if curl -fL --progress-bar --retry 3 --retry-delay 5 \
          --connect-timeout 30 --max-time 1800 \
          -o "${PART_FILE}" \
          "$PART_URL" && [ -s "${PART_FILE}" ]; then
        PSIZE=$(du -h "${PART_FILE}" | cut -f1)
        echo -e "  ${GREEN}✓${NC} part-${SUFFIX}: ${PSIZE}"
        PARTS_DOWNLOADED=$((PARTS_DOWNLOADED + 1))
      else
        rm -f "${PART_FILE}"
        break  # No more parts
      fi
      echo ""
    done

    if [ "$PARTS_DOWNLOADED" -gt 0 ]; then
      echo -e "  Concatenating ${PARTS_DOWNLOADED} parts..."
      cat "${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-"* > "$SNAPSHOT_FILE"
      rm -f "${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-"*
      DOWNLOAD_OK=true
    fi

    # Method 2: try single file URL (legacy snapshots < 2 GB)
    if [ "$DOWNLOAD_OK" = false ]; then
      echo -e "  No split parts found, trying single file..."
      echo -e "  URL: ${SNAPSHOT_URL}"
      echo ""
      if curl -fL --progress-bar --retry 3 --retry-delay 5 \
          --connect-timeout 30 --max-time 3600 \
          -o "${SNAPSHOT_FILE}.tmp" \
          "$SNAPSHOT_URL" && [ -s "${SNAPSHOT_FILE}.tmp" ]; then
        TMPSIZE=$(stat -c%s "${SNAPSHOT_FILE}.tmp" 2>/dev/null || stat -f%z "${SNAPSHOT_FILE}.tmp" 2>/dev/null || echo "0")
        if [ "$TMPSIZE" -gt 1048576 ]; then
          mv "${SNAPSHOT_FILE}.tmp" "$SNAPSHOT_FILE"
          DOWNLOAD_OK=true
        else
          echo -e "  ${YELLOW}⚠${NC} Downloaded file too small (${TMPSIZE} bytes) — not a valid snapshot"
          rm -f "${SNAPSHOT_FILE}.tmp"
        fi
      else
        rm -f "${SNAPSHOT_FILE}.tmp"
      fi
    fi

    # Method 3: gh CLI fallback (for private repos where curl gets 404)
    if [ "$DOWNLOAD_OK" = false ] && command -v gh &>/dev/null && gh auth status &>/dev/null 2>&1; then
      echo -e "  Trying ${BOLD}gh release download${NC} (private repo fallback)..."
      echo -e "  ${YELLOW}Note: gh CLI may not show progress for large files${NC}"
      echo ""
      if gh release download "$RELEASE_TAG" \
           --repo "$GITHUB_REPO" \
           --pattern "${SNAPSHOT_ASSET}.part-*" \
           --dir "$SNAPSHOT_DIR" \
           --clobber 2>&1; then
        PART_FILES=$(ls -1 "${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-"* 2>/dev/null | sort)
        if [ -n "$PART_FILES" ]; then
          PART_COUNT=$(echo "$PART_FILES" | wc -l | tr -d ' ')
          echo -e "  Downloaded ${PART_COUNT} parts, concatenating..."
          cat ${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-* > "$SNAPSHOT_FILE"
          rm -f ${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.part-*
          DOWNLOAD_OK=true
        fi
      fi

      if [ "$DOWNLOAD_OK" = false ]; then
        gh release download "$RELEASE_TAG" \
             --repo "$GITHUB_REPO" \
             --pattern "$SNAPSHOT_ASSET" \
             --dir "$SNAPSHOT_DIR" \
             --clobber 2>/dev/null || true
        if [ -f "${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}" ]; then
          mv "${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}" "$SNAPSHOT_FILE"
          DOWNLOAD_OK=true
        fi
      fi
    fi

    if [ "$DOWNLOAD_OK" = false ]; then
      echo -e "${RED}ERROR: Failed to download snapshot.${NC}"
      echo ""
      echo "  Possible causes:"
      echo "  - Private repo: install gh CLI and run 'gh auth login' first"
      echo "    https://cli.github.com/"
      echo "  - Network issue: check your connection and retry"
      echo "  - Custom URL: use --snapshot-url=<URL> with a valid download link"
      exit 1
    fi

    echo ""
    echo -e "  ${GREEN}✓${NC} Downloaded $(du -h "$SNAPSHOT_FILE" | cut -f1)"

    # Try to verify checksum
    CHECKSUM_FILE="${SNAPSHOT_DIR}/${SNAPSHOT_ASSET}.sha256"
    if [ -f "$CHECKSUM_FILE" ]; then
      echo -e "${CYAN}=== Verifying checksum ===${NC}"
      EXPECTED_SHA=$(awk '{print $1}' "$CHECKSUM_FILE")
      ACTUAL_SHA=$(sha256sum "$SNAPSHOT_FILE" | awk '{print $1}')
      if [ "$EXPECTED_SHA" = "$ACTUAL_SHA" ]; then
        echo -e "  ${GREEN}✓${NC} Checksum verified"
      else
        echo -e "${RED}ERROR: Checksum mismatch — snapshot may be corrupted${NC}"
        echo "  Expected: ${EXPECTED_SHA}"
        echo "  Got:      ${ACTUAL_SHA}"
        echo "  Re-run with --force-refresh to re-download"
        rm -f "$SNAPSHOT_FILE"
        exit 1
      fi
      rm -f "$CHECKSUM_FILE"
    else
      echo -e "  ${YELLOW}⚠${NC} No checksum file available, skipping verification"
    fi
  else
    echo -e "${CYAN}=== Snapshot archive found, skipping download ===${NC}"
  fi

  # Extract
  echo ""
  echo -e "${CYAN}=== Extracting snapshot ===${NC}"

  # Clean previous extraction
  rm -f "${SNAPSHOT_DIR}/manifest.json" \
        "${SNAPSHOT_DIR}/postgres.pgdump" \
        "${SNAPSHOT_DIR}/neo4j.dump"
  rm -rf "${SNAPSHOT_DIR}/qdrant"

  # Use pipe instead of -I flag for cross-platform compat (BSD tar on macOS
  # doesn't support -I; GNU tar does but the pipe works everywhere)
  zstd -d -c "$SNAPSHOT_FILE" | tar -xf - -C "$SNAPSHOT_DIR"
  echo -e "  ${GREEN}✓${NC} Extracted"

  # Read manifest
  if [ -f "${SNAPSHOT_DIR}/manifest.json" ] && command -v jq &>/dev/null; then
    SNAP_VERSION=$(jq -r '.version // "unknown"' "${SNAPSHOT_DIR}/manifest.json")
    SNAP_SOURCE=$(jq -r '.source.name // "unknown"' "${SNAPSHOT_DIR}/manifest.json")
    SNAP_COMMIT=$(jq -r '.source.commit // "unknown"' "${SNAPSHOT_DIR}/manifest.json")
    SNAP_ITEMS=$(jq -r '.stats.total_items // "?"' "${SNAPSHOT_DIR}/manifest.json")
    SNAP_MODEL=$(jq -r '.embedding.model // "unknown"' "${SNAPSHOT_DIR}/manifest.json")
    echo -e "  Snapshot v${SNAP_VERSION}: ${SNAP_SOURCE}@${SNAP_COMMIT:0:7}"
    echo -e "  ${SNAP_ITEMS} items, model: ${SNAP_MODEL}"
  fi

else
  RESTORED_VERSION=$(cat "$SNAPSHOT_MARKER" 2>/dev/null || echo "unknown")
  echo -e "${CYAN}=== Snapshot already restored (${RESTORED_VERSION}) ===${NC}"
  echo "  Use --force-refresh to re-download and restore"
fi

echo ""

# ═══════════════════════════════════════════════════════════════════════════════
# STEP 2: Start Databases
# ═══════════════════════════════════════════════════════════════════════════════

if [ "$NEED_RESTORE" = true ]; then
  echo -e "${CYAN}=== Stopping existing services ===${NC}"
  docker compose down --remove-orphans 2>/dev/null || true
  echo -e "  ${GREEN}✓${NC} Services stopped"

  echo ""
  echo -e "${CYAN}=== Removing old database volumes ===${NC}"
  docker volume rm -f rustbrain_postgres_data rustbrain_neo4j_data rustbrain_qdrant_data 2>/dev/null || true
  echo -e "  ${GREEN}✓${NC} Clean volumes"
fi

echo ""
echo -e "${CYAN}=== Starting databases + Ollama ===${NC}"
# Ollama is required for semantic search (query embedding).
# CPU-only is fine — single query embedding takes ~200ms on CPU.
docker compose up -d postgres neo4j qdrant ollama

# Wait for PostgreSQL
echo -n "  Postgres "
for i in $(seq 1 60); do
  if docker compose exec -T postgres pg_isready -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" &>/dev/null; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo -e "${RED}TIMEOUT${NC}"
    echo -e "${RED}ERROR: PostgreSQL not ready after 60s. Check: docker compose logs postgres${NC}"
    exit 1
  fi
  echo -n "."
  sleep 1
done

# Wait for Neo4j
echo -n "  Neo4j    "
for i in $(seq 1 90); do
  if docker compose exec -T neo4j cypher-shell -u neo4j -p "${NEO4J_PASSWORD}" \
     "RETURN 1" &>/dev/null 2>&1; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 90 ]; then
    echo -e "${RED}TIMEOUT${NC}"
    echo -e "${RED}ERROR: Neo4j not ready after 90s. Check: docker compose logs neo4j${NC}"
    exit 1
  fi
  echo -n "."
  sleep 1
done

# Wait for Qdrant
QDRANT_PORT="${QDRANT_REST_PORT:-6333}"
echo -n "  Qdrant   "
for i in $(seq 1 30); do
  if curl -sf "http://localhost:${QDRANT_PORT}/healthz" &>/dev/null; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 30 ]; then
    echo -e "${RED}TIMEOUT${NC}"
    echo -e "${RED}ERROR: Qdrant not ready after 30s. Check: docker compose logs qdrant${NC}"
    exit 1
  fi
  echo -n "."
  sleep 1
done

# Wait for Ollama
OLLAMA_PORT_VAL="${OLLAMA_PORT:-11434}"
echo -n "  Ollama   "
for i in $(seq 1 60); do
  if curl -sf "http://localhost:${OLLAMA_PORT_VAL}/api/tags" &>/dev/null; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo -e "${YELLOW}TIMEOUT (non-fatal — search will be unavailable)${NC}"
    break
  fi
  echo -n "."
  sleep 1
done

# Pull the embedding model if needed (reads from manifest or .env)
EMBED_MODEL="${EMBEDDING_MODEL:-nomic-embed-text}"
if [ -f "${SNAPSHOT_DIR}/manifest.json" ] && command -v jq &>/dev/null; then
  MANIFEST_MODEL=$(jq -r '.embedding.model // empty' "${SNAPSHOT_DIR}/manifest.json" 2>/dev/null)
  if [ -n "$MANIFEST_MODEL" ]; then
    EMBED_MODEL="$MANIFEST_MODEL"
  fi
fi

if curl -sf "http://localhost:${OLLAMA_PORT_VAL}/api/tags" &>/dev/null; then
  # Check if model is already pulled
  HAVE_MODEL=$(curl -sf "http://localhost:${OLLAMA_PORT_VAL}/api/tags" 2>/dev/null \
    | jq -r ".models[]?.name // empty" 2>/dev/null | grep -c "^${EMBED_MODEL}" || true)
  if [ "$HAVE_MODEL" -eq 0 ]; then
    echo ""
    echo -e "${CYAN}=== Pulling embedding model: ${EMBED_MODEL} ===${NC}"
    echo -e "  ${YELLOW}This is a one-time download (may take a few minutes)${NC}"
    curl -sf -X POST "http://localhost:${OLLAMA_PORT_VAL}/api/pull" \
      -d "{\"name\": \"${EMBED_MODEL}\"}" \
      --no-buffer 2>/dev/null | while read -r line; do
        STATUS=$(echo "$line" | jq -r '.status // empty' 2>/dev/null)
        if [ -n "$STATUS" ] && [ "$STATUS" != "null" ]; then
          printf "\r  %s" "$STATUS"
        fi
      done
    echo ""
    echo -e "  ${GREEN}✓${NC} Model ${EMBED_MODEL} ready"
  else
    echo -e "  ${GREEN}✓${NC} Embedding model ${EMBED_MODEL} already available"
  fi
fi

# ═══════════════════════════════════════════════════════════════════════════════
# STEP 3: Restore Data (only if needed)
# ═══════════════════════════════════════════════════════════════════════════════

if [ "$NEED_RESTORE" = true ]; then
  echo ""
  echo -e "${CYAN}=== Restoring PostgreSQL ===${NC}"

  if [ -f "${SNAPSHOT_DIR}/postgres.pgdump" ]; then
    # Resolve container name (handles hash-prefixed names)
    PG_CONTAINER=$(docker ps --format "{{.Names}}" | grep "rustbrain-postgres" | head -1)
    PG_CONTAINER="${PG_CONTAINER:-rustbrain-postgres}"

    docker cp "${SNAPSHOT_DIR}/postgres.pgdump" "${PG_CONTAINER}:/tmp/postgres.pgdump"
    docker compose exec -T postgres pg_restore \
      -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
      --no-owner --no-privileges --clean --if-exists \
      /tmp/postgres.pgdump 2>/dev/null || true
    docker compose exec -T postgres rm -f /tmp/postgres.pgdump

    # Restore source_files from lite CSV (expanded_source excluded from pgdump)
    if [ -f "${SNAPSHOT_DIR}/source_files_lite.csv" ]; then
      docker cp "${SNAPSHOT_DIR}/source_files_lite.csv" "${PG_CONTAINER}:/tmp/source_files_lite.csv"
      docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -c "COPY source_files(id, crate_name, module_path, file_path, original_source,
            expanded_source, git_hash, content_hash, git_blame, last_indexed_at,
            created_at, updated_at, repository_id)
            FROM '/tmp/source_files_lite.csv' CSV HEADER" 2>/dev/null || true
      docker compose exec -T postgres rm -f /tmp/source_files_lite.csv
      SF_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
        -tAc "SELECT count(*) FROM source_files" 2>/dev/null | tr -d '[:space:]' || echo "?")
      echo -e "  ${GREEN}✓${NC} source_files restored (${SF_COUNT} files, expanded_source=NULL)"
    fi

    ITEM_COUNT=$(docker compose exec -T postgres psql -U "${POSTGRES_USER}" -d "${POSTGRES_DB}" \
      -tAc "SELECT count(*) FROM extracted_items" 2>/dev/null | tr -d '[:space:]' || echo "?")
    echo -e "  ${GREEN}✓${NC} PostgreSQL restored (${ITEM_COUNT} items)"
  else
    echo -e "  ${YELLOW}⚠${NC} No postgres.pgdump found in snapshot"
  fi

  echo ""
  echo -e "${CYAN}=== Restoring Neo4j ===${NC}"

  if [ -f "${SNAPSHOT_DIR}/neo4j.dump" ]; then
    # Cold restore: stop neo4j, load dump, restart
    docker compose stop neo4j

    # Resolve the actual Neo4j volume name (handles different project name prefixes)
    NEO4J_VOL=$(docker volume ls --format '{{.Name}}' | grep neo4j_data | head -1)
    if [ -z "$NEO4J_VOL" ]; then
      NEO4J_VOL="rustbrain_neo4j_data"
      echo -e "  ${YELLOW}⚠${NC} Could not find neo4j volume, using default: ${NEO4J_VOL}"
    fi
    echo -e "  Loading dump into volume: ${NEO4J_VOL}"

    # neo4j-admin database load (try --overwrite-destination first, fall back to --overwrite)
    if ! docker run --rm \
      -v "${NEO4J_VOL}:/data" \
      -v "${SNAPSHOT_DIR}:/snapshot:ro" \
      neo4j:5-community \
      neo4j-admin database load neo4j --from-path=/snapshot/ --overwrite-destination 2>&1; then

      echo -e "  ${YELLOW}⚠${NC} Retrying with --overwrite flag..."
      docker run --rm \
        -v "${NEO4J_VOL}:/data" \
        -v "${SNAPSHOT_DIR}:/snapshot:ro" \
        neo4j:5-community \
        neo4j-admin database load neo4j --from-path=/snapshot/ --overwrite 2>&1 || \
        echo -e "  ${RED}✗ Neo4j restore failed${NC}"
    fi

    docker compose up -d neo4j

    # Wait for Neo4j to restart
    echo -n "  Restarting "
    for i in $(seq 1 90); do
      if docker compose exec -T neo4j cypher-shell -u neo4j -p "${NEO4J_PASSWORD}" \
         "RETURN 1" &>/dev/null 2>&1; then
        echo -e " ${GREEN}✓${NC}"
        break
      fi
      if [ "$i" -eq 90 ]; then
        echo -e " ${RED}TIMEOUT${NC}"
        echo -e "${RED}ERROR: Neo4j did not restart after restore${NC}"
        exit 1
      fi
      echo -n "."
      sleep 1
    done

    # Re-run init script to ensure constraints exist (idempotent)
    docker compose exec -T neo4j cypher-shell -u neo4j -p "${NEO4J_PASSWORD}" \
      -f /var/lib/neo4j/import/init.cypher 2>/dev/null || true

    echo -e "  ${GREEN}✓${NC} Neo4j restored"
  else
    echo -e "  ${YELLOW}⚠${NC} No neo4j.dump found in snapshot"
  fi

  echo ""
  echo -e "${CYAN}=== Restoring Qdrant ===${NC}"

  QDRANT_URL="http://localhost:${QDRANT_PORT}"

  for collection in code_embeddings doc_embeddings; do
    SNAP_PATH="${SNAPSHOT_DIR}/qdrant/${collection}.snapshot"
    if [ -f "$SNAP_PATH" ]; then
      echo -n "  ${collection} "
      # Delete existing collection (if leftover from previous restore)
      curl -sf -X DELETE "${QDRANT_URL}/collections/${collection}" &>/dev/null || true
      sleep 1

      # Upload snapshot — this recreates the collection from the snapshot config
      HTTP_CODE=$(curl -sf -o /dev/null -w "%{http_code}" -X POST \
        "${QDRANT_URL}/collections/${collection}/snapshots/upload?priority=snapshot" \
        -H "Content-Type: multipart/form-data" \
        -F "snapshot=@${SNAP_PATH}" 2>/dev/null || echo "000")

      if [ "$HTTP_CODE" = "200" ] || [ "$HTTP_CODE" = "201" ]; then
        echo -e "${GREEN}✓${NC}"
      else
        echo -e "${RED}FAILED (HTTP ${HTTP_CODE})${NC}"
        echo -e "  ${YELLOW}Falling back to init-qdrant.sh for ${collection}${NC}"
      fi
    else
      echo -e "  ${YELLOW}⚠${NC} ${collection}.snapshot not found, skipping"
    fi
  done

  # Write restoration marker
  MARKER_VERSION="${SNAP_VERSION:-1.0.0}"
  echo "$MARKER_VERSION" > "$SNAPSHOT_MARKER"

  # Write snapshot metadata for the API to read
  if [ -f "${SNAPSHOT_DIR}/manifest.json" ]; then
    cp "${SNAPSHOT_DIR}/manifest.json" "${PROJECT_ROOT}/.snapshot-manifest.json"
  fi

  echo ""
  echo -e "  ${GREEN}✓${NC} All databases restored"
fi

# ═══════════════════════════════════════════════════════════════════════════════
# STEP 4: Build and Start API + MCP
# ═══════════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${CYAN}=== Building API + MCP images ===${NC}"
docker compose build api mcp-sse 2>/dev/null || {
  echo -e "${YELLOW}  Building from source (first time takes 2-5 minutes)...${NC}"
  docker compose build api mcp-sse
}
echo -e "  ${GREEN}✓${NC} Images ready"

echo ""
echo -e "${CYAN}=== Starting API + MCP-SSE ===${NC}"
docker compose up -d api mcp-sse

# Wait for API health
echo -n "  API      "
API_PORT_VAL="${API_PORT:-8088}"
for i in $(seq 1 60); do
  if curl -sf "http://localhost:${API_PORT_VAL}/health" &>/dev/null; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo -e "${RED}TIMEOUT${NC}"
    echo -e "${RED}ERROR: API not healthy after 60s. Check: docker compose logs api${NC}"
    exit 1
  fi
  echo -n "."
  sleep 2
done

# Wait for MCP-SSE health
MCP_PORT_VAL="${MCP_SSE_PORT:-3001}"
echo -n "  MCP-SSE  "
for i in $(seq 1 30); do
  if curl -sf "http://localhost:${MCP_PORT_VAL}/health" &>/dev/null; then
    echo -e "${GREEN}✓${NC}"
    break
  fi
  if [ "$i" -eq 30 ]; then
    echo -e "${YELLOW}TIMEOUT (non-fatal)${NC}"
    break
  fi
  echo -n "."
  sleep 2
done

# ═══════════════════════════════════════════════════════════════════════════════
# DONE
# ═══════════════════════════════════════════════════════════════════════════════

echo ""
echo -e "${BOLD}╔══════════════════════════════════════════════════════════════╗${NC}"
echo -e "${BOLD}║                                                              ║${NC}"
echo -e "${BOLD}║           ${GREEN}RUST-BRAIN — READY${NC}${BOLD}                                ║${NC}"
echo -e "${BOLD}║                                                              ║${NC}"
echo -e "${BOLD}╠══════════════════════════════════════════════════════════════╣${NC}"
echo -e "${BOLD}║                                                              ║${NC}"
printf "${BOLD}║${NC}  Playground UI:  ${CYAN}http://localhost:%-24s${NC}${BOLD}║${NC}\n" "${API_PORT_VAL}/"
printf "${BOLD}║${NC}  MCP-SSE:        ${CYAN}http://localhost:%-24s${NC}${BOLD}║${NC}\n" "${MCP_PORT_VAL}/sse"
printf "${BOLD}║${NC}  API Health:     ${CYAN}http://localhost:%-24s${NC}${BOLD}║${NC}\n" "${API_PORT_VAL}/health"
printf "${BOLD}║${NC}  Neo4j Browser:  ${CYAN}http://localhost:%-24s${NC}${BOLD}║${NC}\n" "${NEO4J_HTTP_PORT:-7474}"
echo -e "${BOLD}║                                                              ║${NC}"
echo -e "${BOLD}╠══════════════════════════════════════════════════════════════╣${NC}"
echo -e "${BOLD}║                                                              ║${NC}"
echo -e "${BOLD}║${NC}  Connect your IDE — add to ~/.claude.json:               ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}                                                            ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}{${NC}                                                      ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}  \"mcpServers\": {${NC}                                     ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}    \"rust-brain\": {${NC}                                   ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}      \"type\": \"sse\",${NC}                                  ${BOLD}║${NC}"
printf "${BOLD}║${NC}  ${CYAN}      \"url\": \"http://localhost:%s/sse\"${NC}               ${BOLD}║${NC}\n" "${MCP_PORT_VAL}"
echo -e "${BOLD}║${NC}  ${CYAN}    }${NC}                                                  ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}  }${NC}                                                    ${BOLD}║${NC}"
echo -e "${BOLD}║${NC}  ${CYAN}}${NC}                                                      ${BOLD}║${NC}"
echo -e "${BOLD}║                                                              ║${NC}"
echo -e "${BOLD}╚══════════════════════════════════════════════════════════════╝${NC}"
echo ""
