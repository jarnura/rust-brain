CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY,
    parent_id   TEXT,
    phase       TEXT NOT NULL
                CHECK (phase IN ('understand','plan','build','verify','communicate')),
    class       TEXT NOT NULL CHECK (class IN ('A','B','C','D','E')),
    agent       TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending'
                CHECK (status IN ('pending','dispatched','in_progress','review',
                                  'completed','rejected','blocked','escalated')),
    inputs      JSONB DEFAULT '[]',
    constraints JSONB DEFAULT '{}',
    acceptance  TEXT,
    retry_count INT DEFAULT 0,
    error       TEXT,
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    updated_at  TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_tasks_agent ON tasks(agent);
CREATE INDEX IF NOT EXISTS idx_tasks_class ON tasks(class);
CREATE INDEX IF NOT EXISTS idx_tasks_phase ON tasks(phase);
