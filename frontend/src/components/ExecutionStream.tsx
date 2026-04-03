import { useEffect, useRef } from 'react'
import { openExecutionStream } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { AgentEvent, EventPhase } from '../types'

// ─── Phase styling ────────────────────────────────────────────────────────────

const PHASE_STYLES: Record<string, { bg: string; text: string; label: string }> = {
  init: { bg: 'bg-dark-700', text: 'text-dark-300', label: 'INIT' },
  planning: { bg: 'bg-purple-900', text: 'text-purple-300', label: 'PLANNING' },
  reasoning: { bg: 'bg-blue-900', text: 'text-blue-300', label: 'REASONING' },
  tool_call: { bg: 'bg-yellow-900', text: 'text-yellow-300', label: 'TOOL' },
  tool_result: { bg: 'bg-yellow-900/60', text: 'text-yellow-200', label: 'RESULT' },
  file_edit: { bg: 'bg-green-900', text: 'text-green-300', label: 'FILE' },
  done: { bg: 'bg-green-800', text: 'text-green-200', label: 'DONE' },
  error: { bg: 'bg-red-900', text: 'text-red-300', label: 'ERROR' },
}

function phaseBadge(phase: EventPhase | null) {
  const style = (phase ? PHASE_STYLES[phase] : null) ?? PHASE_STYLES.init
  return (
    <span className={`inline-block px-1.5 py-0.5 rounded text-[10px] font-bold tracking-wide ${style.bg} ${style.text} shrink-0`}>
      {style.label}
    </span>
  )
}

function eventSummary(event: AgentEvent): string {
  const c = event.content
  if (typeof c === 'object' && c !== null) {
    if ('text' in c && typeof c.text === 'string') return c.text
    if ('message' in c && typeof c.message === 'string') return c.message
    if ('tool' in c && typeof c.tool === 'string') return `${c.tool}(…)`
    if ('path' in c && typeof c.path === 'string') return c.path
    if ('summary' in c && typeof c.summary === 'string') return c.summary
    return JSON.stringify(c)
  }
  return String(c)
}

// ─── Component ────────────────────────────────────────────────────────────────

export function ExecutionStream() {
  const {
    activeExecutionId,
    streamEvents,
    streamDone,
    appendStreamEvent,
    setStreamDone,
    upsertExecution,
  } = useWorkspaceStore()

  const bottomRef = useRef<HTMLDivElement>(null)

  // Auto-scroll to bottom on new events
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [streamEvents.length])

  // Open SSE stream when executionId changes
  useEffect(() => {
    if (!activeExecutionId) return

    const cleanup = openExecutionStream(
      activeExecutionId,
      (raw) => {
        const event = raw as AgentEvent
        appendStreamEvent(event)
      },
      (_err) => {
        setStreamDone(true)
      },
      () => {
        setStreamDone(true)
        // Refresh execution status
        fetch(
          `${import.meta.env.VITE_API_BASE_URL ?? `${window.location.protocol}//${window.location.hostname}:8088`}/executions/${activeExecutionId}`,
        )
          .then((r) => r.json())
          .then((exec) => upsertExecution(exec as Parameters<typeof upsertExecution>[0]))
          .catch(() => {})
      },
    )

    return cleanup
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeExecutionId])

  if (!activeExecutionId) {
    return (
      <p className="text-dark-500 text-xs italic p-2">
        Submit a prompt to see live execution events.
      </p>
    )
  }

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs text-dark-400 font-medium">
          Execution <code className="text-brand-400 font-mono text-[10px]">{activeExecutionId.slice(0, 8)}…</code>
        </span>
        {!streamDone && (
          <span className="flex items-center gap-1 text-xs text-green-400">
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
            Live
          </span>
        )}
        {streamDone && (
          <span className="text-xs text-dark-500">Stream closed</span>
        )}
      </div>

      <div className="flex-1 overflow-y-auto space-y-1 font-mono text-xs">
        {streamEvents.map((event) => (
          <div key={event.id} className="flex items-start gap-2 py-0.5">
            {phaseBadge(event.phase)}
            <span className="text-dark-300 shrink-0 tabular-nums">
              {new Date(event.ts).toLocaleTimeString('en-US', { hour12: false })}
            </span>
            <span className="text-dark-200 break-all">{eventSummary(event)}</span>
          </div>
        ))}
        {streamEvents.length === 0 && !streamDone && (
          <p className="text-dark-500 italic animate-pulse">Waiting for events…</p>
        )}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
