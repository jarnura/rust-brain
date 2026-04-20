# PRD Reconciliation: Spec vs. Repo Reality

> Generated 2026-04-21 by CTO against the OpenCode Tracing PRD (RUSA-246).
> OpenCode version: `opencode-ai@latest` (unpinned in `configs/opencode/Dockerfile:4`).

---

## Topology Sketch

```
Backend Entrypoint:   POST /workspaces/:id/execute
                         → services/api/src/handlers/execution.rs:62
                         → tokio::spawn(run_execution(...))

Orchestrator Flow:    services/api/src/execution/runner.rs:101
                         1. Spawn ephemeral Docker container (OpenCode image)
                         2. Wait for container health (GET /health)
                         3. Create OpenCode session (POST /session)
                         4. Send prompt (POST /session/{id}/message) [blocks]
                         5. Poll GET /session/{id}/message every 2s for new parts
                         6. Bridge parts → agent_events rows in Postgres

Transport to UI:      GET /executions/:id/events (SSE)
                         → services/api/src/handlers/execution.rs:157
                         → Polls Postgres every 500ms for new agent_events

UI Entrypoint:        frontend/src/components/ExecutionStream.tsx:204
                         → EventSource on /executions/:id/events
                         → Renders events as flat chronological list

Event-Handling Path:  runner.rs poll loop (lines 329-476) extracts MessagePart variants
                         → classifies as reasoning/tool_call/agent_dispatch/error
                         → insert_agent_event() into Postgres

Persistence Layer:    Postgres table `agent_events`
                         (BIGSERIAL id, execution_id UUID FK, timestamp, event_type, content JSONB)
                         Migration: services/api/migrations/20260403000003_agent_events.sql
```

---

## R-1: Event JSON Shapes vs. §4

**Finding:** The repo does NOT consume raw NDJSON from OpenCode's stdout/stderr. Instead, the execution runner polls OpenCode's REST API (`GET /session/{id}/message`) which returns structured `MessagePart` objects. These are deserialized via a serde tagged enum at `services/api/src/opencode.rs:148-185`:

```
MessagePart variants:
- Text { text: String }                       → stored as event_type "reasoning"
- Reasoning { text: String }                  → stored as event_type "reasoning"
- ToolInvocation { tool_name, args, result, state }  → stored as event_type "tool_call"
- StepStart { id: Option<String> }            → stored as event_type "reasoning"
- StepFinish { reason: Option<String> }       → stored as event_type "reasoning"
- Unknown                                     → skipped (continue)
```

Additional event types minted by the runner (not from OpenCode):
- `agent_dispatch` — when a `task` tool invocation is detected (`runner.rs:416-441`)
- `phase_change` — set via `set_agent_phase` (legacy; not stored as event)
- `container_kept_alive` — system event (`runner.rs:123`)
- `error` — on failures (`runner.rs:148`)

The `content` JSONB field structure per type:
- `reasoning`: `{ "agent": "<name>", "text": "<text>" }` or `{ "agent": "<name>", "reasoning": "<text>" }`
- `tool_call`: `{ "agent": "<name>", "tool": "<name>", "args": {...}, "result": {...} }`
- `agent_dispatch`: `{ "agent": "<dispatched_agent_name>" }`
- `error`: `{ "error": "<message>" }` or `{ "stage": "<stage>", "error": "<message>" }`

**PRD sections affected:** §4.1–4.7 (all event type definitions)

**Edit required:** Rewrite §4 entirely. The event protocol is not NDJSON-over-stream; it is a REST-polled `MessagePart` enum bridged into a Postgres `agent_events` table with 7 allowed `event_type` values. Remove all references to "newline-delimited JSON objects over an ordered byte stream." Replace with the actual REST-poll-and-bridge architecture.

---

## R-2: Tool_use Lifecycle — Single or Streaming?

