import { describe, expect, it } from 'vitest'
import type { AgentEvent } from '../types'
import {
  isAgentDispatchEvent,
  isContainerKeptAliveEvent,
  isErrorEvent,
  isFileEditEvent,
  isPhaseChangeEvent,
  isReasoningEvent,
  isToolCallEvent,
  isUnknownEvent,
} from '../types/events'
import { parseAgentEvent } from './event-parser'

function raw(event_type: string, content: unknown, id = 1): AgentEvent {
  return {
    id,
    execution_id: '00000000-0000-0000-0000-000000000001',
    timestamp: '2026-04-20T00:00:00Z',
    event_type,
    content: content as Record<string, unknown>,
  }
}

describe('parseAgentEvent', () => {
  it('parses reasoning events with text', () => {
    const parsed = parseAgentEvent(raw('reasoning', { agent: 'coder', text: 'thinking...' }))
    expect(isReasoningEvent(parsed)).toBe(true)
    if (isReasoningEvent(parsed)) {
      expect(parsed.content.agent).toBe('coder')
      expect(parsed.content.text).toBe('thinking...')
    }
  })

  it('parses reasoning events that use the legacy `reasoning` key', () => {
    const parsed = parseAgentEvent(raw('reasoning', { agent: 'coder', reasoning: 'plan' }))
    expect(isReasoningEvent(parsed)).toBe(true)
    if (isReasoningEvent(parsed)) {
      expect(parsed.content.text).toBe('plan')
    }
  })

  it('parses tool_call events with args and result', () => {
    const parsed = parseAgentEvent(
      raw('tool_call', {
        agent: 'coder',
        tool: 'read_file',
        args: { path: '/tmp/x' },
        result: 'hello',
      }),
    )
    expect(isToolCallEvent(parsed)).toBe(true)
    if (isToolCallEvent(parsed)) {
      expect(parsed.content.tool).toBe('read_file')
      expect(parsed.content.args).toEqual({ path: '/tmp/x' })
      expect(parsed.content.result).toBe('hello')
    }
  })

  it('parses tool_call events with missing args/result as atomic units', () => {
    const parsed = parseAgentEvent(raw('tool_call', { agent: 'coder', tool: 'bash' }))
    expect(isToolCallEvent(parsed)).toBe(true)
    if (isToolCallEvent(parsed)) {
      expect(parsed.content.args).toBeUndefined()
      expect(parsed.content.result).toBeUndefined()
    }
  })

  it('parses agent_dispatch events', () => {
    const parsed = parseAgentEvent(raw('agent_dispatch', { agent: 'reviewer' }))
    expect(isAgentDispatchEvent(parsed)).toBe(true)
    if (isAgentDispatchEvent(parsed)) {
      expect(parsed.content.agent).toBe('reviewer')
    }
  })

  it('parses error events with just an error message', () => {
    const parsed = parseAgentEvent(raw('error', { error: 'boom' }))
    expect(isErrorEvent(parsed)).toBe(true)
    if (isErrorEvent(parsed)) {
      expect(parsed.content.error).toBe('boom')
      expect(parsed.content.stage).toBeUndefined()
    }
  })

  it('parses error events with stage context', () => {
    const parsed = parseAgentEvent(raw('error', { error: 'timeout', stage: 'poll' }))
    expect(isErrorEvent(parsed)).toBe(true)
    if (isErrorEvent(parsed)) {
      expect(parsed.content.stage).toBe('poll')
    }
  })

  it('parses file_edit events and preserves extra metadata fields', () => {
    const parsed = parseAgentEvent(
      raw('file_edit', { path: 'src/a.ts', before: 'x', after: 'y' }),
    )
    expect(isFileEditEvent(parsed)).toBe(true)
    if (isFileEditEvent(parsed)) {
      expect(parsed.content.path).toBe('src/a.ts')
      expect(parsed.content.before).toBe('x')
      expect(parsed.content.after).toBe('y')
    }
  })

  it('parses phase_change events', () => {
    const parsed = parseAgentEvent(raw('phase_change', { phase: 'reasoning' }))
    expect(isPhaseChangeEvent(parsed)).toBe(true)
    if (isPhaseChangeEvent(parsed)) {
      expect(parsed.content.phase).toBe('reasoning')
    }
  })

  it('parses container_kept_alive events', () => {
    const parsed = parseAgentEvent(
      raw('container_kept_alive', {
        expires_at: '2026-04-20T01:00:00Z',
        keep_alive_secs: 300,
      }),
    )
    expect(isContainerKeptAliveEvent(parsed)).toBe(true)
    if (isContainerKeptAliveEvent(parsed)) {
      expect(parsed.content.keep_alive_secs).toBe(300)
      expect(parsed.content.expires_at).toBe('2026-04-20T01:00:00Z')
    }
  })

  it('falls back to UnknownContent for unrecognized event_type', () => {
    const parsed = parseAgentEvent(raw('novel_future_type', { foo: 'bar' }))
    expect(isUnknownEvent(parsed)).toBe(true)
    if (isUnknownEvent(parsed)) {
      expect(parsed.content.raw_event_type).toBe('novel_future_type')
      expect(parsed.content.raw).toEqual({ foo: 'bar' })
    }
  })

  it('falls back to UnknownContent when reasoning is missing required fields', () => {
    const parsed = parseAgentEvent(raw('reasoning', { agent: 'coder' }))
    expect(isUnknownEvent(parsed)).toBe(true)
    if (isUnknownEvent(parsed)) {
      expect(parsed.content.raw_event_type).toBe('reasoning')
      expect(parsed.content.raw).toEqual({ agent: 'coder' })
    }
  })

  it('falls back to UnknownContent when tool_call is missing tool', () => {
    const parsed = parseAgentEvent(raw('tool_call', { agent: 'coder' }))
    expect(isUnknownEvent(parsed)).toBe(true)
  })

  it('falls back to UnknownContent when file_edit lacks path', () => {
    const parsed = parseAgentEvent(raw('file_edit', { other: 'x' }))
    expect(isUnknownEvent(parsed)).toBe(true)
  })

  it('falls back to UnknownContent when content is not an object', () => {
    const bad: AgentEvent = {
      id: 5,
      execution_id: 'e',
      timestamp: 't',
      event_type: 'reasoning',
      content: null as unknown as Record<string, unknown>,
    }
    const parsed = parseAgentEvent(bad)
    expect(isUnknownEvent(parsed)).toBe(true)
    if (isUnknownEvent(parsed)) {
      expect(parsed.content.raw).toEqual({})
    }
  })

  it('does not throw on arbitrarily malformed input', () => {
    const shapes: AgentEvent[] = [
      { ...raw('reasoning', 42 as unknown) },
      { ...raw('tool_call', 'string' as unknown) },
      { ...raw('error', [] as unknown) },
      { ...raw('', {}) },
    ]
    for (const s of shapes) {
      expect(() => parseAgentEvent(s)).not.toThrow()
    }
  })

  it('preserves id, execution_id, timestamp, and original event_type', () => {
    const parsed = parseAgentEvent(raw('reasoning', { agent: 'a', text: 't' }, 42))
    expect(parsed.id).toBe(42)
    expect(parsed.execution_id).toBe('00000000-0000-0000-0000-000000000001')
    expect(parsed.timestamp).toBe('2026-04-20T00:00:00Z')
    expect(parsed.event_type).toBe('reasoning')
  })

  it('type guards narrow discriminated union correctly', () => {
    const parsed = parseAgentEvent(
      raw('tool_call', { agent: 'a', tool: 'grep', args: { q: 'x' } }),
    )
    // Switch-based narrowing exercise — forces TypeScript to exhaustively check.
    switch (parsed.content.kind) {
      case 'tool_call':
        expect(parsed.content.tool).toBe('grep')
        break
      default:
        throw new Error('expected tool_call narrowing')
    }
  })
})
