-- Migration: add runtime info fields to executions table
-- These fields support the runtime info panel in the playground UI.
ALTER TABLE executions ADD COLUMN IF NOT EXISTS volume_name TEXT;
ALTER TABLE executions ADD COLUMN IF NOT EXISTS opencode_endpoint TEXT;
ALTER TABLE executions ADD COLUMN IF NOT EXISTS workspace_path TEXT;
