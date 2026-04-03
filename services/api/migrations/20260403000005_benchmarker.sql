-- Migration: benchmarker tables (eval_cases, bench_runs, bench_case_results)
-- Supports the Phase 4 Benchmarking suite: registry, run tracking, and per-case results.

-- ---------------------------------------------------------------------------
-- eval_cases: catalogue of PRs used as ground truth for benchmarking
-- ---------------------------------------------------------------------------
CREATE TABLE eval_cases (
    id                TEXT        PRIMARY KEY,
    repo              TEXT        NOT NULL,
    pr_number         INTEGER     NOT NULL,
    expected_outcome  TEXT        NOT NULL
        CHECK (expected_outcome IN ('pass', 'reject')),
    weight            FLOAT       NOT NULL DEFAULT 1.0,
    tags              TEXT[]      NOT NULL DEFAULT '{}',
    suite_name        TEXT        NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ON eval_cases(suite_name);

-- ---------------------------------------------------------------------------
-- bench_runs: one row per suite execution; tracks aggregate progress and cost
-- ---------------------------------------------------------------------------
CREATE TABLE bench_runs (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    suite_name       TEXT        NOT NULL,
    release_tag      TEXT,
    status           TEXT        NOT NULL DEFAULT 'running'
        CHECK (status IN ('running', 'completed', 'failed')),
    total_cases      INTEGER     NOT NULL,
    completed_cases  INTEGER     NOT NULL DEFAULT 0,
    pass_count       INTEGER     NOT NULL DEFAULT 0,
    pass_rate        FLOAT,
    mean_composite   FLOAT,
    total_cost_usd   NUMERIC(10, 4),
    started_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at     TIMESTAMPTZ
);

CREATE INDEX ON bench_runs(suite_name, started_at DESC);

-- ---------------------------------------------------------------------------
-- bench_case_results: per-case per-run result, linked to validator_runs
-- ---------------------------------------------------------------------------
CREATE TABLE bench_case_results (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    bench_run_id     UUID        NOT NULL REFERENCES bench_runs(id)    ON DELETE CASCADE,
    eval_case_id     TEXT        NOT NULL REFERENCES eval_cases(id),
    validator_run_id UUID                 REFERENCES validator_runs(id),
    run_index        SMALLINT    NOT NULL,
    composite        FLOAT       NOT NULL,
    pass             BOOLEAN     NOT NULL,
    cost_usd         NUMERIC(10, 6),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ON bench_case_results(bench_run_id);
CREATE INDEX ON bench_case_results(eval_case_id);
