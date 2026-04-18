# Ingestion Performance Baseline

This document captures baseline performance metrics for the rust-brain ingestion pipeline.

## Test Environment

| Component | Configuration |
|-----------|---------------|
| Host | Docker on Linux |
| Memory Budget | 32GB (ingestion container) |
| Database Containers | Postgres 6GB, Neo4j 12GB, Qdrant 12GB |
| Embedding Model | Ollama qwen3-embedding:4b (GPU) |

## Baseline Metrics (April 2026)

### Ingestion Run: 2026-04-08

| Stage | Duration | Items Processed | Rate |
|-------|----------|-----------------|------|
| expand | 95.4s | 2 crates | — |
| parse | 0.43s | 2,689 | **6,253 items/sec** |
| typecheck | 0.15s | 2 | 13 items/sec |
| extract | 1.27s | 0 | — |
| graph | 2.21s | 11,248 | **5,089 items/sec** |
| embed | 58.95s | 2,285 | **39 items/sec** |

**Total Duration**: ~2.5 minutes (partial run)

### Stage Breakdown

1. **Expand** — `cargo expand` macro expansion
   - Bottleneck: Rust compilation time
   - ~48s per crate on average
   - Failure rate: 40% (2 of 5 crates failed)

2. **Parse** — tree-sitter + syn parsing
   - Fast: 6,000+ items/sec
   - Minimal overhead

3. **Typecheck** — rust-analyzer type inference
   - Currently limited: only processes 2 items
   - Needs investigation for full crate coverage

4. **Extract** — item extraction
   - Fast: ~1s
   - Low item count suggests extraction happens in earlier stages

5. **Graph** — Neo4j node/edge creation
   - Fast: 5,000+ items/sec
   - Batch insertion working well

6. **Embed** — Ollama embeddings
   - **Primary bottleneck**: 39 items/sec
   - GPU-accelerated but still slow
   - 2,285 items in 59 seconds

## Memory Usage (Idle State)

| Container | Memory Limit | Current Usage | % |
|-----------|--------------|---------------|---|
| postgres | 6GB | 44MB | 0.7% |
| neo4j | 12GB | 1.3GB | 11% |
| qdrant | 12GB | 665MB | 5.4% |
| ollama | 16GB | 3.4GB | 21% |
| api | 2GB | 46MB | 2.2% |
| mcp-sse | 256MB | 37MB | 14% |

## Identified Bottlenecks

1. **Embedding Stage (Primary)**
   - 39 items/sec vs 5,000+ for other stages
   - 100x slower than graph insertion
   - Suggestion: Increase batch size, parallel embedding requests

2. **Expand Stage (Secondary)**
   - 95s for 2 crates
   - High failure rate (40%)
   - Suggestion: Cache expanded code, skip crates that already failed

3. **Typecheck Stage (Needs Investigation)**
   - Only processing 2 items
   - May be skipping most items due to configuration or errors

## Recommendations

1. **Embedding Optimization**
   - Increase batch size from current to 100+ items
   - Consider parallel embedding requests
   - Monitor Ollama GPU utilization during embedding

2. **Expand Caching**
   - Cache expanded code in Postgres
   - Skip re-expansion for unchanged crates
   - Add retry logic with exponential backoff

3. **Monitoring Enhancements**
   - Add per-stage items/sec to Grafana dashboard
   - Add container memory peak tracking
   - Add embedding batch size metric

## Grafana Dashboard

Dashboard: `Ingestion Pipeline` (http://localhost:3000/d/rustbrain-pipeline)

Panels:
- Ingestion Runs / min
- Items Extracted / min
- Errors / min
- Ingestion Latency (p50, p95)
- Phase Duration

## Monitoring Commands

```bash
# Real-time monitoring during ingestion
watch -n 5 ./scripts/monitor-ingestion.sh

# Container resource usage
docker stats --no-stream

# Recent ingestion runs
docker exec 3edea4c00618_rustbrain-postgres psql -U rustbrain -d rustbrain -c \
  "SELECT id, started_at, status, metadata->'stages' as stages FROM ingestion_runs ORDER BY started_at DESC LIMIT 5;"
```

## Future Baselines

As ingestion scales to larger crates (e.g., Hyperswitch with 500K+ items), update this document with:

1. Large-scale items/sec rates
2. Memory peak during full ingestion
3. Time-to-completion for full crate ingestion
4. Bottleneck analysis at scale
