// Tests for ReconnectingEventSource (RUSA-257).
//
// Covers: exponential backoff, cursor-based reconnect via last_event_id query
// param, duplicate detection, gap detection, and connection-state transitions.
// EventSource is mocked so we can drive the lifecycle deterministically.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  openReconnectingEventSource,
  type ConnectionState,
  type ReconnectingClientCallbacks,
} from './sse-reconnect'

// ─── MockEventSource ─────────────────────────────────────────────────────────

type Listener = (e: MessageEvent) => void

class MockEventSource {
  static instances: MockEventSource[] = []

  url: string
  readyState: number = 0
  onopen: ((e: Event) => void) | null = null
  onerror: ((e: Event) => void) | null = null
  onmessage: ((e: MessageEvent) => void) | null = null
  closed = false
  listeners: Record<string, Listener[]> = {}

  constructor(url: string) {
    this.url = url
    MockEventSource.instances.push(this)
  }

  addEventListener(name: string, cb: Listener): void {
    if (!this.listeners[name]) this.listeners[name] = []
    this.listeners[name].push(cb)
  }

  removeEventListener(name: string, cb: Listener): void {
    const arr = this.listeners[name]
    if (!arr) return
    const idx = arr.indexOf(cb)
    if (idx >= 0) arr.splice(idx, 1)
  }

  close(): void {
    this.closed = true
    this.readyState = 2
  }

  // Test helpers
  _open(): void {
    this.readyState = 1
    this.onopen?.({ type: 'open' } as Event)
  }

  _fireNamed(name: string, data: string, lastEventId = ''): void {
    const ev = { data, lastEventId, type: name } as MessageEvent
    for (const cb of this.listeners[name] ?? []) cb(ev)
  }

  _error(): void {
    this.readyState = 2
    this.onerror?.({ type: 'error' } as Event)
  }

  static reset(): void {
    MockEventSource.instances = []
  }

  static last(): MockEventSource {
    const inst = MockEventSource.instances[MockEventSource.instances.length - 1]
    if (!inst) throw new Error('no MockEventSource instances yet')
    return inst
  }
}

// ─── Test harness ────────────────────────────────────────────────────────────

interface Harness {
  events: Array<{ data: unknown; seq: number | null }>
  states: ConnectionState[]
  gaps: Array<{ expected: number; actual: number }>
  doneCount: number
  callbacks: ReconnectingClientCallbacks
}

function makeHarness(): Harness {
  const events: Array<{ data: unknown; seq: number | null }> = []
  const states: ConnectionState[] = []
  const gaps: Array<{ expected: number; actual: number }> = []
  const h: Harness = {
    events,
    states,
    gaps,
    doneCount: 0,
    callbacks: {
      onEvent: (data: unknown, seq: number | null) => events.push({ data, seq }),
      onStateChange: (s: ConnectionState) => states.push(s),
      onGap: (expected: number, actual: number) => gaps.push({ expected, actual }),
      onDone: () => {
        h.doneCount += 1
      },
    },
  }
  return h
}

function lastState(h: Harness): ConnectionState | undefined {
  return h.states[h.states.length - 1]
}

