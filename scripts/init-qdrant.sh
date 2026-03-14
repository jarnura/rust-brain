#!/bin/bash
# =============================================================================
# rust-brain — Initialize Qdrant Collections
# =============================================================================
set -euo pipefail

QDRANT_HOST="${QDRANT_HOST:-http://localhost:6333}"
EMBEDDING_DIMENSIONS="${EMBEDDING_DIMENSIONS:-768}"

echo "=== Initializing Qdrant Collections ==="
echo "Qdrant Host: $QDRANT_HOST"
echo "Embedding Dimensions: $EMBEDDING_DIMENSIONS"

# Wait for Qdrant to be healthy
echo "Waiting for Qdrant to be healthy..."
for i in {1..30}; do
    if curl -sf "${QDRANT_HOST}/healthz" > /dev/null 2>&1; then
        echo "Qdrant is healthy!"
        break
    fi
    if [ $i -eq 30 ]; then
        echo "ERROR: Qdrant not healthy after 30 seconds"
        exit 1
    fi
    sleep 1
done

# Create code_embeddings collection
echo ""
echo "=== Creating code_embeddings collection ==="
curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings" \
    -H "Content-Type: application/json" \
    -d "{
        \"vectors\": {
            \"size\": ${EMBEDDING_DIMENSIONS},
            \"distance\": \"Cosine\"
        },
        \"optimizers_config\": {
            \"indexing_threshold\": 20000
        }
    }" | jq '.' || echo "Collection may already exist"

# Create payload indexes for code_embeddings
echo ""
echo "=== Creating payload indexes for code_embeddings ==="
curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "fqn", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "crate_name", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "module_path", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "item_type", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "visibility", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "file_path", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/code_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "has_generics", "field_schema": "bool"}' | jq '.' || true

# Create doc_embeddings collection
echo ""
echo "=== Creating doc_embeddings collection ==="
curl -sf -X PUT "${QDRANT_HOST}/collections/doc_embeddings" \
    -H "Content-Type: application/json" \
    -d "{
        \"vectors\": {
            \"size\": ${EMBEDDING_DIMENSIONS},
            \"distance\": \"Cosine\"
        }
    }" | jq '.' || echo "Collection may already exist"

# Create payload indexes for doc_embeddings
echo ""
echo "=== Creating payload indexes for doc_embeddings ==="
curl -sf -X PUT "${QDRANT_HOST}/collections/doc_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "source_fqn", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/doc_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "content_type", "field_schema": "keyword"}' | jq '.' || true

curl -sf -X PUT "${QDRANT_HOST}/collections/doc_embeddings/index" \
    -H "Content-Type: application/json" \
    -d '{"field_name": "crate_name", "field_schema": "keyword"}' | jq '.' || true

# Verify collections
echo ""
echo "=== Verifying collections ==="
curl -sf "${QDRANT_HOST}/collections" | jq '.result.collections[] | {name, vectors_count, points_count}'

echo ""
echo "=== Qdrant initialization complete ==="
