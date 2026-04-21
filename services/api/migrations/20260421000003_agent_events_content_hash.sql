-- Migration: Add content_hash column for write-side deduplication (RUSA-266 2B-1)
--
-- Computes SHA-256 of (execution_id, event_type, content) before insert.
-- The UNIQUE constraint on (execution_id, content_hash) prevents duplicate
-- events from being inserted (e.g., on runner retry / double-bridge).
-- INSERT ... ON CONFLICT DO NOTHING returns nothing, so the fallback SELECT
-- retrieves the existing row.

-- Require pgcrypto extension for digest() — must be before any digest() call
CREATE EXTENSION IF NOT EXISTS pgcrypto;

-- Add content_hash column
ALTER TABLE agent_events ADD COLUMN content_hash BYTEA;

-- Create unique index for dedup constraint
CREATE UNIQUE INDEX uq_agent_events_execution_content_hash
    ON agent_events(execution_id, content_hash);

-- Backfill existing rows with computed hash
UPDATE agent_events
SET content_hash = digest(
    execution_id::text || event_type || content::text,
    'sha256'
)
WHERE content_hash IS NULL;

-- Make NOT NULL after backfill
ALTER TABLE agent_events ALTER COLUMN content_hash SET NOT NULL;
