-- Add container_expires_at column for configurable keep-alive (RUSA-131)
ALTER TABLE executions ADD COLUMN IF NOT EXISTS container_expires_at TIMESTAMPTZ NULL;
