-- Migration: Add 'unknown' to agent_events.event_type CHECK constraint
--
-- Per RECONCILIATION.md R-4 P0 fix: unknown MessagePart types must be
-- stored as opaque events rather than silently dropped. The runner now
-- inserts rows with event_type = 'unknown' for unrecognized parts.

-- Drop the existing CHECK constraint and replace with one that includes 'unknown'.
-- Postgres auto-names CHECK constraints as "<table>_<column>_check".
ALTER TABLE agent_events
    DROP CONSTRAINT IF EXISTS agent_events_event_type_check,
    ADD CONSTRAINT agent_events_event_type_check
        CHECK (event_type IN (
            'reasoning', 'tool_call', 'file_edit', 'error',
            'phase_change', 'agent_dispatch', 'container_kept_alive',
            'unknown'
        ));
