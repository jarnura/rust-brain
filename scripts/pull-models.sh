#!/bin/bash
# =============================================================================
# rust-brain — Pull Ollama Models
# =============================================================================
set -euo pipefail

OLLAMA_HOST="${OLLAMA_HOST:-http://localhost:11434}"
EMBEDDING_MODEL="${EMBEDDING_MODEL:-nomic-embed-text}"
CODE_MODEL="${CODE_MODEL:-codellama:7b}"

echo "=== Pulling Ollama Models ==="
echo "Ollama Host: $OLLAMA_HOST"
echo "Embedding Model: $EMBEDDING_MODEL"
echo "Code Model: $CODE_MODEL"

# Wait for Ollama to be healthy
echo ""
echo "Waiting for Ollama to be healthy..."
for i in {1..60}; do
    if curl -sf "${OLLAMA_HOST}/api/tags" > /dev/null 2>&1; then
        echo "Ollama is healthy!"
        break
    fi
    if [ $i -eq 60 ]; then
        echo "ERROR: Ollama not healthy after 60 seconds"
        exit 1
    fi
    sleep 1
done

# Pull embedding model
echo ""
echo "=== Pulling embedding model: $EMBEDDING_MODEL ==="
curl -fsSL "${OLLAMA_HOST}/api/pull" \
    -H "Content-Type: application/json" \
    -d "{\"name\": \"${EMBEDDING_MODEL}\", \"stream\": false}" | jq '.'

# Pull code understanding model
echo ""
echo "=== Pulling code model: $CODE_MODEL ==="
curl -fsSL "${OLLAMA_HOST}/api/pull" \
    -H "Content-Type: application/json" \
    -d "{\"name\": \"${CODE_MODEL}\", \"stream\": false}" | jq '.'

# Verify models
echo ""
echo "=== Verifying models ==="
curl -fsSL "${OLLAMA_HOST}/api/tags" | jq '.models[] | {name, size}'

# Test embedding endpoint
echo ""
echo "=== Testing embedding endpoint ==="
RESPONSE=$(curl -fsSL "${OLLAMA_HOST}/api/embed" \
    -H "Content-Type: application/json" \
    -d "{\"model\": \"${EMBEDDING_MODEL}\", \"input\": \"fn main() { println!(\\\"hello\\\"); }\"}")

DIMS=$(echo "$RESPONSE" | jq '.embeddings[0] | length')
echo "Embedding dimensions: $DIMS"

if [ "$DIMS" != "768" ] && [ "$DIMS" != "1024" ]; then
    echo "WARNING: Unexpected embedding dimensions: $DIMS"
    echo "You may need to update EMBEDDING_DIMENSIONS in .env"
else
    echo "Embedding dimensions look correct!"
fi

echo ""
echo "=== Ollama models ready ==="
