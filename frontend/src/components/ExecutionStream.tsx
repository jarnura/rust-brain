import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { API_BASE, openExecutionStream } from '../api/client'
import type { ConnectionState } from '../api/sse-reconnect'
import { useWorkspaceStore } from '../store/workspace'
import type { AgentEvent } from '../types'
import { parseAgentEvent } from '../lib/event-parser'
import {
  isReasoningEvent,
  isToolCallEvent,
  type TypedAgentEvent,
} from '../types/events'
import {
  buildNavUnits,
  eventMatchesQuery,
  groupConsecutiveEvents,
  isNearBottom,
  nextNavIndex,
  prevNavIndex,
  summarizeGroup,
  type NavUnit,
  type TranscriptGroup,
} from '../lib/collapsible-grouping'
import { ToolCallCard } from './ToolCallCard'
import { ReasoningCard } from './ReasoningCard'

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

function eventSummary(event: AgentEvent | TypedAgentEvent): string {
  const c = event.content as Record<string, unknown>
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

function agentSourceBadge(event: AgentEvent | TypedAgentEvent) {
  const c = event.content as Record<string, unknown>
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

// ─── Generic event row (non-tool, non-reasoning) ─────────────────────────────

interface EventRowProps {
  event: TypedAgentEvent
  highlighted: boolean
  focused: boolean
}

function EventRow({ event, highlighted, focused }: EventRowProps) {
  const ringClass = focused
    ? 'ring-1 ring-brand-400/70 rounded'
    : highlighted
      ? 'ring-1 ring-yellow-400/60 rounded'
      : ''
  return (
    <div className={`flex items-start gap-2 py-0.5 px-1 ${ringClass}`}>
      {phaseBadge(event.event_type)}
      {agentSourceBadge(event)}
      <span className="text-dark-300 shrink-0 tabular-nums">
        {new Date(event.timestamp).toLocaleTimeString('en-US', { hour12: false })}
      </span>
      <span className="text-dark-200 break-all">{eventSummary(event)}</span>
    </div>
  )
}

// ─── Collapsed group header ──────────────────────────────────────────────────

interface GroupHeaderProps {
  group: TranscriptGroup
  onExpand: () => void
  focused: boolean
}

function CollapsedGroupHeader({ group, onExpand, focused }: GroupHeaderProps) {
  const summary = summarizeGroup(group)
  const ringClass = focused ? 'ring-1 ring-brand-400/70' : ''
  return (
    <button
      type="button"
      onClick={onExpand}
      aria-expanded={false}
      className={`w-full flex items-center gap-2 px-2 py-1 text-left text-[11px] text-dark-300 border border-dashed border-dark-700 rounded hover:bg-dark-800/60 ${ringClass}`}
    >
      <span className="text-dark-500">▶</span>
      <span className="font-medium">{summary}</span>
      <span className="ml-auto text-[10px] text-dark-500">expand</span>
    </button>
  )
}

// ─── Connection status indicator ─────────────────────────────────────────────

interface ConnectionStatusProps {
  state: ConnectionState
  streamDone: boolean
}

function ConnectionStatus({ state, streamDone }: ConnectionStatusProps) {
  // Stream completed cleanly via `done` event — show terminal state, not a
  // warning, even though the underlying transport is now `disconnected`.
  if (streamDone) {
    return <span className="text-xs text-dark-500">Stream closed</span>
  }

  switch (state) {
    case 'connected':
      return (
        <span className="flex items-center gap-1 text-xs text-green-400">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
          Connected
        </span>
      )
    case 'connecting':
      return (
        <span className="flex items-center gap-1 text-xs text-dark-400">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-dark-400 animate-pulse" />
          Connecting…
        </span>
      )
    case 'reconnecting':
      return (
        <span className="flex items-center gap-1 text-xs text-yellow-400">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-yellow-400 animate-pulse" />
          Reconnecting…
        </span>
      )
    case 'disconnected':
      return (
        <span className="flex items-center gap-1 text-xs text-red-400">
          <span className="inline-block w-1.5 h-1.5 rounded-full bg-red-400" />
          Disconnected
        </span>
      )
  }
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
    streamConnectionState,
    setStreamConnectionState,
    streamGapCount,
    recordStreamGap,
    clearStreamGap,
  } = useWorkspaceStore()

  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const bottomRef = useRef<HTMLDivElement>(null)

  // ─── State: collapse, focus, autoscroll, filter ────────────────────────────

  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set())
  const [expandedUnits, setExpandedUnits] = useState<Set<string>>(() => new Set())
  const [focusedIndex, setFocusedIndex] = useState(0)
  const [autoScroll, setAutoScroll] = useState(true)
  const [filter, setFilter] = useState('')

  // Typed events (memoized so parsing doesn't thrash under frequent renders).
  const typedEvents = useMemo(() => streamEvents.map(parseAgentEvent), [streamEvents])

  // Derive agent timeline from events
  const { entries, activeAgent, usesLegacyPhases } = useMemo(
    () => deriveTimeline(streamEvents),
    [streamEvents],
  )

  // Group consecutive same-kind events
  const groups = useMemo(() => groupConsecutiveEvents(typedEvents), [typedEvents])

  // Flat navigable units (respects collapsed groups)
  const navUnits: NavUnit[] = useMemo(
    () => buildNavUnits(groups, collapsedGroups),
    [groups, collapsedGroups],
  )

  // ─── Handlers ──────────────────────────────────────────────────────────────

  const toggleUnit = useCallback((unitId: string) => {
    setExpandedUnits((prev) => {
      const next = new Set(prev)
      if (next.has(unitId)) next.delete(unitId)
      else next.add(unitId)
      return next
    })
  }, [])

  const toggleGroupCollapse = useCallback((groupId: string) => {
    setCollapsedGroups((prev) => {
      const next = new Set(prev)
      if (next.has(groupId)) next.delete(groupId)
      else next.add(groupId)
      return next
    })
  }, [])

  const expandAll = useCallback(() => {
    setCollapsedGroups(new Set())
    const allUnitIds = new Set<string>()
    for (const group of groups) {
      for (const event of group.events) {
        allUnitIds.add(`${group.id}:${event.id}`)
      }
    }
    setExpandedUnits(allUnitIds)
  }, [groups])

  const collapseAll = useCallback(() => {
    setExpandedUnits(new Set())
    setCollapsedGroups(new Set(groups.map((g) => g.id)))
  }, [groups])

  const toggleFocused = useCallback(() => {
    const unit = navUnits[focusedIndex]
    if (!unit) return
    if (unit.kind === 'group-header') {
      toggleGroupCollapse(unit.groupId)
    } else {
      toggleUnit(unit.id)
    }
  }, [focusedIndex, navUnits, toggleGroupCollapse, toggleUnit])

  // Keyboard shortcuts (J/K next/prev, Enter toggle, Ctrl+Shift+C collapse all)
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      // Don't hijack keys while typing in form inputs.
      const target = e.target as HTMLElement | null
      if (target) {
        const tag = target.tagName
        if (tag === 'INPUT' || tag === 'TEXTAREA' || target.isContentEditable) {
          return
        }
      }

      if (e.ctrlKey && e.shiftKey && (e.key === 'C' || e.key === 'c')) {
        e.preventDefault()
        collapseAll()
        return
      }
      if (e.key === 'j' || e.key === 'J') {
        e.preventDefault()
        setFocusedIndex((idx) => nextNavIndex(idx, navUnits.length))
      } else if (e.key === 'k' || e.key === 'K') {
        e.preventDefault()
        setFocusedIndex((idx) => prevNavIndex(idx, navUnits.length))
      } else if (e.key === 'Enter') {
        e.preventDefault()
        toggleFocused()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [collapseAll, navUnits.length, toggleFocused])

  // Clamp focusedIndex when navUnits shrinks.
  useEffect(() => {
    if (focusedIndex >= navUnits.length && navUnits.length > 0) {
      setFocusedIndex(navUnits.length - 1)
    }
  }, [focusedIndex, navUnits.length])

  // Auto-scroll when new events arrive (if enabled).
  useEffect(() => {
    if (!autoScroll) return
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [autoScroll, streamEvents.length])

  // Scroll handler: pause auto-scroll when user scrolls up, resume at bottom.
  const onScroll = useCallback(() => {
    const el = scrollContainerRef.current
    if (!el) return
    const atBottom = isNearBottom(el.scrollTop, el.scrollHeight, el.clientHeight)
    setAutoScroll(atBottom)
  }, [])

  // Open SSE stream when executionId changes
  useEffect(() => {
    if (!activeExecutionId) return

    const cleanup = openExecutionStream(activeExecutionId, {
      onEvent: (raw) => {
        const event = raw as AgentEvent
        appendStreamEvent(event)
      },
      onStateChange: (state) => {
        setStreamConnectionState(state)
      },
      onGap: () => {
        recordStreamGap()
      },
      onDone: () => {
        setStreamDone(true)
        fetch(`${API_BASE}/executions/${activeExecutionId}`)
          .then((r) => r.json())
          .then((exec) => upsertExecution(exec as Parameters<typeof upsertExecution>[0]))
          .catch(() => {})
      },
    })

    return cleanup
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeExecutionId])

  // Reset state when a new execution begins.
  useEffect(() => {
    setCollapsedGroups(new Set())
    setExpandedUnits(new Set())
    setFocusedIndex(0)
    setAutoScroll(true)
    clearStreamGap()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeExecutionId])

  if (!activeExecutionId) {
    return (
      <p className="text-dark-500 text-xs italic p-2">
        Submit a prompt to see live execution events.
      </p>
    )
  }

  const filterActive = filter.trim().length > 0
  const matchCount = filterActive
    ? typedEvents.reduce((n, ev) => (eventMatchesQuery(ev, filter) ? n + 1 : n), 0)
    : 0

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between mb-2">
        <span className="text-xs text-dark-400 font-medium">
          Execution <code className="text-brand-400 font-mono text-[10px]">{activeExecutionId.slice(0, 8)}...</code>
        </span>
        <ConnectionStatus
          state={streamConnectionState}
          streamDone={streamDone}
        />
      </div>

      {streamGapCount > 0 && (
        <div
          role="alert"
          className="mb-2 px-2 py-1 text-[11px] rounded border border-yellow-700/60 bg-yellow-900/30 text-yellow-300 flex items-center justify-between"
        >
          <span>
            ⚠ {streamGapCount} event{streamGapCount === 1 ? '' : 's'} may be
            missing — server buffer overflowed during reconnect.
          </span>
          <button
            type="button"
            onClick={clearStreamGap}
            className="text-[10px] px-1 text-yellow-200 hover:text-yellow-100"
          >
            dismiss
          </button>
        </div>
      )}

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

      {/* Transcript toolbar */}
      <div className="flex flex-wrap items-center gap-2 mb-2 text-[11px]">
        <button
          type="button"
          onClick={expandAll}
          className="px-2 py-0.5 rounded border border-dark-700 text-dark-300 hover:bg-dark-800"
          title="Expand all events"
        >
          Expand all
        </button>
        <button
          type="button"
          onClick={collapseAll}
          className="px-2 py-0.5 rounded border border-dark-700 text-dark-300 hover:bg-dark-800"
          title="Collapse all (Ctrl+Shift+C)"
        >
          Collapse all
        </button>
        <input
          type="text"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter…"
          aria-label="Filter events"
          className="flex-1 min-w-[8rem] px-2 py-0.5 rounded border border-dark-700 bg-dark-900/60 text-dark-200 placeholder-dark-500 focus:outline-none focus:border-brand-500"
        />
        {filterActive && (
          <span className="text-dark-400 tabular-nums">
            {matchCount} match{matchCount === 1 ? '' : 'es'}
          </span>
        )}
        <span
          className={`px-2 py-0.5 rounded ${autoScroll ? 'text-green-400' : 'text-dark-500'}`}
          title={
            autoScroll
              ? 'Auto-scroll on (at bottom)'
              : 'Auto-scroll paused (user scrolled up)'
          }
        >
          {autoScroll ? '⇣ auto' : '⏸ paused'}
        </span>
        <span className="text-[10px] text-dark-500">J/K nav · Enter toggle</span>
      </div>

      <div
        ref={scrollContainerRef}
        onScroll={onScroll}
        className="flex-1 overflow-y-auto space-y-1 font-mono text-xs"
      >
        {groups.map((group, groupIdx) => {
          const isGroupCollapsed = collapsedGroups.has(group.id)
          const headerUnitId = `${group.id}:hdr`
          const headerFocused =
            isGroupCollapsed &&
            navUnits[focusedIndex]?.kind === 'group-header' &&
            navUnits[focusedIndex]?.groupId === group.id

          if (isGroupCollapsed) {
            return (
              <div key={headerUnitId} data-group-id={group.id}>
                <CollapsedGroupHeader
                  group={group}
                  onExpand={() => toggleGroupCollapse(group.id)}
                  focused={headerFocused}
                />
              </div>
            )
          }

          const showGroupDivider = group.events.length > 1

          return (
            <div key={group.id} data-group-id={group.id}>
              {showGroupDivider && (
                <div className="flex items-center gap-2 mt-1 mb-0.5 text-[10px] text-dark-500">
                  <button
                    type="button"
                    onClick={() => toggleGroupCollapse(group.id)}
                    className="flex items-center gap-1 hover:text-dark-300"
                    title="Collapse group"
                  >
                    <span>▼</span>
                    <span>{summarizeGroup(group)}</span>
                  </button>
                  <div className="flex-1 h-px bg-dark-800" />
                </div>
              )}
              {group.events.map((event, eventIdx) => {
                const unitId = `${group.id}:${event.id}`
                const unitExpanded = expandedUnits.has(unitId)
                const highlighted = filterActive && eventMatchesQuery(event, filter)
                const navUnit = navUnits[focusedIndex]
                const unitFocused =
                  navUnit?.kind === 'event' &&
                  navUnit.groupIndex === groupIdx &&
                  navUnit.eventIndex === eventIdx

                if (isToolCallEvent(event)) {
                  return (
                    <ToolCallCard
                      key={event.id}
                      event={event}
                      expanded={unitExpanded}
                      onToggle={() => toggleUnit(unitId)}
                      highlighted={highlighted}
                      focused={unitFocused}
                    />
                  )
                }
                if (isReasoningEvent(event)) {
                  return (
                    <ReasoningCard
                      key={event.id}
                      event={event}
                      expanded={unitExpanded}
                      onToggle={() => toggleUnit(unitId)}
                      highlighted={highlighted}
                      focused={unitFocused}
                    />
                  )
                }
                return (
                  <EventRow
                    key={event.id}
                    event={event}
                    highlighted={highlighted}
                    focused={unitFocused}
                  />
                )
              })}
            </div>
          )
        })}
        {streamEvents.length === 0 && !streamDone && (
          <p className="text-dark-500 italic animate-pulse">Waiting for events...</p>
        )}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
