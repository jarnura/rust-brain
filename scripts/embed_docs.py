#!/usr/bin/env python3
"""Embed project documentation into Qdrant doc_embeddings collection.

Reads markdown files from the project, chunks them by heading sections,
generates embeddings via Ollama (qwen3-embedding:4b), and stores in Qdrant.

Usage:
    python3 scripts/embed_docs.py [--dry-run] [--verbose]
"""

import hashlib
import json
import os
import re
import sys
import time
import uuid
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError, URLError

# --- Configuration ---
QDRANT_URL = os.environ.get("QDRANT_HOST", "http://localhost:6333")
OLLAMA_URL = os.environ.get("OLLAMA_HOST", "http://localhost:11434")
EMBEDDING_MODEL = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:4b")
COLLECTION = "doc_embeddings"
VECTOR_SIZE = 2560
MAX_CHUNK_CHARS = 3000
MIN_CHUNK_CHARS = 200  # Merge sections smaller than this
BATCH_SIZE = 4  # Ollama batch size (smaller for CPU inference)
CRATE_NAME = "rust_brain"

# UUID v5 namespace for deterministic IDs (same as ingestion service)
NAMESPACE = uuid.UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")

# Project root (relative to this script)
PROJECT_ROOT = Path(__file__).resolve().parent.parent

# Files to embed (relative to project root)
DOC_GLOBS = [
    "README.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "CHANGELOG.md",
    "KNOWN_ISSUES.md",
    "RELEASE_CHECKLIST.md",
    "configs/README.md",
    "tests/README.md",
    "docs/*.md",
    "docs/adr/*.md",
    "docs/issues/*.md",
]

# Files to skip (ephemeral, not documentation)
SKIP_PATTERNS = [
    "tasks",
    "logging-audit",
    "RUSA-",
    "PROJECT_STATE",
    "COVERAGE_REPORT",
    "CLASS_A_TESTING",
    "E2E_TEST_SUITE",
    "INGESTION_PERFORMANCE",
    "agent-config-corrections",
]

# Directories to skip entirely
SKIP_DIRS = ["docs/agent-prompts", "docs/prompts"]


def should_skip(filepath: str) -> bool:
    name = Path(filepath).stem
    if any(pat in name for pat in SKIP_PATTERNS):
        return True
    return any(filepath.startswith(d) for d in SKIP_DIRS)


def collect_files() -> list[Path]:
    """Collect markdown files matching DOC_GLOBS."""
    import glob as globmod

    files = []
    for pattern in DOC_GLOBS:
        for match in globmod.glob(str(PROJECT_ROOT / pattern)):
            p = Path(match)
            rel = str(p.relative_to(PROJECT_ROOT))
            if not should_skip(rel) and p.is_file():
                files.append(p)
    return sorted(set(files))


def chunk_markdown(text: str, filepath: str) -> list[dict]:
    """Split markdown into chunks by h1/h2 heading boundaries.

    h3+ headings stay within their parent section to produce fewer, larger chunks.
    Small sections are merged with the next section.
    Max chunk size: MAX_CHUNK_CHARS.
    """
    lines = text.split("\n")
    raw_sections: list[dict] = []
    current_h1 = ""
    current_h2 = ""
    current_lines: list[str] = []
    current_heading = Path(filepath).stem

    def flush():
        nonlocal current_lines
        content = "\n".join(current_lines).strip()
        if content:
            raw_sections.append({
                "heading": current_heading,
                "h1": current_h1,
                "h2": current_h2,
                "content": content,
            })
        current_lines = []

    for line in lines:
        h1_match = re.match(r"^#\s+(.+)", line)
        h2_match = re.match(r"^##\s+(.+)", line)

        if h1_match:
            flush()
            current_h1 = h1_match.group(1).strip()
            current_h2 = ""
            current_heading = current_h1
            current_lines.append(line)
        elif h2_match:
            flush()
            current_h2 = h2_match.group(1).strip()
            current_heading = current_h2
            current_lines.append(line)
        else:
            current_lines.append(line)

    flush()

    # Merge small sections into their neighbor
    merged: list[dict] = []
    for section in raw_sections:
        if merged and len(section["content"]) < MIN_CHUNK_CHARS:
            merged[-1]["content"] += "\n\n" + section["content"]
            merged[-1]["heading"] += " / " + section["heading"]
        else:
            merged.append(dict(section))

    # Split oversized sections and build final chunks
    chunks: list[dict] = []
    for section in merged:
        prefix_parts = [f"File: {filepath}"]
        if section["h1"]:
            prefix_parts.append(section["h1"])
        if section["h2"]:
            prefix_parts.append(section["h2"])
        prefix = " > ".join(prefix_parts)
        content = section["content"]

        if len(content) > MAX_CHUNK_CHARS:
            paragraphs = re.split(r"\n\n+", content)
            buffer = ""
            for para in paragraphs:
                if buffer and len(buffer) + len(para) + 2 > MAX_CHUNK_CHARS:
                    chunks.append({
                        "heading": section["heading"],
                        "text": f"{prefix}\n\n{buffer.strip()}",
                    })
                    buffer = para
                else:
                    buffer = f"{buffer}\n\n{para}" if buffer else para
            if buffer.strip():
                chunks.append({
                    "heading": section["heading"],
                    "text": f"{prefix}\n\n{buffer.strip()}",
                })
        else:
            chunks.append({
                "heading": section["heading"],
                "text": f"{prefix}\n\n{content}",
            })

    # Assign chunk indices
    for i, chunk in enumerate(chunks):
        chunk["chunk_index"] = i

    return chunks


