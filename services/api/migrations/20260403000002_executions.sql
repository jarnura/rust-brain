-- Migration: executions table
CREATE TABLE IF NOT EXISTS executions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    prompt TEXT NOT NULL,
    branch_name TEXT,
    session_id TEXT,
    status TEXT NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'completed', 'failed', 'aborted')),
    agent_phase TEXT CHECK (agent_phase IN ('orchestrating', 'researching', 'planning', 'developing')),
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    diff_summary JSONB,
    error TEXT,
    timeout_config_secs INTEGER NOT NULL DEFAULT 7200
);

CREATE INDEX IF NOT EXISTS idx_executions_workspace_id ON executions(workspace_id);
