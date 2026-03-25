-- =============================================================================
-- rust-brain — Postgres Initialization Schema
-- =============================================================================
-- Idempotent schema for code intelligence storage
-- =============================================================================

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- =============================================================================
-- SOURCE FILES: Raw and expanded Rust source
-- =============================================================================

CREATE TABLE IF NOT EXISTS source_files (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    crate_name TEXT NOT NULL,
    module_path TEXT NOT NULL,
    file_path TEXT NOT NULL,
    original_source TEXT NOT NULL,
    expanded_source TEXT,
    git_hash TEXT,
    content_hash TEXT,
    git_blame JSONB,
    last_indexed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(crate_name, module_path, file_path)
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_source_files_crate ON source_files(crate_name);
CREATE INDEX IF NOT EXISTS idx_source_files_module ON source_files(crate_name, module_path);
CREATE INDEX IF NOT EXISTS idx_source_files_path ON source_files(file_path);

-- =============================================================================
-- EXTRACTED ITEMS: Functions, structs, enums, traits, etc.
-- =============================================================================

CREATE TYPE item_type AS ENUM (
    'function', 'struct', 'enum', 'trait', 'impl', 'type_alias', 
    'const', 'static', 'macro', 'module'
);

CREATE TABLE IF NOT EXISTS extracted_items (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    source_file_id UUID REFERENCES source_files(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK (item_type IN ('function','struct','enum','trait','impl','type_alias','const','static','macro','module')),
    fqn TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    visibility TEXT NOT NULL DEFAULT 'private',
    signature TEXT,
    doc_comment TEXT,
    start_line INT NOT NULL,
    end_line INT NOT NULL,
    body_source TEXT,
    generic_params JSONB DEFAULT '[]',
    where_clauses JSONB DEFAULT '[]',
    attributes JSONB DEFAULT '[]',
    generated_by TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    updated_at TIMESTAMPTZ DEFAULT NOW()
);

-- Indexes for lookups
CREATE INDEX IF NOT EXISTS idx_extracted_items_fqn ON extracted_items(fqn);
CREATE INDEX IF NOT EXISTS idx_extracted_items_type ON extracted_items(item_type);
CREATE INDEX IF NOT EXISTS idx_extracted_items_name ON extracted_items(name);
CREATE INDEX IF NOT EXISTS idx_extracted_items_source ON extracted_items(source_file_id);
CREATE INDEX IF NOT EXISTS idx_extracted_items_crate ON extracted_items(fqn text_pattern_ops);
CREATE INDEX IF NOT EXISTS idx_extracted_items_generated_by ON extracted_items(generated_by);

-- GIN index for generic_params and attributes JSONB queries
CREATE INDEX IF NOT EXISTS idx_extracted_items_generic ON extracted_items USING GIN(generic_params);
CREATE INDEX IF NOT EXISTS idx_extracted_items_attributes ON extracted_items USING GIN(attributes);

-- =============================================================================
-- CALL SITES: Monomorphization and call graph data
-- =============================================================================

CREATE TABLE IF NOT EXISTS call_sites (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    caller_fqn TEXT NOT NULL,
    callee_fqn TEXT NOT NULL,
    file_path TEXT NOT NULL,
    line_number INT NOT NULL,
    concrete_type_args JSONB DEFAULT '[]',
    is_monomorphized BOOLEAN DEFAULT FALSE,
    quality TEXT NOT NULL DEFAULT 'heuristic' CHECK (quality IN ('analyzed', 'heuristic')),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Indexes for call graph queries
CREATE INDEX IF NOT EXISTS idx_call_sites_caller ON call_sites(caller_fqn);
CREATE INDEX IF NOT EXISTS idx_call_sites_callee ON call_sites(callee_fqn);
CREATE INDEX IF NOT EXISTS idx_call_sites_file ON call_sites(file_path);
CREATE INDEX IF NOT EXISTS idx_call_sites_types ON call_sites USING GIN(concrete_type_args);

-- =============================================================================
-- TRAIT IMPLEMENTATIONS: impl Trait for Type mappings
-- =============================================================================

CREATE TABLE IF NOT EXISTS trait_implementations (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    trait_fqn TEXT NOT NULL,
    self_type TEXT NOT NULL,
    impl_fqn TEXT UNIQUE NOT NULL,
    file_path TEXT NOT NULL,
    line_number INT NOT NULL,
    generic_params JSONB DEFAULT '[]',
    quality TEXT NOT NULL DEFAULT 'heuristic' CHECK (quality IN ('analyzed', 'heuristic')),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Indexes for trait resolution queries
CREATE INDEX IF NOT EXISTS idx_trait_impls_trait ON trait_implementations(trait_fqn);
CREATE INDEX IF NOT EXISTS idx_trait_impls_type ON trait_implementations(self_type);
CREATE INDEX IF NOT EXISTS idx_trait_impls_file ON trait_implementations(file_path);

-- =============================================================================
-- INGESTION RUNS: Track pipeline executions
-- =============================================================================

CREATE TABLE IF NOT EXISTS ingestion_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'completed', 'failed', 'partial')),
    crates_processed INT DEFAULT 0,
    items_extracted INT DEFAULT 0,
    errors JSONB DEFAULT '[]',
    metadata JSONB DEFAULT '{}'
);

-- =============================================================================
-- REPOSITORIES: Multi-repo support (future)
-- =============================================================================

CREATE TABLE IF NOT EXISTS repositories (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT UNIQUE NOT NULL,
    git_url TEXT,
    branch TEXT DEFAULT 'main',
    last_indexed_hash TEXT,
    last_indexed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Add repository_id to source_files for multi-repo support
ALTER TABLE source_files ADD COLUMN IF NOT EXISTS repository_id UUID REFERENCES repositories(id);

-- =============================================================================
-- AUDIT EVENTS: Pipeline audit trail
-- =============================================================================

CREATE TABLE IF NOT EXISTS audit_events (
    id UUID PRIMARY KEY,
    pipeline_id UUID NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    event_type TEXT NOT NULL,
    stage TEXT,
    detail JSONB,
    severity TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_audit_events_pipeline_id ON audit_events (pipeline_id);
CREATE INDEX IF NOT EXISTS idx_audit_events_timestamp ON audit_events (timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_events_event_type ON audit_events (event_type);
CREATE INDEX IF NOT EXISTS idx_audit_events_severity ON audit_events (severity);

-- =============================================================================
-- UPDATE TRIGGER: Auto-update updated_at
-- =============================================================================

CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

CREATE TRIGGER update_source_files_updated_at
    BEFORE UPDATE ON source_files
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_extracted_items_updated_at
    BEFORE UPDATE ON extracted_items
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- =============================================================================
-- GRANTS (adjust as needed for your setup)
-- =============================================================================

-- GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO rustbrain;
-- GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO rustbrain;