def generate_point_id(filepath: str, chunk_index: int) -> str:
    """Deterministic UUID v5 for idempotent upserts."""
    key = f"{filepath}:doc:{chunk_index}"
    return str(uuid.uuid5(NAMESPACE, key))


def embed_texts(texts: list[str]) -> list[list[float]]:
    """Call Ollama /api/embed for batch embedding."""
    payload = json.dumps({
        "model": EMBEDDING_MODEL,
        "input": texts,
    }).encode()

    req = Request(
        f"{OLLAMA_URL}/api/embed",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    for attempt in range(5):
        try:
            with urlopen(req, timeout=120) as resp:
                data = json.loads(resp.read())
                embeddings = data.get("embeddings", [])
                if not embeddings:
                    raise ValueError(f"No embeddings returned: {data}")
                return embeddings
        except (HTTPError, URLError) as e:
            if attempt < 4:
                wait = 2 ** attempt
                print(f"  Ollama error (attempt {attempt+1}): {e}. Retrying in {wait}s...")
                time.sleep(wait)
            else:
                raise


def upsert_points(points: list[dict]):
    """Batch upsert points to Qdrant."""
    payload = json.dumps({"points": points}).encode()
    req = Request(
        f"{QDRANT_URL}/collections/{COLLECTION}/points",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="PUT",
    )
    with urlopen(req, timeout=30) as resp:
        data = json.loads(resp.read())
        if data.get("status") != "ok":
            raise ValueError(f"Qdrant upsert failed: {data}")


def search_docs(query: str, limit: int = 5) -> list[dict]:
    """Search doc_embeddings for verification."""
    # Get query embedding
    embeddings = embed_texts([query])
    vector = embeddings[0]

    payload = json.dumps({
        "vector": vector,
        "limit": limit,
        "with_payload": True,
        "score_threshold": 0.3,
    }).encode()

    req = Request(
        f"{QDRANT_URL}/collections/{COLLECTION}/points/search",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urlopen(req, timeout=30) as resp:
        data = json.loads(resp.read())
        return data.get("result", [])


def main():
    dry_run = "--dry-run" in sys.argv
    verbose = "--verbose" in sys.argv

    print(f"=== Doc Embedding Script ===")
    print(f"Qdrant:  {QDRANT_URL}")
    print(f"Ollama:  {OLLAMA_URL}")
    print(f"Model:   {EMBEDDING_MODEL}")
    print(f"Collection: {COLLECTION}")
    if dry_run:
        print("MODE: DRY RUN (no writes)")
    print()

    # Collect files
    files = collect_files()
    print(f"Found {len(files)} documentation files:")
    for f in files:
        print(f"  {f.relative_to(PROJECT_ROOT)}")
    print()

    # Chunk all files
    all_chunks = []
    for filepath in files:
        rel_path = str(filepath.relative_to(PROJECT_ROOT))
        text = filepath.read_text(encoding="utf-8", errors="replace")
        chunks = chunk_markdown(text, rel_path)
        all_chunks.extend([(rel_path, chunk) for chunk in chunks])
        if verbose:
            print(f"  {rel_path}: {len(chunks)} chunks")

    print(f"Total chunks: {len(all_chunks)}")
    print()

    if dry_run:
        for rel_path, chunk in all_chunks:
            print(f"  [{chunk['chunk_index']}] {rel_path} > {chunk['heading'][:60]} ({len(chunk['text'])} chars)")
        return

    # Process in batches
    total_embedded = 0
    for batch_start in range(0, len(all_chunks), BATCH_SIZE):
        batch = all_chunks[batch_start : batch_start + BATCH_SIZE]
        texts = [c[1]["text"] for c in batch]

        print(f"Embedding batch {batch_start // BATCH_SIZE + 1}/{(len(all_chunks) + BATCH_SIZE - 1) // BATCH_SIZE} ({len(batch)} chunks)...")

        embeddings = embed_texts(texts)

        # Build Qdrant points
        points = []
        for (rel_path, chunk), vector in zip(batch, embeddings):
            point_id = generate_point_id(rel_path, chunk["chunk_index"])
            # Build FQN-like identifier from file path
            source_fqn = rel_path.replace("/", "::").replace(".md", "").replace("-", "_")

            points.append({
                "id": point_id,
                "vector": vector,
                "payload": {
                    "source_fqn": f"{CRATE_NAME}::{source_fqn}",
                    "file_path": rel_path,
                    "section_title": chunk["heading"],
                    "content_type": "documentation",
                    "crate_name": CRATE_NAME,
                    "chunk_index": chunk["chunk_index"],
                    "text": chunk["text"],
                },
            })

        upsert_points(points)
        total_embedded += len(points)

        if verbose:
            for (rel_path, chunk), _ in zip(batch, embeddings):
                print(f"    {rel_path}:{chunk['chunk_index']} - {chunk['heading'][:50]}")

    print(f"\nEmbedded {total_embedded} doc chunks into {COLLECTION}")

    # Verification searches
    print("\n=== Verification Searches ===\n")
    test_queries = [
        "triple storage architecture postgres neo4j qdrant",
        "how to set up and run the ingestion pipeline",
        "MCP server configuration for Claude",
        "API endpoints for semantic search",
        "why did we choose local Ollama for embeddings",
    ]
    for query in test_queries:
        print(f'Q: "{query}"')
        results = search_docs(query, limit=3)
        for r in results:
            score = r["score"]
            payload = r["payload"]
            fp = payload.get("file_path", "?")
            section = payload.get("section_title", "?")
            print(f"  [{score:.3f}] {fp} > {section}")
        print()

    print("Done.")


if __name__ == "__main__":
    main()
