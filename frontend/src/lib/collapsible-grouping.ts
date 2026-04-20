// Pure helpers for transcript grouping, navigation, search, and auto-scroll.
//
// Phase 3D (RUSA-256) wires collapse/expand controls and keyboard navigation
// onto the trace view. Per FR-32, density controls may collapse events into
// summaries but must never silently omit them — `summarizeGroup` always
// reports the count.

import type {
  ToolCallContent,
  TypedAgentEvent,
  TypedEventContent,
} from '../types/events'

/**
 * Per-event navigable kinds — anything else is folded into 'other'. The
 * ordering here is incidental; consumers should treat the kind as opaque.
 */
export type GroupKind = TypedEventContent['kind']

/**
 * A run of consecutive events of the same `kind`. Tool calls are always
 * placed in singleton groups so each card retains its independent collapse
 * state from Phase 3B.
 */
export interface TranscriptGroup {
  /** Stable id derived from the first event's id. */
  id: string
  kind: GroupKind
  events: TypedAgentEvent[]
}

/** Flat navigable unit inside the transcript: either a single event card,
 *  or the header of a collapsed group of multiple events. */
export type NavUnitKind = 'event' | 'group-header'

export interface NavUnit {
  /** Unique key for React + focus management. */
  id: string
  kind: NavUnitKind
  /** Reference back to the originating group. */
  groupId: string
  /** Index into the transcript groups array. */
  groupIndex: number
  /** When `kind === 'event'`, the index inside the group's `events`. */
  eventIndex?: number
}

/** Group consecutive events of the same `content.kind`. Tool call events
 *  always form singleton groups so per-card expand/collapse stays usable. */
export function groupConsecutiveEvents(
  events: readonly TypedAgentEvent[],
): TranscriptGroup[] {
  if (events.length === 0) return []

  const groups: TranscriptGroup[] = []
  for (const event of events) {
    const kind = event.content.kind
    const last = groups.length > 0 ? groups[groups.length - 1] : null
    const canMerge =
      last !== null && last.kind === kind && kind !== 'tool_call'
    if (canMerge) {
      last.events.push(event)
    } else {
      groups.push({
        id: `g-${event.id}`,
        kind,
        events: [event],
      })
    }
  }
  return groups
}

/** Compact summary line shown when a group is collapsed. Always includes the
 *  event count so callers can verify FR-32 (no silent omission). */
export function summarizeGroup(group: TranscriptGroup): string {
  const count = group.events.length
  if (count === 0) return 'empty group'

  const first = group.events[0]
  switch (first.content.kind) {
    case 'tool_call': {
      const c = first.content
      const status = toolCallStatusLabel(c)
      return count === 1
        ? `tool: ${c.tool}${status ? ` → ${status}` : ''}`
        : `${count} tool calls (${c.tool}…)`
    }
    case 'reasoning':
      return count === 1 ? 'reasoning' : `${count} reasoning events`
    case 'agent_dispatch':
      return count === 1
        ? `dispatch ${first.content.agent}`
        : `${count} agent dispatches`
    case 'phase_change':
      return count === 1
        ? `phase: ${first.content.phase}`
        : `${count} phase changes`
    case 'error':
      return count === 1 ? 'error' : `${count} errors`
    case 'file_edit':
      return count === 1
        ? `file: ${first.content.path}`
        : `${count} file edits`
    case 'container_kept_alive':
      return count === 1
        ? 'container heartbeat'
        : `${count} container heartbeats`
    case 'unknown':
      return count === 1 ? 'unknown event' : `${count} unknown events`
  }
}

function toolCallStatusLabel(content: ToolCallContent): string {
  if (!('result' in content) || content.result === null || content.result === undefined) {
    return 'pending'
  }
  if (typeof content.result === 'string' && /^\s*error\b/i.test(content.result)) {
    return 'error'
  }
  if (
    content.result !== null &&
    typeof content.result === 'object' &&
    !Array.isArray(content.result)
  ) {
    const record = content.result as Record<string, unknown>
    if (record.is_error === true) return 'error'
    const err = record.error
    if (typeof err === 'string' && err.length > 0) return 'error'
    if (err !== null && err !== undefined && typeof err === 'object') return 'error'
  }
  return 'completed'
}

/** Build the flat list of navigable units used by keyboard navigation. */
export function buildNavUnits(
  groups: readonly TranscriptGroup[],
  collapsedGroupIds: ReadonlySet<string>,
): NavUnit[] {
  const units: NavUnit[] = []
  groups.forEach((group, groupIndex) => {
    if (collapsedGroupIds.has(group.id)) {
      units.push({
        id: `${group.id}:hdr`,
        kind: 'group-header',
        groupId: group.id,
        groupIndex,
      })
      return
    }
    group.events.forEach((event, eventIndex) => {
      units.push({
        id: `${group.id}:${event.id}`,
        kind: 'event',
        groupId: group.id,
        groupIndex,
        eventIndex,
      })
    })
  })
  return units
}

/** Bounded increment used by the J keybinding. Returns the same index when
 *  already at the end so navigation never wraps unexpectedly. */
export function nextNavIndex(current: number, total: number): number {
  if (total <= 0) return 0
  if (current < 0) return 0
  if (current >= total - 1) return total - 1
  return current + 1
}

/** Bounded decrement used by the K keybinding. */
export function prevNavIndex(current: number, total: number): number {
  if (total <= 0) return 0
  if (current <= 0) return 0
  return current - 1
}

/** Whether the scroll viewport is within `threshold` pixels of the bottom.
 *  Used to decide if auto-scroll should remain enabled — once the user
 *  scrolls up past the threshold we pause auto-scroll. */
export function isNearBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  threshold = 32,
): boolean {
  const distanceFromBottom = scrollHeight - (scrollTop + clientHeight)
  return distanceFromBottom <= threshold
}

/** Case-insensitive substring match across the human-visible fields of an
 *  event. Returns false for empty queries. Filter-as-highlight is FR-32
 *  compliant — non-matches stay rendered, just unhighlighted. */
export function eventMatchesQuery(event: TypedAgentEvent, query: string): boolean {
  const q = query.trim().toLowerCase()
  if (q.length === 0) return false
  const haystack = collectSearchableText(event).toLowerCase()
  return haystack.includes(q)
}

function collectSearchableText(event: TypedAgentEvent): string {
  const parts: string[] = [event.event_type]
  const c = event.content
  switch (c.kind) {
    case 'reasoning':
      parts.push(c.agent, c.text)
      break
    case 'tool_call':
      parts.push(c.agent, c.tool)
      if (c.args !== undefined) parts.push(safeStringify(c.args))
      if (c.result !== undefined) parts.push(safeStringify(c.result))
      break
    case 'agent_dispatch':
      parts.push(c.agent)
      break
    case 'error':
      parts.push(c.error)
      if (c.stage) parts.push(c.stage)
      break
    case 'file_edit':
      parts.push(c.path)
      break
    case 'phase_change':
      parts.push(c.phase)
      break
    case 'container_kept_alive':
      parts.push(c.expires_at)
      break
    case 'unknown':
      parts.push(c.raw_event_type, safeStringify(c.raw))
      break
  }
  return parts.join(' ')
}

function safeStringify(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value)
  } catch {
    return String(value)
  }
}
