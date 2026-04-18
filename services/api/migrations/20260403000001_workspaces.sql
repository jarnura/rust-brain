-- Migration: workspaces table
CREATE TABLE IF NOT EXISTS workspaces (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    source_type TEXT NOT NULL CHECK (source_type IN ('github', 'local')),
    source_url TEXT NOT NULL,
    clone_path TEXT,
    volume_name TEXT,
    schema_name TEXT UNIQUE,
    status TEXT NOT NULL DEFAULT 'cloning'
        CHECK (status IN ('cloning', 'indexing', 'ready', 'error', 'archived')),
    default_branch TEXT,
    github_auth_method TEXT CHECK (github_auth_method IN ('app', 'pat', 'none')),
    index_started_at TIMESTAMPTZ,
    index_completed_at TIMESTAMPTZ,
    index_stage TEXT,
    index_progress JSONB,
    index_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
