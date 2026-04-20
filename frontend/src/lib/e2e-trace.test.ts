// End-to-end trace integration test (RUSA-258, Phase 4A — happy path scope).
//
// Drives a realistic OpenCode execution fixture through the full frontend
// display pipeline: raw SSE `AgentEvent[]` → `parseAgentEvent` → typed events
// → `groupConsecutiveEvents` → `buildNavUnits` → `ToolCallCard` helpers
// (`deriveToolCallStatus`, `summarizeArgs`, `formatValue`) → search filter →
// `copyToClipboard`.
//
// Per the CTO's scoping comment on RUSA-258 (2026-04-20), this covers the
// happy-path acceptance criteria only:
//   - Trace display renders correctly (event list, timestamps)
//   - ToolCallCard paired rendering (args + result)
//   - Collapse/expand groups (J/K nav, search filter)
//   - Copy-to-clipboard on tool call detail
//
// Deferred until RUSA-257 (SSE reconnection) lands:
//   - Disconnect/reconnect gap-free transcript
//   - Cursor resume after disconnect

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentEvent } from '../types'
import {
  isReasoningEvent,
  isToolCallEvent,
  type TypedAgentEvent,
} from '../types/events'
import {
  copyToClipboard,
  deriveToolCallStatus,
  formatValue,
  summarizeArgs,
} from '../components/ToolCallCard'
import { parseAgentEvent } from './event-parser'
import {
  buildNavUnits,
  eventMatchesQuery,
  groupConsecutiveEvents,
  nextNavIndex,
  prevNavIndex,
  summarizeGroup,
} from './collapsible-grouping'

// ─── Fixture: realistic OpenCode execution transcript ────────────────────────
//
// Simulates: user prompt → agent dispatch (explore) → reasoning → tool_call
// (search_code with args+result) → reasoning → tool_call (read_file) → agent
// dispatch (develop) → reasoning → tool_call (write_file) → file_edit →
// reasoning → phase_change → done. Mirrors the wire format emitted by the
// runner in services/api/src/execution/runner.rs.

const EXEC_ID = '11111111-2222-3333-4444-555555555555'

let rawCounter = 1

function rawEvent(
  event_type: string,
  content: Record<string, unknown>,
  tsMs: number,
): AgentEvent {
  return {
    id: rawCounter++,
    execution_id: EXEC_ID,
    timestamp: new Date(tsMs).toISOString(),
    event_type,
    content,
  }
}

function buildFixture(): AgentEvent[] {
  rawCounter = 1
  const t0 = Date.parse('2026-04-20T12:00:00Z')
  return [
    rawEvent('agent_dispatch', { agent: 'explore' }, t0 + 0),
    rawEvent(
      'reasoning',
      { agent: 'explore', text: 'Looking at the extraction pipeline.' },
      t0 + 120,
    ),
    rawEvent(
      'tool_call',
      {
        agent: 'explore',
        tool: 'search_code',
        args: { query: 'extract_calls', scope: 'services/ingestion' },
        result: {
          rows: [
            { path: 'services/ingestion/src/typecheck/resolver.rs', line: 42 },
          ],
        },
      },
      t0 + 400,
    ),
    rawEvent(
      'reasoning',
      { agent: 'explore', text: 'Found the resolver; reading the call site.' },
      t0 + 700,
    ),
    rawEvent(
      'tool_call',
      {
        agent: 'explore',
        tool: 'read_file',
        args: { path: 'services/ingestion/src/typecheck/resolver.rs' },
        result: 'pub fn extract_calls(...) -> Vec<CallSite> { ... }',
      },
      t0 + 1100,
    ),
    rawEvent('agent_dispatch', { agent: 'develop' }, t0 + 1500),
    rawEvent(
      'reasoning',
      { agent: 'develop', text: 'Adding a guard for empty paths.' },
      t0 + 1600,
    ),
    rawEvent(
      'tool_call',
      {
        agent: 'develop',
        tool: 'write_file',
        args: {
          path: 'services/ingestion/src/typecheck/resolver.rs',
          content: '... patched source ...',
        },
        result: { is_error: false, bytes_written: 2048 },
      },
      t0 + 2000,
    ),
    rawEvent(
      'file_edit',
      {
        path: 'services/ingestion/src/typecheck/resolver.rs',
        before: 'old',
        after: 'new',
      },
      t0 + 2100,
    ),
    rawEvent(
      'reasoning',
      { agent: 'develop', text: 'Done. Handing back to orchestrator.' },
      t0 + 2200,
    ),
    rawEvent('phase_change', { phase: 'done' }, t0 + 2300),
  ]
}