**Finding:** **Single atomic delivery.** Each `ToolInvocation` part arrives as a complete object containing both input (`args` / `state.input`) and output (`result` / `state.output`). The runner never sees a tool call "in progress" — OpenCode's `GET /session/{id}/message` returns parts only after they're complete.

The runner's polling loop (`runner.rs:329-476`) fetches ALL assistant message parts across ALL turns, counts total parts, and bridges any new ones. But each individual `ToolInvocation` part is atomic — there are no "running" intermediate updates.

Evidence: `opencode.rs:159` defines `state` as a single `Option<serde_json::Value>` — no streaming delta mechanism. The runner processes each part once and inserts one `agent_events` row per part.

**PRD sections affected:** §4.3, §4.9 (I-2, I-4), §5.3, §5.6, §6.1, FR-10, FR-29, FR-30, AC-15

**Edit required:** Remove the entire "update phase" concept. Tool calls are atomic: one `ToolInvocation` part = one `tool_call` event. No call/update/result phases exist. FR-29 (paired units) becomes trivial — each event already carries both input and output. FR-30 (incremental rendering) does not apply. AC-15 (streaming tool output) is N/A. Rewrite §4.3 and I-2 to reflect single-shot delivery.

---

## R-3: Six Architectural Layers → Implementing Files

**Finding:**

| Layer | PRD Section | Implementing File(s) | Line(s) |
|-------|-------------|---------------------|----------|
| 1. Event Ingress | §5.1 | `services/api/src/execution/runner.rs` | 329-476 (poll loop fetching from OpenCode REST API) |
| 2. Parsing & Validation | §5.2 | `services/api/src/opencode.rs` | 148-185 (MessagePart serde enum with tagged dispatch) |
| 3. Correlation | §5.3 | `services/api/src/execution/runner.rs` | 614-637 (`extract_dispatched_agent_name`), 533-610 (`bridge_new_parts`) |
| 4. Storage | §5.4 | `services/api/src/execution/models.rs` | 381-440 (agent_events CRUD) |
| 5. Subscription/Query | §5.5 | `services/api/src/handlers/execution.rs` | 157-230 (SSE stream polling PG every 500ms) |
| 6. Rendering | §5.6 | `frontend/src/components/ExecutionStream.tsx` | 1-316 (full component) |

**PRD sections affected:** §5.1–5.6

**Edit required:** Replace generic layer descriptions with these concrete file references. Note that layers 1-3 are tightly coupled in a single poll loop (not cleanly separated modules). The ingress is NOT an ordered byte stream — it's an HTTP-polled structured API.

---

## R-4: Parser Behavior on Invalid Input

**Finding:**

| Scenario | Behavior | Location |
|----------|----------|----------|
| (a) Invalid JSON from `get_messages` | `reqwest` JSON parse fails → `anyhow::Error` → `warn!` + `continue` in poll loop | `runner.rs:379-383` |
| (b) Known type with missing fields | `serde` deserialization fails for entire message list → same as (a) | `opencode.rs:461-465` |
| (c) Unknown `type` value | Deserializes to `MessagePart::Unknown` → `continue` (silently skipped) | `opencode.rs:184`, `runner.rs:464` |

**Verdict: (c) is a P0 silent-drop bug per PRD §5.2 FR-9.** Unknown types produce no visible artifact. The event is silently discarded.

**PRD sections affected:** §5.2, §8 E-1/E-2/E-3

**Edit required:** Note that (a) and (b) cause entire poll iterations to be skipped (not individual events) — the granularity of failure is "all new parts in this 2s window" not "one bad line." This is fundamentally different from NDJSON parsing where individual lines can fail independently. FR-8/FR-9 need to be restated for a REST-API-poll architecture. Add a P0 task: Unknown MessagePart types MUST be stored as a raw/opaque agent_event rather than silently dropped.

---

## R-5: Tool Call + Result Pairing

**Finding:** **No pairing mechanism exists because none is needed.** OpenCode delivers tool invocations as atomic `ToolInvocation` parts containing both `args`/`state.input` (the call) and `result`/`state.output` (the result) in a single object.

