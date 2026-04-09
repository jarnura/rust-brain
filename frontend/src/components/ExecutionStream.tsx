import { useEffect, useMemo, useRef } from 'react'
import { API_BASE, openExecutionStream } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { AgentEvent } from '../types'

// ─── Agent display config ────────────────────────────────────────────────────

interface AgentDisplay {
  label: string
  color: string
  bgActive: string
  bgInactive: string
  textActive: string
  textInactive: string
}

const AGENT_DISPLAY: Record<string, AgentDisplay> = {
  explorer:   { label: 'Explorer',   color: 'cyan',    bgActive: 'bg-cyan-600',    bgInactive: 'bg-cyan-900/40',    textActive: 'text-white',      textInactive: 'text-cyan-400/60' },
  developer:  { label: 'Developer',  color: 'green',   bgActive: 'bg-green-600',   bgInactive: 'bg-green-900/40',   textActive: 'text-white',      textInactive: 'text-green-400/60' },
  reviewer:   { label: 'Reviewer',   color: 'purple',  bgActive: 'bg-purple-600',  bgInactive: 'bg-purple-900/40',  textActive: 'text-white',      textInactive: 'text-purple-400/60' },
  planner:    { label: 'Planner',    color: 'blue',    bgActive: 'bg-blue-600',    bgInactive: 'bg-blue-900/40',    textActive: 'text-white',      textInactive: 'text-blue-400/60' },
  tester:     { label: 'Tester',     color: 'yellow',  bgActive: 'bg-yellow-600',  bgInactive: 'bg-yellow-900/40',  textActive: 'text-white',      textInactive: 'text-yellow-400/60' },
  debugger:   { label: 'Debugger',   color: 'red',     bgActive: 'bg-red-600',     bgInactive: 'bg-red-900/40',     textActive: 'text-white',      textInactive: 'text-red-400/60' },
  refactorer: { label: 'Refactorer', color: 'orange',  bgActive: 'bg-orange-600',  bgInactive: 'bg-orange-900/40',  textActive: 'text-white',      textInactive: 'text-orange-400/60' },
  documenter: { label: 'Documenter', color: 'teal',    bgActive: 'bg-teal-600',    bgInactive: 'bg-teal-900/40',    textActive: 'text-white',      textInactive: 'text-teal-400/60' },
  architect:  { label: 'Architect',  color: 'indigo',  bgActive: 'bg-indigo-600',  bgInactive: 'bg-indigo-900/40',  textActive: 'text-white',      textInactive: 'text-indigo-400/60' },
  security:   { label: 'Security',   color: 'rose',    bgActive: 'bg-rose-600',    bgInactive: 'bg-rose-900/40',    textActive: 'text-white',      textInactive: 'text-rose-400/60' },
  optimizer:  { label: 'Optimizer',  color: 'amber',   bgActive: 'bg-amber-600',   bgInactive: 'bg-amber-900/40',   textActive: 'text-white',      textInactive: 'text-amber-400/60' },
  integrator: { label: 'Integrator', color: 'lime',    bgActive: 'bg-lime-600',    bgInactive: 'bg-lime-900/40',    textActive: 'text-white',      textInactive: 'text-lime-400/60' },
}

const DEFAULT_AGENT_DISPLAY: AgentDisplay = {
  label: 'Agent',
  color: 'slate',
  bgActive: 'bg-slate-600',
  bgInactive: 'bg-slate-900/40',
  textActive: 'text-white',
  textInactive: 'text-slate-400/60',
}

function getAgentDisplay(agentKey: string): AgentDisplay {
  const known = AGENT_DISPLAY[agentKey]
  if (known) return known
  return { ...DEFAULT_AGENT_DISPLAY, label: agentKey.charAt(0).toUpperCase() + agentKey.slice(1) }
}

// ─── Legacy phase-to-agent mapping (backward compat) ─────────────────────────

const PHASE_TO_AGENT: Record<string, string> = {
  orchestrating: 'planner',
  researching: 'explorer',
  planning: 'planner',
  developing: 'developer',
}

// ─── Event badge styling ─────────────────────────────────────────────────────

const PHASE_STYLES: Record<string, { bg: string; text: string; label: string }> = {
  init:           { bg: 'bg-dark-700',       text: 'text-dark-300',    label: 'INIT' },
  planning:       { bg: 'bg-purple-900',     text: 'text-purple-300',  label: 'PLANNING' },
  reasoning:      { bg: 'bg-blue-900',       text: 'text-blue-300',    label: 'REASONING' },
  phase_change:   { bg: 'bg-indigo-900',     text: 'text-indigo-300',  label: 'PHASE' },
  agent_dispatch: { bg: 'bg-cyan-900',       text: 'text-cyan-300',    label: 'AGENT' },
  tool_call:      { bg: 'bg-yellow-900',     text: 'text-yellow-300',  label: 'TOOL' },
  tool_result:    { bg: 'bg-yellow-900/60',  text: 'text-yellow-200',  label: 'RESULT' },
  file_edit:      { bg: 'bg-green-900',      text: 'text-green-300',   label: 'FILE' },
  done:           { bg: 'bg-green-800',      text: 'text-green-200',   label: 'DONE' },
  error:          { bg: 'bg-red-900',        text: 'text-red-300',     label: 'ERROR' },
}

function phaseBadge(eventType: string) {
  const style = PHASE_STYLES[eventType] ?? PHASE_STYLES.init
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
    if ('agent' in c && typeof c.agent === 'string' && event.event_type === 'agent_dispatch') {
      return `Dispatching ${getAgentDisplay(c.agent).label}`
    }
    if ('tool' in c && typeof c.tool === 'string') return `${c.tool}(...)`
    if ('path' in c && typeof c.path === 'string') return c.path
    if ('summary' in c && typeof c.summary === 'string') return c.summary
    if ('phase' in c && typeof c.phase === 'string') return c.phase
    return JSON.stringify(c)
  }
  return String(c)
}

