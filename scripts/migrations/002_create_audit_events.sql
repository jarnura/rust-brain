-- Audit trail for ingestion pipeline events
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
