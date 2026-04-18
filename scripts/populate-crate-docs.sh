#!/usr/bin/env bash
# Populates the crate_docs Qdrant collection from extracted_items doc comments.
# Requires: Docker stack running (Postgres, Qdrant, Ollama)
# Usage: ./scripts/populate-crate-docs.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "=== populate-crate-docs ==="

# Check Python3
if ! command -v python3 &>/dev/null; then
  echo "ERROR: python3 not found" >&2
  exit 1
fi

# Check required Python packages
python3 -c "import psycopg2, requests" 2>/dev/null || {
  echo "Installing Python dependencies..."
  pip3 install --quiet psycopg2-binary requests
}

exec python3 "$SCRIPT_DIR/populate-crate-docs.py" "$@"
