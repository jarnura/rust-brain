# ADR-002: Local AI with Ollama

## Status

**Accepted** — 2026-03-14

## Context

A code intelligence platform needs AI capabilities for:

1. **Code embeddings** — Semantic representation of functions, structs, modules
2. **Natural language queries** — "Find authentication handlers"
3. **Code understanding** — Summarizing functions, explaining logic
4. **Query translation** — Converting natural language to structured queries

We needed to decide between:

1. **Cloud AI APIs** — OpenAI, Anthropic, Cohere, etc.
2. **Local AI** — Self-hosted models via Ollama, vLLM, or similar
3. **Hybrid** — Local embeddings, cloud LLMs

## Decision

We will use **Ollama for all AI/ML workloads**, running entirely locally.

### Chosen Models

| Purpose | Model | Dimensions | Size |
|---------|-------|------------|------|
| Embeddings | `qwen3-embedding:4b` | 2560 | ~2.5 GB |
| Code understanding | `codellama:7b` | — | ~3.8 GB |

> **Update (2026-03):** The embedding model was upgraded from `nomic-embed-text` (768-dim) to `qwen3-embedding:4b` (2560-dim) for better code semantic search quality. See `.env.example` for the current default.

### Why Ollama

1. **Privacy First**
   - Source code never leaves the machine
   - No API keys to manage or rotate
   - No data sent to third parties
   - Suitable for proprietary codebases

2. **No External Dependencies**
   - Works entirely offline
   - No rate limits or quotas
   - No service outages from providers
   - No billing surprises

3. **Predictable Performance**
   - No network latency
   - Consistent response times
   - No cold starts from provider side

4. **Cost Effective**
   - No per-token charges
   - Unlimited queries
   - Only hardware costs

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      rust-brain                              │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌──────────┐     ┌──────────┐     ┌──────────────────┐    │
│  │ Ingestion│────▶│  Ollama  │────▶│ Qdrant (vectors) │    │
│  │ Pipeline │     │ :11434   │     │ :6333            │    │
│  └──────────┘     └────┬─────┘     └──────────────────┘    │
│                        │                                     │
│  ┌──────────┐          │                                     │
│  │ Tool API │◀─────────┘                                     │
│  │ :8080    │          Code understanding                    │
│  └──────────┘                                                │
│                                                              │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
              ┌────────────────────────┐
              │    Local Machine        │
              │                         │
              │  ┌─────────────────┐   │
              │  │ Ollama Container│   │
              │  │                 │   │
              │  │ • nomic-embed   │   │
              │  │ • codellama:7b  │   │
              │  │                 │   │
              │  │ Memory: 8GB     │   │
              │  │ GPU: Optional   │   │
              │  └─────────────────┘   │
              └────────────────────────┘
```

## Usage Patterns

### Embedding Generation (Ingestion)

```bash
# Generate embedding for code snippet
curl http://localhost:11434/api/embed \
  -d '{"model": "nomic-embed-text", "input": "fn main() { println!(\"hello\"); }"}'

# Response: 768-dimensional vector
```

### Code Understanding (Query Time)

```bash
# Generate response
curl http://localhost:11434/api/generate \
  -d '{
    "model": "codellama:7b",
    "prompt": "Explain what this Rust function does: fn foo<T: Clone>(x: T) -> T { x.clone() }"
  }'