The only "correlation" that exists is for agent dispatches: `extract_dispatched_agent_name()` at `runner.rs:614-637` extracts the agent name from either `args.subagent_type` or `state.input.subagent_type` to detect which sub-agent was dispatched.

There is no `callID`-based pairing, no orphan detection, no incomplete-call marking.

**PRD sections affected:** §4.3, FR-10 through FR-16, §8 E-4/E-5/E-8

**Edit required:** Remove the entire correlation layer specification. Tool calls are self-contained units. Remove FR-11 through FR-16 (correlation FRs). Remove E-4, E-5, E-6, E-8 (orphan/incomplete/collision handling). The correlation concept from the PRD is based on a streaming event model that does not match this repo's REST-poll architecture.

---

## R-6: Session → Run Routing

**Finding:** **Strict 1:1 mapping.** One `Execution` = one OpenCode `Session` = one ephemeral Docker container.

Evidence:
- `runner.rs:258`: `opencode.create_session(Some("rustbrain-execution"))` — one session created per execution
- `models.rs:24`: `session_id: Option<String>` — stored on the Execution row
- `runner.rs:174-184`: One container spawned per execution via `docker.spawn_execution_container()`
- No multiplexing — each execution is completely isolated (own container, own session, own volume mount)

Cardinality: `Workspace 1───* Execution 1───1 Session 1───1 Container 1───* AgentEvent`

**PRD sections affected:** §9, NFR-2, ID-1, ID-5

**Edit required:** Confirm §9's Session↔Run model but simplify: Run = Execution = Session = Container. There's no need for the abstract Session→Run mapping — they're the same entity. NFR-2 (concurrent runs) is satisfied by container isolation — no shared state between executions.

---

## R-7: Current Persistence

**Finding:** Events ARE persisted in Postgres.

**Table:** `agent_events` (migration: `services/api/migrations/20260403000003_agent_events.sql`)

```sql
CREATE TABLE agent_events (
    id BIGSERIAL PRIMARY KEY,             -- monotonic sequence
    execution_id UUID NOT NULL REFERENCES executions(id) ON DELETE CASCADE,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    event_type TEXT NOT NULL CHECK (event_type IN (
        'reasoning', 'tool_call', 'file_edit', 'error',
        'phase_change', 'agent_dispatch', 'container_kept_alive'
    )),
    content JSONB NOT NULL
);
CREATE INDEX idx_agent_events_execution_id ON agent_events(execution_id);
```

**Keying:** Primary key is `id` (BIGSERIAL). Indexed on `execution_id` for per-execution queries.

**Access patterns:**
- `list_agent_events_after(pool, execution_id, after_id)` — cursor-based retrieval (`models.rs:422-440`)
- `insert_agent_event(pool, execution_id, event_type, content)` — append-only writes (`models.rs:381-399`)

**PRD sections affected:** §5.4, D-2

**Edit required:** Lock D-2 to "row-per-event table in Postgres." Note: the storage already satisfies FR-17 (persist every event), FR-18 (streaming reads via `after_id` cursor), FR-19 (append-only — no UPDATE on agent_events). FR-20 (no truncation) is satisfied — full JSONB stored.

---

## R-8: Transport Backend → UI

**Finding:** **SSE (Server-Sent Events)** with poll-from-Postgres.

- Endpoint: `GET /executions/:id/events` (`handlers/execution.rs:157-230`)
- Backend implementation: `async_stream` polling Postgres every 500ms for `agent_events WHERE id > last_event_id`
- Event format: SSE event type `"agent_event"`, data is JSON-serialized `AgentEvent` struct
- Terminal signal: SSE event type `"done"` with `{ "status": "<terminal_status>" }`
- Keep-alive: Axum's `KeepAlive::default()` (15s interval)

