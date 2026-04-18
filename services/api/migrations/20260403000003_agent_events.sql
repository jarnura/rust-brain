-- Migration: agent_events table
CREATE TABLE IF NOT EXISTS agent_events (
    id BIGSERIAL PRIMARY KEY,
    execution_id UUID NOT NULL REFERENCES executions(id) ON DELETE CASCADE,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    event_type TEXT NOT NULL
        CHECK (event_type IN ('reasoning', 'tool_call', 'file_edit', 'error', 'phase_change', 'agent_dispatch', 'container_kept_alive')),
    content JSONB NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_events_execution_id ON agent_events(execution_id);
