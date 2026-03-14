#!/bin/bash
# =============================================================================
# rust-brain — Stop Script
# =============================================================================
set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== Stopping rust-brain infrastructure ==="
docker-compose down
echo ""
echo "To remove all data volumes, run:"
echo "  docker-compose down -v"
