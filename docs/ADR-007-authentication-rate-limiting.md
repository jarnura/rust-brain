# ADR-007: Authentication & Rate Limiting

**Status:** Accepted  
**Date:** 2026-04-21  
**Deciders:** CTO  
**Relates to:** Gap 11 (GAP_ANALYSIS.md), RUSA-274

## Context

The API currently has no authentication. All 49 routes are publicly accessible. Rate limiting exists only on 3 embedding-search endpoints via `tower_governor` (10 req/s per IP). The MCP bridge connects as a trusted internal service with no credentials.

For any deployment beyond localhost, we need:
- API key validation to prevent unauthorized access
- Per-key rate limits to prevent abuse
- Request size enforcement (already partially done with 1 MiB body limit)
- Cypher query sanitization (currently claims read-only but needs hardening)

## Decision

### Authentication Layer

**API Key model** with tiered access:

```sql
CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    key_hash TEXT NOT NULL UNIQUE,  -- SHA-256 of the key, never store plaintext
    name TEXT NOT NULL,
    tier TEXT NOT NULL CHECK (tier IN ('admin', 'standard', 'readonly')),
    workspace_id TEXT,  -- NULL = all workspaces, set = scoped to one workspace
    rate_limit_per_minute INTEGER NOT NULL DEFAULT 60,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ,  -- NULL = never expires
    last_used_at TIMESTAMPTZ,
    is_active BOOLEAN DEFAULT true
);
```

**Key format:** `rb_live_<32-char-random-hex>` (prefix enables grep/rotation tooling).

**Tiers:**
| Tier | Access | Default Rate Limit |
|------|--------|-------------------|
| `admin` | All endpoints + write operations + ingestion triggers | 120/min |
| `standard` | All read endpoints + chat | 60/min |
| `readonly` | Code intelligence endpoints only (no chat, no ingestion) | 30/min |

### Middleware Architecture

```
Request → Auth Middleware → Rate Limit Middleware → Route Handler
```

**Auth middleware** (`src/middleware/auth.rs`):
1. Extract `Authorization: Bearer <key>` header
2. SHA-256 hash the key, lookup in `api_keys` table
3. Reject expired or inactive keys with 401
4. Attach `ApiKeyContext { tier, workspace_id, rate_limit }` to request extensions
5. Update `last_used_at` asynchronously (don't block the request)

**Bypass rules:**
- `GET /health` and `GET /metrics` — always public (for monitoring)
- Internal service calls (MCP → API) — use a dedicated `internal` tier key injected via env var
- Local development — configurable `RUSTBRAIN_AUTH_DISABLED=true` to skip auth entirely

### Rate Limiting Enhancement

Replace the current per-IP governor with **per-key rate limiting**:

```rust
// Keyed by API key hash, not IP
let governor = GovernorConfigBuilder::default()
    .key_extractor(ApiKeyExtractor)
    .per_second(key_context.rate_limit_per_minute / 60)
    .burst_size(key_context.rate_limit_per_minute / 6)  // 10s burst
    .finish()
    .unwrap();
```

Rate limit headers in responses:
- `X-RateLimit-Limit`: max requests per minute
- `X-RateLimit-Remaining`: remaining in current window
- `X-RateLimit-Reset`: Unix timestamp when window resets

### Cypher Query Hardening

The `POST /tools/query_graph` endpoint currently does regex-based write detection. Harden with:

1. **Allowlist approach**: Only permit queries starting with `MATCH`, `OPTIONAL MATCH`, `CALL { MATCH`, `WITH`, `UNWIND`
2. **Blocklist**: Reject any query containing `CREATE`, `MERGE`, `DELETE`, `DETACH`, `SET`, `REMOVE`, `DROP`, `CALL apoc.`
3. **Read-only Neo4j user**: Connect the API with a Neo4j user that only has READ privileges (defense in depth)

### MCP Bridge Auth

The MCP service gets a dedicated internal API key (`rb_internal_<hex>`) configured via `RUSTBRAIN_INTERNAL_API_KEY` env var. This key:
- Has `admin` tier access
- Is not rate-limited (or very high limit: 1000/min)
- Is workspace-scoped per session (MCP passes `X-Workspace-Id` header)

### Key Management Endpoints

```
POST   /api/keys          — Create new API key (admin only, returns key once)
GET    /api/keys          — List keys (admin only, shows metadata not keys)
DELETE /api/keys/{id}     — Revoke key (admin only)
PATCH  /api/keys/{id}     — Update tier/rate_limit/expiry (admin only)
```

Bootstrap: First key is created via `RUSTBRAIN_BOOTSTRAP_KEY` env var at startup. If set, the system creates an admin key with that value on first boot.

## Consequences

**Positive:**
- Enables safe deployment beyond localhost
- Per-key rate limits prevent a single consumer from saturating the API
- Workspace scoping enables multi-tenant key isolation
- Cypher hardening closes the injection vector
- Internal key for MCP maintains zero-config local development

**Negative:**
- Adds latency (~1-2ms) for key lookup per request (mitigated by in-memory cache)
- Key management is another surface to secure
- Breaking change for existing users (mitigated by `RUSTBRAIN_AUTH_DISABLED` flag)

**Mitigations:**
- Cache active keys in-memory with 60s TTL (invalidate on revocation)
- Provide clear migration guide in release notes
- `AUTH_DISABLED` mode for local dev matches current behavior exactly

## Alternatives Considered

1. **JWT tokens**: More complex, requires token issuance flow. Overkill for machine-to-machine API access.
2. **OAuth2**: Enterprise-grade but massive implementation effort. Not justified at current scale.
3. **mTLS**: Strong security but operational complexity for key distribution. Could layer on later.
4. **IP allowlisting**: Too rigid for cloud deployments, doesn't provide identity.
