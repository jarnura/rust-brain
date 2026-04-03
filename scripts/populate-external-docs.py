#!/usr/bin/env python3
"""Populate external_docs Qdrant collection from docs.rs pages.

Fetches docs.rs HTML for key Hyperswitch dependencies, chunks by section,
embeds via Ollama, and upserts to Qdrant. Used by the Research agent for
API lookups on external crates.

Requires: Docker stack running (Qdrant, Ollama) + internet access
Usage: python3 scripts/populate-external-docs.py
"""

import os
import re
import sys
import uuid
from html.parser import HTMLParser

import requests

QDRANT_URL = os.environ.get("QDRANT_URL", "http://localhost:6333")
OLLAMA_URL = os.environ.get("OLLAMA_URL", "http://localhost:11434")
COLLECTION = "external_docs"
MODEL = os.environ.get("EMBEDDING_MODEL", "qwen3-embedding:4b")
DIMENSIONS = int(os.environ.get("EMBEDDING_DIMENSIONS", "2560"))
BATCH_SIZE = int(os.environ.get("BATCH_SIZE", "32"))
MAX_CHUNK_SIZE = 500  # chars per chunk

# Same namespace UUID as the ingestion pipeline (UUID namespace DNS)
NAMESPACE = uuid.UUID("6ba7b810-9dad-11d1-80b4-00c04fd430c8")

# Key dependencies to index
# Format: (crate_name, version_prefix, important_items)
KEY_DEPS = [
    ("axum", "0.7", ["Router", "Json", "State", "extract", "Extension", "middleware"]),
    ("sqlx", "0.8", ["query", "query_as", "PgPool", "Row", "FromRow", "Pool"]),
    ("neo4rs", "0.7", ["Graph", "query", "Node", "Relation", "BoltType"]),
    ("tokio", "1", ["spawn", "select", "sync", "time", "task", "net"]),
    ("serde", "1.0", ["Serialize", "Deserialize"]),
    ("serde_json", "1.0", ["Value", "json", "from_str", "to_string"]),
    ("reqwest", "0.12", ["Client", "Response", "RequestBuilder"]),
    ("anyhow", "1.0", ["Result", "Context", "bail", "Error"]),
    ("tracing", "0.1", ["info", "debug", "warn", "error", "instrument", "span"]),
    ("uuid", "1", ["Uuid"]),
    ("qdrant_client", "1", ["Qdrant", "PointStruct", "SearchPoints"]),
]


class HTMLTextExtractor(HTMLParser):
    """Extract visible text from HTML, preserving section structure."""

    def __init__(self):
        super().__init__()
        self.sections: list[tuple[str, str]] = []  # (heading, text)
        self._current_heading = ""
        self._current_text = []
        self._in_heading = False
        self._skip_tags = {"script", "style", "nav", "footer", "head"}
        self._skip_depth = 0

    def handle_starttag(self, tag, attrs):
        if tag in self._skip_tags:
            self._skip_depth += 1
        if tag in ("h1", "h2", "h3", "h4"):
            self._flush_section()
            self._in_heading = True

    def handle_endtag(self, tag):
        if tag in self._skip_tags and self._skip_depth > 0:
            self._skip_depth -= 1
        if tag in ("h1", "h2", "h3", "h4"):
            self._in_heading = False

    def handle_data(self, data):
        if self._skip_depth > 0:
            return
        text = data.strip()
        if not text:
            return
        if self._in_heading:
            self._current_heading = text
        else:
            self._current_text.append(text)

    def _flush_section(self):
        if self._current_text:
            body = " ".join(self._current_text)
            self.sections.append((self._current_heading, body))
            self._current_text = []

    def get_sections(self) -> list[tuple[str, str]]:
        self._flush_section()
        return self.sections


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


def fetch_docs_page(crate_name: str, version: str, path: str) -> str:
    """Fetch a docs.rs page and return raw HTML."""
    url = f"https://docs.rs/{crate_name}/{version}/{path}"
    try:
        resp = requests.get(url, timeout=15, headers={
            "User-Agent": "rust-brain/1.0 (docs indexer)"
        })
        if resp.status_code == 200:
            return resp.text
    except requests.RequestException:
        pass
    return ""


def extract_sections(html: str) -> list[tuple[str, str]]:
    """Parse HTML and extract (heading, text) sections."""
    parser = HTMLTextExtractor()
    parser.feed(html)
    return parser.get_sections()


def chunk_text(text: str, max_size: int = MAX_CHUNK_SIZE) -> list[str]:
    """Split text into chunks at sentence boundaries."""
    sentences = re.split(r"(?<=[.!?])\s+", text)
    chunks = []
    current = ""
    for sentence in sentences:
        if len(current) + len(sentence) > max_size and current:
            chunks.append(current.strip())
            current = sentence
        else:
            current = f"{current} {sentence}" if current else sentence
    if current.strip():
        chunks.append(current.strip())
    return [c for c in chunks if len(c) > 30]  # drop tiny fragments


