import { describe, expect, it } from 'vitest'
import type { TypedAgentEvent } from '../types/events'
import {
  buildNavUnits,
  eventMatchesQuery,
  groupConsecutiveEvents,
  isNearBottom,
  nextNavIndex,
  prevNavIndex,
  summarizeGroup,
} from './collapsible-grouping'

let nextId = 1
function reset() {
  nextId = 1
}

function makeEvent(content: TypedAgentEvent['content']): TypedAgentEvent {
  return {
    id: nextId++,
    execution_id: '00000000-0000-0000-0000-000000000001',
    timestamp: '2026-04-20T00:00:00Z',
    event_type: content.kind === 'unknown' ? content.raw_event_type : content.kind,
    content,
  }
}

describe('groupConsecutiveEvents', () => {
  it('returns an empty array for empty input', () => {
    reset()
    expect(groupConsecutiveEvents([])).toEqual([])
  })

  it('merges consecutive same-kind events into one group', () => {
    reset()
    const events = [
      makeEvent({ kind: 'reasoning', agent: 'coder', text: 'a' }),
      makeEvent({ kind: 'reasoning', agent: 'coder', text: 'b' }),
      makeEvent({ kind: 'reasoning', agent: 'coder', text: 'c' }),
    ]
    const groups = groupConsecutiveEvents(events)
    expect(groups).toHaveLength(1)
    expect(groups[0].kind).toBe('reasoning')
    expect(groups[0].events).toHaveLength(3)
  })

  it('starts a new group when kind changes', () => {
    reset()
    const events = [
      makeEvent({ kind: 'reasoning', agent: 'coder', text: 'a' }),
      makeEvent({ kind: 'phase_change', phase: 'planning' }),
      makeEvent({ kind: 'reasoning', agent: 'coder', text: 'b' }),
    ]
    const groups = groupConsecutiveEvents(events)
    expect(groups.map((g) => g.kind)).toEqual(['reasoning', 'phase_change', 'reasoning'])
    expect(groups.every((g) => g.events.length === 1)).toBe(true)
  })

  it('always isolates tool_call events into singleton groups', () => {
    reset()
    const events = [
      makeEvent({ kind: 'tool_call', agent: 'coder', tool: 'read_file', result: 'ok' }),
      makeEvent({ kind: 'tool_call', agent: 'coder', tool: 'read_file', result: 'ok' }),
      makeEvent({ kind: 'tool_call', agent: 'coder', tool: 'write_file', result: 'ok' }),
    ]
    const groups = groupConsecutiveEvents(events)
    expect(groups).toHaveLength(3)
    expect(groups.every((g) => g.events.length === 1)).toBe(true)
  })

  it('preserves event order within and across groups', () => {
    reset()
    const events = [
      makeEvent({ kind: 'reasoning', agent: 'a', text: '1' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: '2' }),
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 't', result: 'ok' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: '3' }),
    ]
    const groups = groupConsecutiveEvents(events)
    const flat = groups.flatMap((g) => g.events.map((e) => e.id))
    expect(flat).toEqual([1, 2, 3, 4])
  })

  it('does not silently omit events of unknown kinds', () => {
    reset()
    const events = [
      makeEvent({ kind: 'unknown', raw_event_type: 'mystery', raw: { foo: 1 } }),
      makeEvent({ kind: 'unknown', raw_event_type: 'mystery', raw: { foo: 2 } }),
    ]
    const groups = groupConsecutiveEvents(events)
    expect(groups).toHaveLength(1)
    expect(groups[0].events).toHaveLength(2)
  })
})

describe('summarizeGroup (FR-32: counts must always appear)', () => {
  it('summarizes a multi-reasoning group with the count', () => {
    reset()
    const events = [
      makeEvent({ kind: 'reasoning', agent: 'a', text: 'x' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: 'y' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: 'z' }),
    ]
    const [g] = groupConsecutiveEvents(events)
    expect(summarizeGroup(g)).toBe('3 reasoning events')
  })

  it('renders a singleton tool call summary with status', () => {
    reset()
    const [g] = groupConsecutiveEvents([
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 'search_code', result: { rows: [] } }),
    ])
    expect(summarizeGroup(g)).toBe('tool: search_code → completed')
  })

  it('marks pending tool calls', () => {
    reset()
    const [g] = groupConsecutiveEvents([
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 'slow_tool' }),
    ])
    expect(summarizeGroup(g)).toBe('tool: slow_tool → pending')
  })

  it('marks tool calls whose result encodes an error', () => {
    reset()
    const [g] = groupConsecutiveEvents([
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 'edit', result: { is_error: true } }),
    ])
    expect(summarizeGroup(g)).toBe('tool: edit → error')
  })

  it('summarizes phase changes by name when singleton', () => {
    reset()
    const [g] = groupConsecutiveEvents([
      makeEvent({ kind: 'phase_change', phase: 'planning' }),
    ])
    expect(summarizeGroup(g)).toBe('phase: planning')
  })

  it('summarizes file edits by path when singleton', () => {
    reset()
    const [g] = groupConsecutiveEvents([
      makeEvent({ kind: 'file_edit', path: 'src/a.rs' }),
    ])
    expect(summarizeGroup(g)).toBe('file: src/a.rs')
  })
})

