-- Create workspace_audit_log table for cross-workspace audit logging (RUSA-197)
-- Tracks workspace lifecycle events: create, clone, index, archive, volume ops, cleanup failures
-- Complements the existing audit_events table (which tracks ingestion pipeline events only)

CREATE TABLE IF NOT EXISTS workspace_audit_log (
    id            BIGSERIAL PRIMARY KEY,
    workspace_id  UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    operation     TEXT NOT NULL,
    actor         TEXT NOT NULL DEFAULT 'system',
    old_status    TEXT,
    new_status    TEXT,
    resource_ids  JSONB DEFAULT '{}',
    detail        JSONB DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Fast lookup by workspace (most common query: "show me all events for workspace X")
CREATE INDEX IF NOT EXISTS idx_workspace_audit_log_workspace
    ON workspace_audit_log(workspace_id);

-- Filter by operation type (e.g., "show me all cleanup_failed events")
CREATE INDEX IF NOT EXISTS idx_workspace_audit_log_operation
    ON workspace_audit_log(operation);

-- Time-range queries for retention policy and forensics
CREATE INDEX IF NOT EXISTS idx_workspace_audit_log_created_at
    ON workspace_audit_log(created_at);

-- Composite: workspace + time (dashboard: "recent events for workspace X")
CREATE INDEX IF NOT EXISTS idx_workspace_audit_log_workspace_time
    ON workspace_audit_log(workspace_id, created_at DESC);

-- Valid operation values (documented here for reference; not enforced via CHECK
-- because the Rust audit service may add new operations without a migration):
--   create          - workspace record created
--   clone_start     - git clone initiated
--   clone_done      - git clone completed
--   clone_failed    - git clone failed
--   index_start     - ingestion started
--   index_done      - ingestion completed
--   index_failed    - ingestion failed
--   volume_create   - Docker volume created
--   volume_remove   - Docker volume removed
--   volume_create_failed - Docker volume creation failed
--   volume_remove_failed - Docker volume removal failed (potential leak)
--   container_spawn - execution container started
--   container_stop  - execution container stopped
--   container_remove_failed - container removal failed (potential leak)
--   archive         - workspace archived
--   cleanup_failed  - workspace cleanup failed (potential leak)
--   leak_detected   - leak detector found orphaned resource
--   leak_cleaned    - leak detector removed orphaned resource
