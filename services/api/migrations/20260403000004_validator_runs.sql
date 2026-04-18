-- Migration: validator_runs
-- Stores per-run evaluation results from the rustbrain-validator pipeline.
-- One row per RunResult within a ValidationResult (indexed by repo + pr_number + run_index).

CREATE TABLE validator_runs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    repo TEXT NOT NULL,
    pr_number INTEGER NOT NULL,
    run_index SMALLINT NOT NULL,
    composite_score FLOAT NOT NULL,
    pass BOOLEAN NOT NULL,
    inverted BOOLEAN NOT NULL DEFAULT FALSE,
    dimension_scores JSONB NOT NULL,
    tokens_used INTEGER,
    cost_usd NUMERIC(10,6),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ON validator_runs(repo, pr_number);
