-- Migration: Add per-execution monotonic seq column to agent_events
--
-- Phase 1D (RUSA-251): Monotonic seq numbering per execution for cursor-based
-- event storage and SSE backfill. The global BIGSERIAL `id` is not suitable as
-- a per-execution cursor because it is shared across all executions and can have
-- gaps. The `seq` column provides a dense, per-execution monotonically
-- increasing sequence starting at 1.
--
-- FR-3:  Merge stdout+stderr into single per-run sequence, assign seq monotonically
-- FR-17: Persist every event in seq order, keyed by run id, replayable
-- FR-18: Support streaming reads from offset/cursor (not just full reads)
-- FR-19: Append-only per run; no in-place mutation

-- Add seq column (nullable first for safe migration; will be NOT NULL after backfill)
ALTER TABLE agent_events ADD COLUMN seq BIGINT;

-- Backfill existing rows: assign seq per execution_id based on id ordering
-- Uses a window function to compute dense sequential numbering per execution.
UPDATE agent_events ae
SET seq = sub.new_seq
FROM (
    SELECT id, ROW_NUMBER() OVER (PARTITION BY execution_id ORDER BY id ASC) AS new_seq
    FROM agent_events
) sub
WHERE ae.id = sub.id;

-- Make seq NOT NULL after backfill
ALTER TABLE agent_events ALTER COLUMN seq SET NOT NULL;

-- Composite index: primary access pattern is "events for execution X after seq Y"
CREATE INDEX idx_agent_events_exec_seq ON agent_events(execution_id, seq);

-- Unique constraint: prevents duplicate seq values within an execution (safety net)
CREATE UNIQUE INDEX uq_agent_events_execution_seq ON agent_events(execution_id, seq);