// ─── Agent source badge (shows which agent produced the event) ───────────────

function agentSourceBadge(event: AgentEvent) {
  const c = event.content
  if (typeof c === 'object' && c !== null && 'agent' in c && typeof c.agent === 'string') {
    const display = getAgentDisplay(c.agent)
    return (
      <span className={`inline-block px-1 py-0.5 rounded text-[9px] font-medium ${display.bgInactive} ${display.textInactive} shrink-0`}>
        {display.label}
      </span>
    )
  }
  return null
}

// ─── Agent Timeline ──────────────────────────────────────────────────────────

interface TimelineEntry {
  agentKey: string
  dispatchedAt: string
}

function deriveTimeline(events: readonly AgentEvent[]): { entries: TimelineEntry[]; activeAgent: string | null; usesLegacyPhases: boolean } {
  const entries: TimelineEntry[] = []
  const seen = new Set<string>()
  let activeAgent: string | null = null
  let hasAgentDispatch = false

  for (const event of events) {
    const c = event.content
    if (typeof c !== 'object' || c === null) continue

    if (event.event_type === 'agent_dispatch' && 'agent' in c && typeof c.agent === 'string') {
      hasAgentDispatch = true
      if (!seen.has(c.agent)) {
        seen.add(c.agent)
        entries.push({ agentKey: c.agent, dispatchedAt: event.timestamp })
      }
      activeAgent = c.agent
    } else if (event.event_type === 'phase_change' && 'phase' in c && typeof c.phase === 'string') {
      const mappedAgent = PHASE_TO_AGENT[c.phase]
      if (mappedAgent && !seen.has(mappedAgent)) {
        seen.add(mappedAgent)
        entries.push({ agentKey: mappedAgent, dispatchedAt: event.timestamp })
      }
      if (mappedAgent) activeAgent = mappedAgent
    }

    if (event.event_type === 'done' || event.event_type === 'error') {
      activeAgent = null
    }
  }

  return { entries, activeAgent, usesLegacyPhases: !hasAgentDispatch && entries.length > 0 }
}

interface AgentTimelineProps {
  entries: TimelineEntry[]
  activeAgent: string | null
  streamDone: boolean
  usesLegacyPhases: boolean
}

function AgentTimeline({ entries, activeAgent, streamDone, usesLegacyPhases }: AgentTimelineProps) {
  if (entries.length === 0) return null

  return (
    <div className="mb-3">
      {usesLegacyPhases && (
        <p className="text-[10px] text-dark-500 mb-1 italic">Legacy phase mode</p>
      )}
      <div className="flex items-center gap-1.5 flex-wrap">
        {entries.map((entry, idx) => {
          const display = getAgentDisplay(entry.agentKey)
          const isActive = !streamDone && activeAgent === entry.agentKey
          const isPast = activeAgent !== entry.agentKey || streamDone
          const bg = isActive ? display.bgActive : display.bgInactive
          const text = isActive ? display.textActive : display.textInactive

          return (
            <div key={entry.agentKey} className="flex items-center gap-1.5">
              {idx > 0 && (
                <div className="w-3 h-px bg-dark-600" />
              )}
              <span
                className={`inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-semibold transition-all duration-300 ${bg} ${text}`}
              >
                {isActive && (
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-white animate-pulse" />
                )}
                {isPast && !isActive && (
                  <span className="inline-block w-1.5 h-1.5 rounded-full bg-current opacity-40" />
                )}
                {display.label}
              </span>
            </div>
          )
        })}
        {!streamDone && activeAgent === null && entries.length > 0 && (
          <span className="text-[10px] text-dark-500 italic ml-1">Idle</span>
        )}
      </div>
    </div>
  )
}

// ─── Component ───────────────────────────────────────────────────────────────

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

  // Derive agent timeline from events
  const { entries, activeAgent, usesLegacyPhases } = useMemo(
    () => deriveTimeline(streamEvents),
    [streamEvents],
  )

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
        fetch(`${API_BASE}/executions/${activeExecutionId}`)
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
          Execution <code className="text-brand-400 font-mono text-[10px]">{activeExecutionId.slice(0, 8)}...</code>
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

      {/* Agent timeline */}
      <AgentTimeline
        entries={entries}
        activeAgent={activeAgent}
        streamDone={streamDone}
        usesLegacyPhases={usesLegacyPhases}
      />

      {/* Fallback: no agents dispatched yet and stream is live */}
      {entries.length === 0 && !streamDone && streamEvents.length > 0 && (
        <div className="mb-3">
          <span className="inline-flex items-center gap-1 px-2 py-1 rounded-md text-[11px] font-semibold bg-dark-700 text-dark-300">
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-dark-400 animate-pulse" />
            Processing
          </span>
        </div>
      )}

      <div className="flex-1 overflow-y-auto space-y-1 font-mono text-xs">
        {streamEvents.map((event) => (
          <div key={event.id} className="flex items-start gap-2 py-0.5">
            {phaseBadge(event.event_type)}
            {agentSourceBadge(event)}
            <span className="text-dark-300 shrink-0 tabular-nums">
              {new Date(event.timestamp).toLocaleTimeString('en-US', { hour12: false })}
            </span>
            <span className="text-dark-200 break-all">{eventSummary(event)}</span>
          </div>
        ))}
        {streamEvents.length === 0 && !streamDone && (
          <p className="text-dark-500 italic animate-pulse">Waiting for events...</p>
        )}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