// ─── 1. Trace display renders (event list, timestamps) ───────────────────────

describe('e2e-trace: trace display pipeline', () => {
  it('parses every fixture event into a typed event without dropping any', () => {
    const raw = buildFixture()
    const typed = raw.map(parseAgentEvent)
    expect(typed).toHaveLength(raw.length)
    // No event falls into the Unknown bucket on the happy path.
    expect(typed.every((e) => e.content.kind !== 'unknown')).toBe(true)
  })

  it('preserves wire-format fields (id, execution_id, timestamp, event_type)', () => {
    const raw = buildFixture()
    const typed = raw.map(parseAgentEvent)
    raw.forEach((r, i) => {
      expect(typed[i].id).toBe(r.id)
      expect(typed[i].execution_id).toBe(r.execution_id)
      expect(typed[i].timestamp).toBe(r.timestamp)
      expect(typed[i].event_type).toBe(r.event_type)
    })
  })

  it('produces monotonically non-decreasing timestamps for display order', () => {
    const typed = buildFixture().map(parseAgentEvent)
    for (let i = 1; i < typed.length; i++) {
      expect(Date.parse(typed[i].timestamp)).toBeGreaterThanOrEqual(
        Date.parse(typed[i - 1].timestamp),
      )
    }
  })

  it('renders a valid HH:MM:SS clock string for every event timestamp', () => {
    // ExecutionStream.tsx formats timestamps via toLocaleTimeString.
    const typed = buildFixture().map(parseAgentEvent)
    for (const ev of typed) {
      const clock = new Date(ev.timestamp).toLocaleTimeString('en-US', {
        hour12: false,
      })
      expect(clock).toMatch(/^\d{1,2}:\d{2}:\d{2}$/)
    }
  })
})

// ─── 2. ToolCallCard paired rendering (args + result) ────────────────────────

describe('e2e-trace: ToolCallCard paired rendering', () => {
  it('every tool_call in the fixture exposes both args and a result', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const toolCalls = typed.filter(isToolCallEvent)
    expect(toolCalls).toHaveLength(3)
    for (const tc of toolCalls) {
      expect(tc.content.args).toBeDefined()
      expect(tc.content.result).toBeDefined()
    }
  })

  it('derives a completed status for every happy-path tool_call', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const toolCalls = typed.filter(isToolCallEvent)
    for (const tc of toolCalls) {
      expect(deriveToolCallStatus(tc.content)).toBe('completed')
    }
  })

  it('produces non-empty args + result text for paired display', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const toolCalls = typed.filter(isToolCallEvent)
    for (const tc of toolCalls) {
      const argsText = formatValue(tc.content.args)
      const resultText = formatValue(tc.content.result)
      expect(argsText.length).toBeGreaterThan(0)
      expect(resultText.length).toBeGreaterThan(0)
    }
  })

  it('summarizes tool args compactly for the collapsed header row', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const firstTool = typed.filter(isToolCallEvent)[0]
    const summary = summarizeArgs(firstTool.content.args)
    expect(summary.length).toBeLessThanOrEqual(61)
    expect(summary).toContain('extract_calls')
  })
})

// ─── 3. Collapsible groups, J/K nav, search filter ───────────────────────────