**Pull/offset endpoint:** None independent of SSE. The SSE stream itself starts from `last_event_id = 0` and delivers all events. EventSource auto-reconnect effectively provides backfill (starts over from 0, frontend deduplicates via React key on `event.id`).

**Frame size limit:** None enforced. Full JSONB content is serialized into each SSE frame regardless of size.

Frontend SSE client: `frontend/src/api/client.ts:153-202` — `EventSource` with `agent_event` listener + `done` listener + error auto-close.

**PRD sections affected:** §5.5, D-1, FR-20, NFR-4

**Edit required:** Lock D-1 to "SSE push with implicit backfill (re-delivery from seq=0 on reconnect)." No explicit pull endpoint exists — the SSE stream IS both live and historical delivery. D-3 frame limit: none currently enforced (potential issue for very large tool outputs). NFR-4: not implemented. FR-23 (reconnect + backfill): partially satisfied — EventSource reconnects, but the backend restarts from 0 (re-sends all events), relying on the frontend's React keying for dedup.

---

## R-9: Incremental Tool_use Display

**Finding:** **No incremental display.** The renderer shows each event as a completed, atomic line item.

`ExecutionStream.tsx:80-95` (`eventSummary` function) extracts a one-line summary from each event's `content`:
- `tool_call` → displays `"${c.tool}(...)"` (just the tool name)
- Full args/result are NOT rendered — only a summary badge

The entire component is a flat chronological list (`streamEvents.map(...)` at line 299). Tool calls appear as single "TOOL" badge entries. There is no expand/collapse, no streaming output visualization, no running/completed state distinction for individual tools.

**PRD sections affected:** §6.1, FR-29, FR-30, FR-35, FR-37, FR-38

**Edit required:** FR-29 is trivially satisfied (each event already contains both input and output). FR-30 does not apply (no streaming updates exist per R-2). FR-35 (show full arguments + result) is UNMET — only a summary is shown. FR-37 (collapse/expand) is UNMET. FR-38 (visual phase distinction) is N/A (no phases per R-2). These gaps become implementation tasks.

---

## R-10: Dedup Key

**Finding:** **No content-stable dedup key exists.** The system uses Postgres BIGSERIAL `id` as the primary identifier.

- Backend: `list_agent_events_after(pool, execution_id, after_id)` — cursor is the auto-increment PK
- SSE stream: each event carries `.id(event.id.to_string())` as the SSE event ID
- Frontend: React `key={event.id}` (the integer PK)

On reconnect, EventSource resumes from the last-seen SSE event ID if the server supports `Last-Event-ID` header — but the current backend implementation always starts from `last_event_id = 0` (hardcoded at `execution.rs:171`).

**PRD sections affected:** §4.9 I-3, FR-21, FR-24, NFR-11, E-11

**Edit required:** Replace §4.9 I-3's SHA-256 content identity with the actual approach: Postgres BIGSERIAL as the ordering and identity primitive. Content-stable identity is unnecessary given append-only storage and sequential delivery. FR-24 (dedup) is handled by the cursor (`id > last_seen`). NFR-11 (content-keyed render state) — use `id` directly. D-6 (dedup cache) is unnecessary — the database cursor provides implicit dedup.

---

## R-11: Playground Operations Today

**Finding:** The playground supports:

1. **Read-only live viewing** — SSE stream displays events as they arrive (`ExecutionStream.tsx:229-253`)
2. **Agent timeline visualization** — shows dispatched agents as colored pills with active/inactive states (`AgentTimeline` component, lines 160-200)
3. **Per-event type badges** — REASONING, TOOL, AGENT, ERROR, PHASE, DONE (`phaseBadge` function, line 71)
4. **Auto-scroll** to latest events (line 224)
5. **Live/closed indicator** (lines 269-277)
6. **Historical viewing** — after stream closes, events remain rendered from state

**NOT supported:** Step-through replay, editing prompts and re-running, annotating, exporting transcripts, filtering by event type, search within events, collapse/expand individual events.

