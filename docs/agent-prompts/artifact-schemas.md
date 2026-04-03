# Artifact Schemas — Inter-Agent Contract Definitions

Every agent produces exactly one artifact type. Artifacts are stored in the shared
context_store (PostgreSQL JSONB) and referenced by ID. The Orchestrator sees only
the summary fields; specialist agents see the full payload.

---

## Core artifact table (PostgreSQL)

```sql
CREATE TABLE artifacts (
    id          TEXT PRIMARY KEY,          -- e.g. "art_20240315_001"
    task_id     TEXT NOT NULL REFERENCES tasks(id),
    type        TEXT NOT NULL,             -- enum: see types below
    producer    TEXT NOT NULL,             -- agent name
    status      TEXT NOT NULL DEFAULT 'draft',  -- draft | final | superseded
    confidence  FLOAT NOT NULL DEFAULT 1.0,     -- 0.0-1.0, triggers escalation < 0.7
    summary     JSONB NOT NULL,            -- compressed view for Orchestrator
    payload     JSONB NOT NULL,            -- full artifact body for consuming agents
    created_at  TIMESTAMPTZ DEFAULT NOW(),
    superseded_by TEXT REFERENCES artifacts(id)  -- when a retry produces a new version
);

CREATE INDEX idx_artifacts_task ON artifacts(task_id);
CREATE INDEX idx_artifacts_type ON artifacts(type);
CREATE INDEX idx_artifacts_status ON artifacts(status);
```

## Tasks table (PostgreSQL)

```sql
CREATE TABLE tasks (
    id              TEXT PRIMARY KEY,
    parent_id       TEXT,
    phase           TEXT NOT NULL,  -- understand, plan, build, verify, communicate
    class           TEXT NOT NULL,  -- A, B, C, D, E
    agent           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending',
    inputs          JSONB DEFAULT '[]',
    constraints     JSONB DEFAULT '{}',
    acceptance      TEXT,
    retry_count     INT DEFAULT 0,
    created_at      TIMESTAMPTZ DEFAULT NOW(),
    updated_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_agent ON tasks(agent);
CREATE INDEX idx_tasks_class ON tasks(class);
CREATE INDEX idx_tasks_phase ON tasks(phase);
```

---

## Artifact Types Summary

| Type | Producer | Consumers | Summary Fields |
|------|----------|-----------|----------------|
| ResearchBrief | Research | Planner, Developer | topic, source_count, key_finding, relevance_score, has_open_questions |
| CodeMap | Explorer | Planner, Developer, Debugger, Reviewer | root_module, symbol_count, file_count, has_call_graph, key_types |
| ImplementationPlan | Planner | Developer, Reviewer | change_count, risk_level, estimated_complexity, breaking_changes, files_affected |
| ChangeSet | Developer | Reviewer, Testing, Deployment, Docs, Demo | files_modified, lines_added, lines_removed, compilation_status, clippy_clean |
| ReviewVerdict | Reviewer | Developer (if rejected), Orchestrator | approved, blocking_count, suggestion_count, risk_flags |
| DiagnosticReport | Debugger | Developer, Orchestrator | error_class, root_cause_identified, fix_confidence, affected_file_count |
| TestReport | Testing | Debugger (if failures), Deployment, Orchestrator | tests_added, tests_passed, tests_failed, all_passed, coverage_delta |
| ReleaseManifest | Deployment | Documentation, Blog, Demo | version, breaking_changes, crates_published, ci_status |
| DocsUpdate | Documentation | — | files_modified, doc_coverage_delta, has_migration_guide |
| BlogDraft | Blog Writer | — | title, word_count, target_audience |
| DemoPackage | Demo Creator | — | example_count, compiles, runs |

---

## Artifact Lifecycle

```
draft ──→ final ──→ superseded (when retried)
                        │
                        └──→ new version (draft → final)
```

- Agents produce artifacts in `draft` status
- Orchestrator transitions to `final` when acceptance criteria met
- On retry, old artifact marked `superseded`, new one created
- Superseded artifacts kept for audit trail, excluded from context summaries

---

## Task Lifecycle States

```
PENDING → DISPATCHED → IN_PROGRESS → REVIEW → COMPLETED
                │                       │
                └→ BLOCKED              └→ REJECTED → (back to DISPATCHED, retry++)
                      │                                        │
                      └→ ESCALATED ←───────────────────────────┘ (if retry >= max)
```

### State Transition Rules
- PENDING → DISPATCHED: All input artifacts exist in context_store
- DISPATCHED → IN_PROGRESS: Agent acknowledges receipt
- IN_PROGRESS → REVIEW: Agent produces an artifact
- REVIEW → COMPLETED: Acceptance criteria met (or human approves)
- REVIEW → REJECTED: Acceptance criteria not met, retry counter increments
- REJECTED → DISPATCHED: Retry with rejection feedback attached
- Any → BLOCKED: Waiting on dependency or human input
- Any → ESCALATED: Max retries exceeded OR confidence < 0.7 OR stall > 120s

---

## TaskEnvelope Schema

Every dispatch uses this structure:

```json
{
  "task_id": "task_20240315_001",
  "parent_task_id": null,
  "phase": "understand",
  "class": "A",
  "agent": "explorer",
  "inputs": ["art_001"],
  "constraints": {
    "max_retries": 3,
    "timeout_seconds": 300,
    "cost_budget": "M"
  },
  "acceptance_criteria": "CodeMap with call graph for payment routing module",
  "context_hint": "Focus on pub API surface"
}
```

---

## Summary Compression for Orchestrator

The Orchestrator sees ONLY summary fields, never payload. Example:

```json
[
  {"id": "art_001", "type": "CodeMap", "producer": "Explorer", "status": "final",
   "confidence": 0.95, "summary": {"root_module": "crates/router/src/core/payments",
   "symbol_count": 47, "file_count": 12, "has_call_graph": true,
   "key_types": ["PaymentRouter", "ConnectorSelection", "RoutingAlgorithm"]}},
  {"id": "art_002", "type": "ImplementationPlan", "producer": "Planner", "status": "final",
   "confidence": 0.88, "summary": {"change_count": 4, "risk_level": "medium",
   "estimated_complexity": "M", "breaking_changes": false, "files_affected": 4}}
]
```