function fireAgentEvent(es: MockEventSource, seq: number, payload: unknown): void {
  es._fireNamed('agent_event', JSON.stringify(payload), String(seq))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('openReconnectingEventSource', () => {
  beforeEach(() => {
    MockEventSource.reset()
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('reports connecting → connected and delivers events', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/executions/e1/events',
        eventName: 'agent_event',
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    expect(h.states).toEqual(['connecting'])
    const es = MockEventSource.last()
    es._open()
    expect(h.states).toEqual(['connecting', 'connected'])

    fireAgentEvent(es, 1, { id: 1, event_type: 'reasoning' })
    fireAgentEvent(es, 2, { id: 2, event_type: 'tool_call' })

    expect(h.events).toEqual([
      { data: { id: 1, event_type: 'reasoning' }, seq: 1 },
      { data: { id: 2, event_type: 'tool_call' }, seq: 2 },
    ])
  })

  it('transitions to reconnecting on error and reconnects with last_event_id cursor', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/executions/e1/events',
        eventName: 'agent_event',
        initialBackoffMs: 1000,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es1 = MockEventSource.last()
    es1._open()
    fireAgentEvent(es1, 1, { id: 1 })
    fireAgentEvent(es1, 2, { id: 2 })

    es1._error()

    // Should close the old source immediately
    expect(es1.closed).toBe(true)
    // Connection state should flip to reconnecting synchronously
    expect(lastState(h)).toBe('reconnecting')
    // No new EventSource yet — backoff in progress
    expect(MockEventSource.instances).toHaveLength(1)

    vi.advanceTimersByTime(999)
    expect(MockEventSource.instances).toHaveLength(1)

    vi.advanceTimersByTime(1)
    expect(MockEventSource.instances).toHaveLength(2)

    const es2 = MockEventSource.last()
    expect(es2.url).toContain('last_event_id=2')
    expect(lastState(h)).toBe('connecting')
    es2._open()
    expect(lastState(h)).toBe('connected')
  })

  it('uses exponential backoff up to maxBackoffMs', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 1000,
        maxBackoffMs: 8000,
        maxAttempts: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    function cycle(expectedDelay: number, instanceCount: number) {
      const es = MockEventSource.last()
      es._open()
      es._error()
      // Delay should not fire before `expectedDelay`
      vi.advanceTimersByTime(expectedDelay - 1)
      expect(MockEventSource.instances).toHaveLength(instanceCount - 1)
      vi.advanceTimersByTime(1)
      expect(MockEventSource.instances).toHaveLength(instanceCount)
    }

    cycle(1000, 2) // 1s
    cycle(2000, 3) // 2s
    cycle(4000, 4) // 4s
    cycle(8000, 5) // 8s (cap)
    cycle(8000, 6) // still 8s
  })

  it('transitions to disconnected after maxAttempts errors', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        maxBackoffMs: 10,
        maxAttempts: 2,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    // initial + 2 retries = 3 total allowed attempts
    for (let i = 0; i < 2; i++) {
      const es = MockEventSource.last()
      es._open()
      es._error()
      vi.advanceTimersByTime(10)
    }

    // Third error: no more retries
    const es3 = MockEventSource.last()
    es3._error()
    vi.advanceTimersByTime(1000)

    expect(lastState(h)).toBe('disconnected')
    // Should not have spawned a 4th attempt
    expect(MockEventSource.instances).toHaveLength(3)
  })

  it('deduplicates events by seq (same seq delivered twice = once forwarded)', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es = MockEventSource.last()
    es._open()
    fireAgentEvent(es, 1, { v: 'a' })
    fireAgentEvent(es, 2, { v: 'b' })
    fireAgentEvent(es, 2, { v: 'b-duplicate' })
    fireAgentEvent(es, 1, { v: 'a-duplicate' })
    fireAgentEvent(es, 3, { v: 'c' })

    expect(h.events.map((e) => e.seq)).toEqual([1, 2, 3])
  })

  it('detects gaps after reconnect (missing seq between last-seen and first-new)', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es1 = MockEventSource.last()
    es1._open()
    fireAgentEvent(es1, 1, {})
    fireAgentEvent(es1, 2, {})
    es1._error()
    vi.advanceTimersByTime(10)

    const es2 = MockEventSource.last()
    es2._open()
    // Server skipped seq 3 and 4 — this is a gap.
    fireAgentEvent(es2, 5, {})

    expect(h.gaps).toEqual([{ expected: 3, actual: 5 }])
    // Event is still forwarded so caller can decide whether to render.
    expect(h.events[h.events.length - 1].seq).toBe(5)
  })

  it('emits no gap when reconnect delivers the next expected seq', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es1 = MockEventSource.last()
    es1._open()
    fireAgentEvent(es1, 1, {})
    fireAgentEvent(es1, 2, {})
    es1._error()
    vi.advanceTimersByTime(10)

    const es2 = MockEventSource.last()
    es2._open()
    fireAgentEvent(es2, 3, {})
    fireAgentEvent(es2, 4, {})

    expect(h.gaps).toEqual([])
  })

  it('invokes onDone on the `done` event and closes', () => {
    const h = makeHarness()
    const handle = openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es = MockEventSource.last()
    es._open()
    es._fireNamed('done', '{"status":"completed"}')
    expect(h.doneCount).toBe(1)
    expect(es.closed).toBe(true)
    expect(handle.state()).toBe('disconnected')
  })

  it('handle.close() stops reconnect attempts', () => {
    const h = makeHarness()
    const handle = openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es = MockEventSource.last()
    es._open()
    es._error()

    handle.close()
    vi.advanceTimersByTime(1000)
    expect(MockEventSource.instances).toHaveLength(1)
    expect(handle.state()).toBe('disconnected')
  })

  it('preserves existing query string when appending last_event_id', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e?workspace=ws1',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es1 = MockEventSource.last()
    es1._open()
    fireAgentEvent(es1, 7, {})
    es1._error()
    vi.advanceTimersByTime(10)

    const es2 = MockEventSource.last()
    expect(es2.url).toBe('https://api.example.com/e?workspace=ws1&last_event_id=7')
  })

  it('does not reconnect after a `done` event (stream terminated cleanly)', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        initialBackoffMs: 10,
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es = MockEventSource.last()
    es._open()
    es._fireNamed('done', '{}')
    // A native EventSource implementation may fire an error on close —
    // we must not reopen.
    es._error()
    vi.advanceTimersByTime(1000)
    expect(MockEventSource.instances).toHaveLength(1)
  })

  it('forwards malformed JSON as null data (non-throwing)', () => {
    const h = makeHarness()
    openReconnectingEventSource(
      {
        url: 'https://api.example.com/e',
        eventName: 'agent_event',
        EventSourceCtor: MockEventSource as unknown as typeof EventSource,
      },
      h.callbacks,
    )

    const es = MockEventSource.last()
    es._open()
    es._fireNamed('agent_event', 'not-json', '1')

    // Should not throw; harness should see nothing added.
    expect(h.events).toHaveLength(0)
  })
})
