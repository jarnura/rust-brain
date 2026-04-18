ALTER TABLE executions ADD COLUMN container_expires_at TIMESTAMPTZ NULL;
CREATE INDEX idx_executions_expires_at ON executions (container_expires_at)
  WHERE container_expires_at IS NOT NULL AND container_id IS NOT NULL;