describe('e2e-trace: grouping + J/K nav + search filter', () => {
  it('isolates every tool_call into its own group (per R-2 atomic units)', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const groups = groupConsecutiveEvents(typed)
    const toolGroups = groups.filter((g) => g.kind === 'tool_call')
    expect(toolGroups).toHaveLength(3)
    expect(toolGroups.every((g) => g.events.length === 1)).toBe(true)
  })

  it('renders a count suffix on multi-event group summaries (FR-32 no silent omission)', () => {
    const typed: TypedAgentEvent[] = [
      ...buildFixture().map(parseAgentEvent),
      // Append two more reasoning events to form a merged group.
      parseAgentEvent(
        rawEvent(
          'reasoning',
          { agent: 'develop', text: 'trailing note A' },
          Date.now(),
        ),
      ),
      parseAgentEvent(
        rawEvent(
          'reasoning',
          { agent: 'develop', text: 'trailing note B' },
          Date.now() + 1,
        ),
      ),
    ]
    const groups = groupConsecutiveEvents(typed)
    const multi = groups.find((g) => g.kind === 'reasoning' && g.events.length > 1)
    expect(multi).toBeDefined()
    if (multi) {
      expect(summarizeGroup(multi)).toMatch(/^\d+ reasoning events$/)
    }
  })

  it('J/K keyboard nav steps through every navigable unit without wrapping', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const groups = groupConsecutiveEvents(typed)
    const units = buildNavUnits(groups, new Set())
    expect(units.length).toBeGreaterThan(0)

    // Walk forward with J from index 0 to the end; never wrap past the last.
    let idx = 0
    const visited: number[] = [idx]
    for (let i = 0; i < units.length + 3; i++) {
      idx = nextNavIndex(idx, units.length)
      visited.push(idx)
    }
    expect(visited[visited.length - 1]).toBe(units.length - 1)

    // Walk backward with K; never wrap before 0.
    let back = units.length - 1
    for (let i = 0; i < units.length + 3; i++) {
      back = prevNavIndex(back, units.length)
    }
    expect(back).toBe(0)
  })

  it('collapse/expand toggles a group between header-only and per-event units', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const groups = groupConsecutiveEvents(typed)
    const target = groups.find((g) => g.kind === 'tool_call')
    expect(target).toBeDefined()
    if (!target) return

    const expanded = buildNavUnits(groups, new Set())
    const collapsed = buildNavUnits(groups, new Set([target.id]))

    // Collapsed variant has exactly one fewer unit (the tool_call group is
    // a singleton, so it collapses from 1 event-unit to 1 header-unit → same
    // length). Use a multi-event group instead to see a real reduction.
    expect(expanded.length).toBe(groups.length)
    expect(collapsed.length).toBe(groups.length)

    // The collapsed unit is now a header, not an event row.
    const hdr = collapsed.find((u) => u.groupId === target.id)
    expect(hdr?.kind).toBe('group-header')
    const row = expanded.find((u) => u.groupId === target.id)
    expect(row?.kind).toBe('event')
  })

  it('search filter matches tool name, args, and reasoning text', () => {
    const typed = buildFixture().map(parseAgentEvent)

    const matches = (q: string) =>
      typed.filter((e) => eventMatchesQuery(e, q)).length

    // Tool name appears in exactly one tool_call event.
    expect(matches('search_code')).toBe(1)
    // Query argument appears in the search_code args AND in the reasoning text
    // that referenced it.
    expect(matches('extract_calls')).toBeGreaterThanOrEqual(1)
    // File path appears in read_file args, write_file args, and file_edit.
    expect(matches('resolver.rs')).toBeGreaterThanOrEqual(3)
    // Case-insensitive match on reasoning text.
    expect(matches('ORCHESTRATOR')).toBe(1)
    // Empty/whitespace query never matches (guards against accidental
    // highlight of every row).
    expect(matches('')).toBe(0)
    expect(matches('   ')).toBe(0)
  })

  it('reasoning events survive grouping without dropping text', () => {
    const typed = buildFixture().map(parseAgentEvent)
    const reasoning = typed.filter(isReasoningEvent)
    expect(reasoning).toHaveLength(4)
    for (const ev of reasoning) {
      expect(ev.content.text.length).toBeGreaterThan(0)
    }
  })
})

// ─── 4. Copy-to-clipboard on tool call detail ────────────────────────────────

describe('e2e-trace: copy-to-clipboard on tool call detail', () => {
  const originalClipboard = navigator.clipboard

  beforeEach(() => {
    vi.restoreAllMocks()
  })

  afterEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: originalClipboard,
    })
  })

  it('copies the pretty-printed args for a tool call from the fixture', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })

    const typed = buildFixture().map(parseAgentEvent)
    const firstTool = typed.filter(isToolCallEvent)[0]
    const payload = formatValue(firstTool.content.args)

    const ok = await copyToClipboard(payload)
    expect(ok).toBe(true)
    expect(writeText).toHaveBeenCalledWith(payload)
    // The copied payload is pretty JSON containing the query arg.
    expect(payload).toContain('"query"')
    expect(payload).toContain('extract_calls')
  })

  it('copies the pretty-printed result for a tool call from the fixture', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    })

    const typed = buildFixture().map(parseAgentEvent)
    const writeTool = typed
      .filter(isToolCallEvent)
      .find((e) => e.content.tool === 'write_file')
    expect(writeTool).toBeDefined()
    if (!writeTool) return

    const payload = formatValue(writeTool.content.result)
    const ok = await copyToClipboard(payload)
    expect(ok).toBe(true)
    expect(writeText).toHaveBeenCalledWith(payload)
    expect(payload).toContain('bytes_written')
  })

  it('degrades gracefully when the clipboard API is unavailable', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: undefined,
    })
    const ok = await copyToClipboard('anything')
    expect(ok).toBe(false)
  })
})