**PRD sections affected:** §6.1, FR-34 through FR-40

**Edit required:** Narrow §6.1 to: live chronological event rendering with agent timeline. Remove replay, annotation, export FRs. Keep FR-34 (render in seq order — SATISFIED). FR-35 (full tool details — UNMET, only summary shown). FR-36 (500ms P99 update — effectively SATISFIED via 500ms PG poll). FR-37 (collapse/expand — UNMET). FR-38 (phase distinction — N/A). FR-39 (persist after run ends — SATISFIED). FR-40 — no replay/edit/export operations exist.

---

## R-12: Chat UI

**Finding:** **No chat UI embedding exists for trace viewing.** The trace is rendered in a dedicated `ExecutionStream` component panel within the workspace view, not embedded in a broader conversation thread.

The `services/api/src/handlers/chat.rs` handlers are for a separate feature: interactive OpenCode chat sessions via the playground's PromptInput component. They are NOT a trace-viewing surface — they're the prompt submission mechanism.

The "reader persona" for the current trace UI is: a developer who submitted a prompt and watches the execution progress in real-time via the agent timeline + event log.

**PRD sections affected:** §6.2, FR-41 through FR-45

**Edit required:** §6.2 (Chat UI) does not map to any existing surface. The entire "summarized, message-oriented" view specification is net-new work, not a reconciliation against existing code. Either: (a) scope §6.2 as a future phase, or (b) define it as the trace panel's evolution toward a more structured "conversation turns" view. FR-41-FR-45 are all UNMET/N/A for the current implementation.

---

## R-13: Assistant Message Boundaries

**Finding:** **No message-boundary concept exists in the trace UI.** Events are rendered as a flat chronological list with no grouping into "messages" or "turns."

The closest structural concept is the **agent timeline**: `agent_dispatch` events visually mark when a new sub-agent becomes active. But this doesn't create message boundaries — all events remain in a single flat list regardless of which agent produced them.

`StepStart` and `StepFinish` parts from OpenCode ARE captured (as `reasoning` events with `step_start`/`step_finish` content keys, `runner.rs:456-462`), but the frontend does not use them for grouping.

**PRD sections affected:** FR-34, §6.2

**Edit required:** Update FR-34 — currently "message boundaries" don't exist. If the chat UI (§6.2) is implemented, the boundary rule should be: each `agent_dispatch` event starts a new agent turn. Within an agent's turn, `StepStart`/`StepFinish` could sub-divide into steps. But this is entirely new UI logic to build.

---

## R-14: Logging / Metrics Stack

**Finding:**

**Logging:**
- Framework: `tracing` crate with `info!`, `warn!`, `debug!` macros
- Init: `rustbrain_common::logging::init_logging_with_directives(Level::INFO, &["rustbrain_api=debug"])` at `main.rs:61`
- Sink: stdout (structured text by default; format configurable via `LOG_FORMAT` env var)
- Namespace conventions: crate-level filtering (`rustbrain_api=debug`)

**Metrics:**
- Framework: `prometheus` crate (v0.13) with custom `Registry`
- Families: `rustbrain_api_requests_total` (counter), `rustbrain_api_request_duration_seconds` (histogram), `rustbrain_api_errors_total` (counter)
- Labels: `endpoint`, `method`, `workspace`, `error_code`
- Per-workspace gauges: `services/api/src/metrics/workspace_gauges.rs`
- Scrape endpoint: `GET /metrics` (Prometheus text format)
- Middleware: `services/api/src/middleware.rs` — intercepts every request, records workspace-labeled metrics

**Infrastructure:**
- Prometheus: `prom/prometheus:v2.51.0` at port 9090 (`docker-compose.yml:199`)
- Grafana: `grafana/grafana:11.0.0` at port 3000 (`docker-compose.yml:227`)
- Postgres exporter: `prometheuscommunity/postgres-exporter:v0.15.0` (`docker-compose.yml:159`)
- Blackbox exporter for health probes (`docker-compose.yml:179`)

