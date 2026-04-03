#!/usr/bin/env python3
"""Populate crate_docs Qdrant collection from extracted_items doc comments.

Unlike doc_embeddings (which splits docs into paragraph chunks), this collection
stores one vector per documented symbol — full doc comment indexed per symbol,
searchable by the Research agent for API documentation queries.

Requires: Docker stack running (Postgres, Qdrant, Ollama)
Usage: python3 scripts/populate-crate-docs.py
"""

import json
import os
import sys
import uuid

import psycopg2
import requests

QDRANT_URL = os.environ.get("QDRANT_URL", "http://localhost:6333")
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://localhost:11434")
DATABASE_URL = os.environ.get(
    "DATABASE_URL", "postgres://postgres:postgres@localhost:5432/rustbrain"
)
COLLECTION = "crate_docs"
MODEL = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:4b")
DIMENSIONS = int(os.environ.get("EMBEDDING_DIMENSIONS", "2560"))
BATCH_SIZE = int(os.environ.get("BATCH_SIZE", "32"))
UPSERT_BATCH_SIZE = 50

# Same namespace UUID as the ingestion pipeline (UUID namespace DNS)
NAMESPACE = uuid.UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")


def check_services():
    """Verify Qdrant and Ollama are reachable."""
    try:
        resp = requests.get(f"{QDRANT_URL}/healthz", timeout=5)
        resp.raise_for_status()
    except Exception as e:
        print(f"ERROR: Qdrant not reachable at {QDRANT_URL}: {e}", file=sys.stderr)
        sys.exit(1)

    try:
        resp = requests.get(f"{OLLAMA_URL}/api/tags", timeout=5)
        resp.raise_for_status()
    except Exception as e:
        print(f"ERROR: Ollama not reachable at {OLLAMA_URL}: {e}", file=sys.stderr)
        sys.exit(1)


def create_collection():
    """Create Qdrant collection if it does not exist."""
    resp = requests.get(f"{QDRANT_URL}/collections/{COLLECTION}", timeout=10)
    if resp.status_code == 200:
        info = resp.json()["result"]
        print(
            f"Collection '{COLLECTION}' exists "
            f"({info['points_count']} points, status={info['status']})"
        )
        return

    print(f"Creating collection '{COLLECTION}' ({DIMENSIONS}-dim, Cosine)...")
    resp = requests.put(
        f"{QDRANT_URL}/collections/{COLLECTION}",
        json={"vectors": {"size": DIMENSIONS, "distance": "Cosine"}},
        timeout=10,
    )
    resp.raise_for_status()
    print(f"Created collection '{COLLECTION}'")


def get_documented_items():
    """Query Postgres for all pub items with doc comments."""
    conn = psycopg2.connect(DATABASE_URL)
    try:
        cur = conn.cursor()
        cur.execute("""
            SELECT ei.fqn, ei.name, ei.item_type, ei.visibility, ei.doc_comment,
                   sf.file_path, sf.crate_name, sf.module_path
            FROM extracted_items ei
            JOIN source_files sf ON ei.source_file_id = sf.id
            WHERE ei.doc_comment IS NOT NULL
              AND ei.doc_comment != ''
              AND ei.visibility IN ('pub', 'pub_crate')
            ORDER BY ei.fqn
        """)
        rows = cur.fetchall()
    finally:
        conn.close()
    return rows


def embed_batch(texts: list[str]) -> list[list[float]]:
    """Generate embeddings for a batch of texts via Ollama /api/embed."""
    resp = requests.post(
        f"{OLLAMA_URL}/api/embed",
        json={"model": MODEL, "input": texts},
        timeout=120,
    )
    resp.raise_for_status()
    return resp.json()["embeddings"]


def deterministic_uuid(fqn: str) -> str:
    """Generate deterministic UUID v5 from FQN.

    Uses "crate_docs:" prefix to avoid collisions with code_embeddings UUIDs
    (which use the raw FQN).
    """
    return str(uuid.uuid5(NAMESPACE, f"crate_docs:{fqn}"))


def upsert_batch(points: list[dict]):
    """Upsert a batch of points to Qdrant."""
    resp = requests.put(
        f"{QDRANT_URL}/collections/{COLLECTION}/points?wait=true",
        json={"points": points},
        timeout=30,
    )
    resp.raise_for_status()


def main():
    print(f"=== Populating {COLLECTION} collection ===")
    print(f"Qdrant: {QDRANT_URL}  Ollama: {OLLAMA_URL}  Model: {MODEL}")

    check_services()
    create_collection()

    items = get_documented_items()
    total = len(items)
    print(f"Found {total} documented pub items in Postgres")

    if total == 0:
        print("Nothing to embed. Ensure the ingestion pipeline has run first.")
        return

    # Process in embedding batches
    batch_texts = []
    batch_items = []
    upsert_buffer = []
    embedded_count = 0
    skip_count = 0

    for i, (fqn, name, item_type, visibility, doc_comment,
            file_path, crate_name, module_path) in enumerate(items):

        # Build text representation for embedding
        text = f"{item_type} {name}\n{doc_comment}"
        batch_texts.append(text)
        batch_items.append((fqn, name, item_type, visibility, doc_comment,
                            file_path, crate_name, module_path))

        if len(batch_texts) >= BATCH_SIZE or i == total - 1:
            try:
                vectors = embed_batch(batch_texts)
            except Exception as e:
                print(f"  WARN: embedding batch failed ({len(batch_texts)} items): {e}")
                skip_count += len(batch_texts)
                batch_texts = []
                batch_items = []
                continue

            for vec, item_data in zip(vectors, batch_items):
                (b_fqn, b_name, b_item_type, b_visibility, b_doc_comment,
                 b_file_path, b_crate_name, b_module_path) = item_data

                point = {
                    "id": deterministic_uuid(b_fqn),
                    "vector": vec,
                    "payload": {
                        "fqn": b_fqn,
                        "name": b_name,
                        "item_type": b_item_type,
                        "crate_name": b_crate_name or "",
                        "module_path": b_module_path or "",
                        "visibility": b_visibility,
                        "doc_comment": b_doc_comment[:2000],
                        "file_path": b_file_path or "",
                        "has_examples": (
                            "```" in b_doc_comment
                            or "# Examples" in b_doc_comment
                            or "# Example" in b_doc_comment
                        ),
                    },
                }
                upsert_buffer.append(point)
                embedded_count += 1

            # Flush upsert buffer when full
            if len(upsert_buffer) >= UPSERT_BATCH_SIZE:
                upsert_batch(upsert_buffer)
                upsert_buffer = []

            batch_texts = []
            batch_items = []

            # Progress
            done = i + 1
            if done % 200 == 0 or done == total:
                print(f"  Progress: {done}/{total} ({embedded_count} embedded, {skip_count} skipped)")

    # Flush remaining points
    if upsert_buffer:
        upsert_batch(upsert_buffer)

    # Verify
    info = requests.get(f"{QDRANT_URL}/collections/{COLLECTION}", timeout=10).json()
    count = info["result"]["points_count"]
    print(f"\n=== Done: {count} points in '{COLLECTION}' ===")
    print(f"Embedded: {embedded_count}  Skipped: {skip_count}")


if __name__ == "__main__":
    main()