def embed_batch(texts: list[str]) -> list[list[float]]:
    """Generate embeddings for a batch of texts via Ollama /api/embed."""
    resp = requests.post(
        f"{OLLAMA_URL}/api/embed",
        json={"model": MODEL, "input": texts},
        timeout=120,
    )
    resp.raise_for_status()
    return resp.json()["embeddings"]


def deterministic_uuid(key: str) -> str:
    """Generate deterministic UUID v5 from key."""
    return str(uuid.uuid5(NAMESPACE, f"external_docs:{key}"))


def upsert_batch(points: list[dict]):
    """Upsert a batch of points to Qdrant."""
    resp = requests.put(
        f"{QDRANT_URL}/collections/{COLLECTION}/points?wait=true",
        json={"points": points},
        timeout=30,
    )
    resp.raise_for_status()


def resolve_docs_paths(crate_name: str, version: str, item: str) -> list[tuple[str, str]]:
    """Try multiple docs.rs URL patterns to find the item page.

    Returns list of (page_path, html) for pages that resolved.
    """
    # docs.rs uses the crate name with hyphens replaced by underscores in paths
    crate_path = crate_name.replace("-", "_")

    candidates = [
        # Module/re-export page
        f"{crate_path}/{item}/index.html",
        # Struct, fn, trait, enum, macro pages
        f"{crate_path}/struct.{item}.html",
        f"{crate_path}/fn.{item}.html",
        f"{crate_path}/trait.{item}.html",
        f"{crate_path}/enum.{item}.html",
        f"{crate_path}/macro.{item}.html",
        f"{crate_path}/type.{item}.html",
        f"{crate_path}/constant.{item}.html",
    ]

    results = []
    for path in candidates:
        html = fetch_docs_page(crate_name, version, path)
        if html:
            results.append((path, html))
            break  # Use first match
    return results


def process_dependency(crate_name: str, version: str, items: list[str]) -> list[dict]:
    """Fetch, chunk, and prepare points for one dependency."""
    points = []

    for item in items:
        pages = resolve_docs_paths(crate_name, version, item)
        if not pages:
            print(f"  SKIP {crate_name}::{item} -- page not found")
            continue

        for page_path, html in pages:
            sections = extract_sections(html)
            url = f"https://docs.rs/{crate_name}/{version}/{page_path}"

            for section_heading, section_text in sections:
                chunks = chunk_text(section_text)
                for idx, chunk in enumerate(chunks):
                    point_key = f"{crate_name}:{item}:{section_heading}:{idx}"
                    points.append({
                        "key": point_key,
                        "text": chunk,
                        "payload": {
                            "crate_name": crate_name,
                            "version": version,
                            "item": item,
                            "section": section_heading,
                            "page_path": page_path,
                            "text": chunk[:1000],
                            "url": url,
                            "chunk_index": idx,
                        },
                    })

        if any(p["payload"]["item"] == item for p in points):
            chunk_count = sum(
                1 for p in points if p["payload"]["item"] == item
            )
            print(f"  {crate_name}::{item}: {chunk_count} chunks")

    return points


def main():
    print(f"=== Populating {COLLECTION} collection ===")
    print(f"Qdrant: {QDRANT_URL}  Ollama: {OLLAMA_URL}  Model: {MODEL}")

    check_services()
    create_collection()

    # Gather all chunks from all dependencies
    all_pending: list[dict] = []

    for crate_name, version, items in KEY_DEPS:
        print(f"\nFetching {crate_name} v{version}...")
        dep_points = process_dependency(crate_name, version, items)
        all_pending.extend(dep_points)

    total = len(all_pending)
    print(f"\nTotal chunks to embed: {total}")

    if total == 0:
        print("No docs fetched. Check internet access and docs.rs availability.")
        return

    # Embed and upsert in batches
    upsert_buffer = []
    embedded_count = 0
    skip_count = 0

    for batch_start in range(0, total, BATCH_SIZE):
        batch = all_pending[batch_start : batch_start + BATCH_SIZE]
        texts = [p["text"] for p in batch]

        try:
            vectors = embed_batch(texts)
        except Exception as e:
            print(f"  WARN: embedding batch failed: {e}")
            skip_count += len(batch)
            continue

        for vec, pending in zip(vectors, batch):
            point = {
                "id": deterministic_uuid(pending["key"]),
                "vector": vec,
                "payload": pending["payload"],
            }
            upsert_buffer.append(point)
            embedded_count += 1

        # Flush when buffer is large enough
        if len(upsert_buffer) >= 50:
            upsert_batch(upsert_buffer)
            upsert_buffer = []

        done = min(batch_start + BATCH_SIZE, total)
        if done % 100 == 0 or done == total:
            print(f"  Embedded: {done}/{total}")

    # Flush remaining
    if upsert_buffer:
        upsert_batch(upsert_buffer)

    # Verify
    info = requests.get(f"{QDRANT_URL}/collections/{COLLECTION}", timeout=10).json()
    count = info["result"]["points_count"]
    print(f"\n=== Done: {count} points in '{COLLECTION}' ===")
    print(f"Embedded: {embedded_count}  Skipped: {skip_count}")


if __name__ == "__main__":
    main()
