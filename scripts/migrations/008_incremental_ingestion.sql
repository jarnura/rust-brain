-- Migration: 008_incremental_ingestion.sql
-- Description: Schema changes to support incremental ingestion as specified in ADR-006

-- 1. Add ingestion tracking columns to source_files
-- ingestion_run_id links each file to the specific run that processed it
ALTER TABLE source_files ADD COLUMN IF NOT EXISTS ingestion_run_id UUID;

-- last_ingested_at tracks when the content was last processed into the graph
-- Separate from last_indexed_at which tracks basic metadata indexing
ALTER TABLE source_files ADD COLUMN IF NOT EXISTS last_ingested_at TIMESTAMPTZ DEFAULT NOW();

-- 2. Add detailed metrics and context to ingestion_runs
ALTER TABLE ingestion_runs 
    ADD COLUMN IF NOT EXISTS crate_name TEXT NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS workspace_id TEXT,
    ADD COLUMN IF NOT EXISTS mode TEXT NOT NULL DEFAULT 'full' CHECK (mode IN ('full', 'incremental')),
    ADD COLUMN IF NOT EXISTS files_total INTEGER DEFAULT 0,
    ADD COLUMN IF NOT EXISTS files_changed INTEGER DEFAULT 0,
    ADD COLUMN IF NOT EXISTS files_skipped INTEGER DEFAULT 0,
    ADD COLUMN IF NOT EXISTS items_upserted INTEGER DEFAULT 0,
    ADD COLUMN IF NOT EXISTS items_deleted INTEGER DEFAULT 0;

-- 3. Add call dispatch tracking column to call_sites
-- Indicates how a call site was resolved: static, trait, or dynamic
ALTER TABLE call_sites ADD COLUMN IF NOT EXISTS dispatch TEXT NOT NULL DEFAULT 'dynamic'
    CHECK (dispatch IN ('static', 'trait', 'dynamic'));

-- 4. Create indices for performance
-- Faster lookup of files by ingestion run for cleanup or auditing
CREATE INDEX IF NOT EXISTS idx_source_files_ingestion_run_id ON source_files(ingestion_run_id);

-- Optimized lookup for change detection during incremental runs
-- Used to quickly find existing content hashes for files in a crate
CREATE INDEX IF NOT EXISTS idx_source_files_crate_hash ON source_files(crate_name, content_hash);