```

## Consequences

### Positive

1. **Complete Privacy**
   - Source code, comments, and queries stay local
   - Suitable for proprietary and sensitive codebases
   - No data governance concerns

2. **No Recurring Costs**
   - No API token charges
   - No monthly subscriptions
   - Unlimited usage after initial setup

3. **Offline Operation**
   - Works without internet connection
   - No dependency on external service availability
   - Consistent behavior regardless of network

4. **No Rate Limits**
   - Can run massive ingestion jobs
   - No throttling during peak usage
   - Predictable throughput

5. **Model Control**
   - Can switch models easily
   - Can fine-tune models (with extra effort)
   - Version pin models for reproducibility

### Negative

1. **Hardware Requirements**

   | Requirement | Minimum | Recommended |
   |-------------|---------|-------------|
   | RAM | 16 GB | 32 GB |
   | Storage | 20 GB | 50 GB |
   | GPU | None (CPU) | NVIDIA 8GB+ VRAM |
   | CPU | 4 cores | 8+ cores |

   **Memory breakdown:**
   - Base system: ~4 GB
   - Postgres: ~2 GB
   - Neo4j: ~4 GB
   - Qdrant: ~4 GB
   - Ollama: ~8 GB
   - **Total: ~22 GB recommended**

2. **Slower Inference** (without GPU)

   | Operation | CPU (M1) | GPU (RTX 3080) | Cloud API |
   |-----------|----------|----------------|-----------|
   | Embedding | ~50ms | ~10ms | ~100ms + network |
   | Code gen (7B) | ~2s | ~200ms | ~1s + network |

3. **Model Quality Gap**
   - Local 7B models < GPT-4/Claude in quality
   - May struggle with complex reasoning
   - Limited context window (4K-8K tokens typical)

4. **Setup Complexity**
   - Need to download models (~4-5 GB)
   - First-time startup slower
   - GPU configuration optional but tricky

### Mitigations

1. **Hardware Requirements**
   - Document minimum specs clearly
   - Provide smaller model options (q4 quantization)
   - Allow disabling Ollama for CPU-only operation

2. **Inference Speed**
   - Use GPU when available (configure in docker-compose)
   - Pre-compute embeddings during ingestion
   - Cache query results

3. **Model Quality**
   - Use specialized code models (CodeLlama)
   - Can upgrade to larger models (13B, 34B) with more RAM
   - Hybrid option: local embeddings, cloud LLM for complex queries

4. **Setup Complexity**
   - `pull-models.sh` script automates downloads
   - Health checks verify model availability
   - Clear error messages when models missing

## Alternatives Considered

### Cloud AI APIs (OpenAI, Anthropic, Cohere)

**Pros:**
- Best-in-class model quality (GPT-4, Claude)
- No hardware requirements
- Simple API integration
- Larger context windows

**Cons:**
- **Privacy concern:** Code sent to third parties
- **Cost:** Per-token charges add up quickly
- **Dependency:** Requires internet, subject to outages
- **Rate limits:** Can throttle large ingestion jobs

**Verdict:** Not suitable for proprietary codebases or high-volume usage

### Hybrid Approach (Local Embeddings + Cloud LLM)

**Pros:**
- Privacy for code content (embeddings stay local)
- Better quality for complex queries (cloud LLM)
- Lower cost than full cloud

**Cons:**
- Complex architecture (two AI systems)
- Still sends queries to cloud (partial privacy loss)
- Cloud dependency for advanced features

**Verdict:** Could be future option for users wanting best of both worlds

### vLLM / Text Generation WebUI

**Pros:**
- More configuration options
- Better GPU utilization
- Support for more model formats

**Cons:**
- More complex setup
- Less polished UX than Ollama
- Smaller community

**Verdict:** Ollama's simplicity wins for standard deployment

## Model Selection Rationale

### qwen3-embedding:4b for Embeddings (current default)

| Criteria | Rating | Notes |
|----------|--------|-------|
| Code quality | ★★★★★ | Higher-dimensional vectors capture more semantic nuance |
| Size | ★★★★☆ | ~2.5 GB (GPU recommended) |
| Speed | ★★★★☆ | ~180 items/sec on RTX 4070 Ti SUPER |
| Dimensions | ★★★★★ | 2560 dims — richer representations than 768 |
| Context | ★★★★☆ | Handles code signatures + docs well |

### CodeLlama:7b for Code Understanding

| Criteria | Rating | Notes |
|----------|--------|-------|
| Code quality | ★★★★☆ | Specialized for code, understands Rust |
| Size | ★★★★☆ | ~4 GB, fits in memory |
| Speed | ★★★☆☆ | Acceptable on CPU, fast on GPU |
| Reasoning | ★★★☆☆ | Good for code tasks, limited for complex logic |
| Context | ★★★☆☆ | 4K-16K context depending on variant |

## Upgrading Models

To use different/better models:

```bash
# Edit .env
EMBEDDING_MODEL=mxbai-embed-large    # Better embeddings
CODE_MODEL=codellama:13b             # Better code understanding (needs 16GB RAM)

# Re-pull models
bash scripts/pull-models.sh
```

## References

- [Ollama Documentation](https://github.com/ollama/ollama)
- [nomic-embed-text Model](https://ollama.com/library/nomic-embed-text)
- [CodeLlama Model](https://ollama.com/library/codellama)
- [Ollama Model Library](https://ollama.com/library)