describe('buildNavUnits', () => {
  it('expands all groups when none are collapsed', () => {
    reset()
    const groups = groupConsecutiveEvents([
      makeEvent({ kind: 'reasoning', agent: 'a', text: '1' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: '2' }),
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 't', result: 'ok' }),
    ])
    const units = buildNavUnits(groups, new Set())
    expect(units).toHaveLength(3)
    expect(units.every((u) => u.kind === 'event')).toBe(true)
  })

  it('replaces a collapsed group with a single header unit', () => {
    reset()
    const groups = groupConsecutiveEvents([
      makeEvent({ kind: 'reasoning', agent: 'a', text: '1' }),
      makeEvent({ kind: 'reasoning', agent: 'a', text: '2' }),
      makeEvent({ kind: 'tool_call', agent: 'a', tool: 't', result: 'ok' }),
    ])
    const collapsed = new Set<string>([groups[0].id])
    const units = buildNavUnits(groups, collapsed)
    expect(units).toHaveLength(2)
    expect(units[0].kind).toBe('group-header')
    expect(units[0].groupId).toBe(groups[0].id)
    expect(units[1].kind).toBe('event')
  })
})

describe('nextNavIndex / prevNavIndex', () => {
  it('clamps at the boundaries instead of wrapping', () => {
    expect(nextNavIndex(4, 5)).toBe(4)
    expect(nextNavIndex(0, 5)).toBe(1)
    expect(prevNavIndex(0, 5)).toBe(0)
    expect(prevNavIndex(3, 5)).toBe(2)
  })

  it('returns 0 when the list is empty', () => {
    expect(nextNavIndex(0, 0)).toBe(0)
    expect(prevNavIndex(0, 0)).toBe(0)
  })

  it('coerces negative current values back to a valid range', () => {
    expect(nextNavIndex(-3, 4)).toBe(0)
    expect(prevNavIndex(-3, 4)).toBe(0)
  })
})

describe('isNearBottom', () => {
  it('is true when the viewport sits at the bottom', () => {
    expect(isNearBottom(800, 1000, 200)).toBe(true)
  })

  it('is true within the default 32px threshold', () => {
    expect(isNearBottom(770, 1000, 200)).toBe(true)
  })

  it('is false once the user scrolls past the threshold', () => {
    expect(isNearBottom(500, 1000, 200)).toBe(false)
  })

  it('honors a custom threshold', () => {
    expect(isNearBottom(500, 1000, 200, 400)).toBe(true)
  })
})

describe('eventMatchesQuery', () => {
  it('returns false for empty queries', () => {
    reset()
    const ev = makeEvent({ kind: 'reasoning', agent: 'coder', text: 'hello' })
    expect(eventMatchesQuery(ev, '')).toBe(false)
    expect(eventMatchesQuery(ev, '   ')).toBe(false)
  })

  it('matches against reasoning text case-insensitively', () => {
    reset()
    const ev = makeEvent({ kind: 'reasoning', agent: 'coder', text: 'Reading file' })
    expect(eventMatchesQuery(ev, 'reading')).toBe(true)
    expect(eventMatchesQuery(ev, 'WRITING')).toBe(false)
  })

  it('matches against tool name and args', () => {
    reset()
    const ev = makeEvent({
      kind: 'tool_call',
      agent: 'coder',
      tool: 'search_code',
      args: { query: 'extract_calls' },
    })
    expect(eventMatchesQuery(ev, 'search_code')).toBe(true)
    expect(eventMatchesQuery(ev, 'extract_calls')).toBe(true)
  })

  it('matches against file_edit path', () => {
    reset()
    const ev = makeEvent({ kind: 'file_edit', path: 'crates/rustbrain-common/src/lib.rs' })
    expect(eventMatchesQuery(ev, 'rustbrain-common')).toBe(true)
  })
})

describe('performance: 500 events', () => {
  it('groups and summarizes within one frame budget', () => {
    reset()
    const events: TypedAgentEvent[] = []
    for (let i = 0; i < 500; i++) {
      const kind = i % 3
      if (kind === 0) {
        events.push(makeEvent({ kind: 'reasoning', agent: 'coder', text: `step ${i}` }))
      } else if (kind === 1) {
        events.push(
          makeEvent({
            kind: 'tool_call',
            agent: 'coder',
            tool: i % 2 ? 'search_code' : 'read_file',
            args: { query: `q${i}` },
            result: { ok: true },
          }),
        )
      } else {
        events.push(makeEvent({ kind: 'phase_change', phase: `phase-${i}` }))
      }
    }

    const t0 = performance.now()
    const groups = groupConsecutiveEvents(events)
    groups.forEach((g) => summarizeGroup(g))
    const elapsed = performance.now() - t0

    const totalEvents = groups.reduce((acc, g) => acc + g.events.length, 0)
    expect(totalEvents).toBe(500)
    expect(elapsed).toBeLessThan(16)
  })
})