**Execution-specific metrics:** NONE. No counters for events bridged, tool calls processed, containers spawned, poll failures, etc.

**PRD sections affected:** NFR-10

**Edit required:** Lock NFR-10 to: structured `tracing` logs (warn/info) + Prometheus counters. For execution tracing observability, ADD metrics: `rustbrain_execution_events_bridged_total`, `rustbrain_execution_poll_failures_total`, `rustbrain_execution_unknown_parts_total` (for the R-4 silent drop). Use existing `tracing::warn!` for failure-candidate sites (already present in runner.rs for most error paths).

---

## R-15: OpenCode Version

**Finding:** **Version is unpinned.**

- Dockerfile: `RUN npm install -g opencode-ai @ai-sdk/openai-compatible` (no version specifier) at `configs/opencode/Dockerfile:4`
- Config schema: `"$schema": "https://opencode.ai/config.json"` at `configs/opencode/opencode.json:2`
- Server command: `opencode serve --port 4096 --hostname 0.0.0.0` at `configs/opencode/docker-entrypoint.sh:195`
- API shape observed: `POST /session` (create), `GET /session/{id}/message` (list messages as `[{info, parts}]`), `POST /session/{id}/message` (send, blocks until response complete)
- Message format: `{info: {id, role, sessionID, time}, parts: [{type: "text"|"reasoning"|"tool"|"tool-invocation"|"step-start"|"step-finish", ...}]}`

The event schema in §4 does NOT match the actual protocol. OpenCode does not emit NDJSON events — it serves structured JSON via REST. The "event types" in §4 (`text`, `reasoning`, `tool_use`, `step_start`, `step_finish`, `error`) roughly correspond to `MessagePart` variants but with different shapes and delivery semantics.

**PRD sections affected:** §4 header, entire §4 schema specification

**Edit required:** Record version as "opencode-ai@latest (unpinned, ~April 2026)". Pin in Dockerfile for reproducibility. Rewrite §4 to document the actual `MessagePart` enum shapes as received from `GET /session/{id}/message`, not the speculative NDJSON format. The PRD's entire event protocol section is based on incorrect assumptions about the delivery mechanism.

---

## Summary: PRD Validity Assessment

| Layer | PRD Status | Notes |
|-------|-----------|-------|
| §4 Event Protocol | **REWRITE REQUIRED** | Assumes NDJSON stream; actual is REST-poll of structured parts |
| §5.1 Event Ingress | **REWRITE** | Not a byte stream; it's HTTP polling |
| §5.2 Parsing | **PARTIAL** | Serde enum exists, but unknown-type handling is a bug |
| §5.3 Correlation | **REMOVE** | Tool calls are atomic; no pairing needed |
| §5.4 Storage | **SATISFIED** | Postgres agent_events table matches requirements |
| §5.5 Subscription | **PARTIAL** | SSE works but lacks reconnect-with-cursor support |
| §5.6 Rendering | **PARTIAL** | Basic chronological rendering exists; detail views missing |
| §6.1 Playground | **PARTIAL** | Live viewing works; collapse/expand/details missing |
| §6.2 Chat UI | **N/A** | No such surface exists; entirely new work |
| §9 Data Model | **SIMPLIFY** | 1:1 Execution=Session=Container; simpler than PRD assumes |
| §10 Decisions | **MOSTLY LOCKED** | Transport=SSE, Storage=PG, no streaming tools |

**Critical gaps requiring tasks:**
1. P0: Unknown `MessagePart` types silently dropped (R-4c)
2. P1: No `Last-Event-ID` support in SSE handler for proper reconnect (R-8)
3. P1: No execution-specific Prometheus metrics (R-14)
4. P2: Tool call details not shown in UI — only summary (R-9, R-11)
5. P2: No collapse/expand for events (R-11)
6. P3: OpenCode version unpinned (R-15)
