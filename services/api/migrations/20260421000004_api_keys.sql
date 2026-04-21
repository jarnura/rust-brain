-- API key authentication table per ADR-007
-- Stores hashed API keys for authentication and per-key rate limiting.

CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    key_hash TEXT NOT NULL UNIQUE,          -- SHA-256 of the key, never store plaintext
    name TEXT NOT NULL,                     -- Human-readable key name
    tier TEXT NOT NULL CHECK (tier IN ('admin', 'standard', 'readonly')),
    workspace_id TEXT,                      -- NULL = all workspaces, set = scoped to one workspace
    rate_limit_per_minute INTEGER NOT NULL DEFAULT 60,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ,                -- NULL = never expires
    last_used_at TIMESTAMPTZ,
    is_active BOOLEAN DEFAULT true
);

-- Index for fast key lookup by hash (every authenticated request hits this)
CREATE INDEX idx_api_keys_key_hash ON api_keys (key_hash) WHERE is_active = true;

-- Index for listing keys by name
CREATE INDEX idx_api_keys_name ON api_keys (name);
