// Reconnecting EventSource client with cursor-based backfill (RUSA-257).
//
// Wraps the browser `EventSource` API to add:
//
//   * Exponential backoff on error (configurable, capped)
//   * Reconnect via `?last_event_id=<seq>` query param so the server can
//     backfill events missed during the outage. We cannot set the
//     `Last-Event-ID` header because `EventSource` has no header-setting API;
//     the backend (`GET /executions/:id/events`) therefore accepts the cursor
//     via query string as a fallback to the `Last-Event-ID` header.
//   * De-duplication by seq so overlapping backfill + live events are a no-op
//     for the caller.
//   * Gap detection: if the first seq after a reconnect skips past
//     `lastSeq + 1`, emit `onGap(expected, actual)` so the UI can warn.
//   * Explicit connection state (`connecting | connected | reconnecting |
//     disconnected`) so the UI can render transport health.
//
// The module is transport-agnostic about payload shape: JSON parsing happens
// here (so the caller gets a typed value), but malformed events are dropped
// silently — MessagePart validation lives in `parseAgentEvent` downstream.

export type ConnectionState =
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'disconnected'

export interface ReconnectingClientCallbacks {
  /** Fired for each forwarded event. `seq` is the numeric cursor (or null). */
  onEvent: (data: unknown, seq: number | null) => void
  /** Fired on every connection-state change. */
  onStateChange: (state: ConnectionState) => void
  /** Fired when the first seq after a reconnect is greater than `expected`. */
  onGap: (expected: number, actual: number) => void
  /** Fired once when the server emits the terminal `done` event. */
  onDone: () => void
}

export interface ReconnectingEventSourceOptions {
  /** Base URL for the SSE endpoint. */
  url: string
  /** Named SSE event to subscribe to (e.g. `agent_event`). */
  eventName: string
  /** Initial backoff in ms (default 1000). Doubles on each attempt. */
  initialBackoffMs?: number
  /** Upper bound on backoff in ms (default 30000). */
  maxBackoffMs?: number
  /**
   * Maximum number of reconnect attempts after the initial connection
   * (default 6). After this, state transitions to `disconnected`.
   */
  maxAttempts?: number
  /**
   * Name of the cursor query-string parameter (default `last_event_id`).
   * Kept configurable for endpoints that already use `last_event_id` for
   * something else.
   */
  cursorParam?: string
  /** EventSource constructor override (tests inject a mock). */
  EventSourceCtor?: typeof EventSource
}

export interface ReconnectingEventSourceHandle {
  close(): void
  state(): ConnectionState
}

const DEFAULTS = {
  initialBackoffMs: 1000,
  maxBackoffMs: 30_000,
  maxAttempts: 6,
  cursorParam: 'last_event_id',
}

function appendCursor(
  url: string,
  param: string,
  lastSeq: number,
): string {
  if (lastSeq <= 0) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}${param}=${lastSeq}`
}

function parseSeq(lastEventId: string): number | null {
  if (!lastEventId) return null
  const n = Number(lastEventId)
  return Number.isFinite(n) ? n : null
}

export function openReconnectingEventSource(
  options: ReconnectingEventSourceOptions,
  callbacks: ReconnectingClientCallbacks,
): ReconnectingEventSourceHandle {
  const initialBackoffMs = options.initialBackoffMs ?? DEFAULTS.initialBackoffMs
  const maxBackoffMs = options.maxBackoffMs ?? DEFAULTS.maxBackoffMs
  const maxAttempts = options.maxAttempts ?? DEFAULTS.maxAttempts
  const cursorParam = options.cursorParam ?? DEFAULTS.cursorParam
  const EventSourceCtor = options.EventSourceCtor ?? EventSource

  let state: ConnectionState | null = null
  let lastSeq = 0
  let attempts = 0
  let retryHandle: ReturnType<typeof setTimeout> | null = null
  let current: EventSource | null = null
  let closedByCaller = false
  let terminatedByServer = false

  // Seqs already forwarded — used for dedup across reconnects.
  const seen = new Set<number>()

  function setState(next: ConnectionState): void {
    if (state === next) return
    state = next
    callbacks.onStateChange(next)
  }

  function detach(es: EventSource | null): void {
    if (!es) return
    try {
      es.close()
    } catch {
      // closing a mock may throw; ignore
    }
  }

  function scheduleReconnect(): void {
    if (closedByCaller || terminatedByServer) return
    if (attempts >= maxAttempts) {
      setState('disconnected')
      return
    }
    // exponential: 1x, 2x, 4x, 8x, ... capped at maxBackoffMs
    const delay = Math.min(initialBackoffMs * Math.pow(2, attempts), maxBackoffMs)
    attempts += 1
    setState('reconnecting')
    retryHandle = setTimeout(() => {
      retryHandle = null
      connect()
    }, delay)
  }

  function connect(): void {
    if (closedByCaller || terminatedByServer) return
    const url = appendCursor(options.url, cursorParam, lastSeq)
    setState('connecting')
    const es = new EventSourceCtor(url)
    current = es

    es.onopen = () => {
      if (closedByCaller || terminatedByServer) return
      // Note: we deliberately do NOT reset `attempts` here. An open that is
      // immediately followed by an error has not actually recovered the
      // stream, so the backoff must keep escalating. `attempts` is reset only
      // after we successfully deliver an event (see `handleMessage`).
      setState('connected')
    }

    const handleMessage = (e: MessageEvent) => {
      if (closedByCaller || terminatedByServer) return
      const seq = parseSeq(e.lastEventId)

      if (seq !== null) {
        if (seen.has(seq)) return // dedup
        if (lastSeq > 0 && seq > lastSeq + 1) {
          callbacks.onGap(lastSeq + 1, seq)
        }
        seen.add(seq)
        lastSeq = Math.max(lastSeq, seq)
      }

      let parsed: unknown
      try {
        parsed = JSON.parse(e.data)
      } catch {
        return // malformed frame — drop (see RECONCILIATION.md R-4 handled by parseAgentEvent upstream)
      }
      // Genuine progress: clear the backoff so transient blips don't keep
      // escalating after the stream has actually recovered.
      attempts = 0
      callbacks.onEvent(parsed, seq)
    }

    es.addEventListener(options.eventName, handleMessage as EventListener)
    // Generic onmessage acts as a fallback for un-named `message` events.
    es.onmessage = handleMessage

    es.addEventListener('done', ((_e: MessageEvent) => {
      if (closedByCaller || terminatedByServer) return
      terminatedByServer = true
      detach(current)
      current = null
      setState('disconnected')
      callbacks.onDone()
    }) as EventListener)

    es.onerror = () => {
      if (closedByCaller || terminatedByServer) return
      detach(current)
      current = null
      scheduleReconnect()
    }
  }

  connect()

  return {
    close(): void {
      if (closedByCaller) return
      closedByCaller = true
      if (retryHandle !== null) {
        clearTimeout(retryHandle)
        retryHandle = null
      }
      detach(current)
      current = null
      setState('disconnected')
    },
    state(): ConnectionState {
      return state ?? 'connecting'
    },
  }
}
