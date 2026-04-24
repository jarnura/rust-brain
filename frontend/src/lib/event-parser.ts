// Parser that lifts a raw SSE `AgentEvent` into a discriminated `TypedAgentEvent`.
//
// Never throws. Any shape that fails validation falls back to `UnknownContent`
// so the UI can still render something and we never silently drop events
// (see RECONCILIATION.md R-4, which flagged silent drops as a P0 bug).

import type { AgentEvent } from '../types'
import type {
  AgentDispatchContent,
  ContainerKeptAliveContent,
  ErrorContent,
  FileEditContent,
  PhaseChangeContent,
  ReasoningContent,
  ToolCallContent,
  TypedAgentEvent,
  TypedEventContent,
  UnknownContent,
} from '../types/events'

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

function asNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function toRawRecord(
  eventType: string,
  content: Record<string, unknown> | null | undefined,
): UnknownContent {
  return {
    kind: 'unknown',
    raw_event_type: eventType,
    raw: content && isRecord(content) ? content : {},
  }
}

function parseReasoning(content: Record<string, unknown>): ReasoningContent | null {
  const agent = asString(content.agent)
  if (agent === undefined) return null

  const stepStart = asString(content.step_start)
  const stepFinish = asString(content.step_finish)

  // Per R-1, reasoning may use `text` or (legacy) `reasoning` for the body.
  // Real OpenCode events may also carry `step_start`/`step_finish` instead.
  let text = asString(content.text) ?? asString(content.reasoning)

  if ((text === undefined || text.length === 0) && stepStart !== undefined) {
    text = `Starting step: ${stepStart}`
  } else if ((text === undefined || text.length === 0) && stepFinish !== undefined) {
    text = `Finished: ${stepFinish}`
  }

  if (text === undefined || text.length === 0) return null

  const result: ReasoningContent = { kind: 'reasoning', agent, text }
  if (stepStart !== undefined) result.step_start = stepStart
  if (stepFinish !== undefined) result.step_finish = stepFinish
  return result
}

function parseToolCall(content: Record<string, unknown>): ToolCallContent | null {
  const agent = asString(content.agent)
  const tool = asString(content.tool)
  if (agent === undefined || tool === undefined) return null
  const out: ToolCallContent = { kind: 'tool_call', agent, tool }
  if ('args' in content) out.args = content.args
  if ('result' in content) out.result = content.result
  return out
}

function parseAgentDispatch(content: Record<string, unknown>): AgentDispatchContent | null {
  const agent = asString(content.agent)
  if (agent === undefined) return null
  return { kind: 'agent_dispatch', agent }
}

function parseError(content: Record<string, unknown>): ErrorContent | null {
  const error = asString(content.error)
  if (error === undefined) return null
  const stage = asString(content.stage)
  return stage !== undefined
    ? { kind: 'error', error, stage }
    : { kind: 'error', error }
}

function parseFileEdit(content: Record<string, unknown>): FileEditContent | null {
  const path = asString(content.path)
  if (path === undefined) return null
  const { path: _path, ...rest } = content
  return { kind: 'file_edit', path, ...rest }
}

function parsePhaseChange(content: Record<string, unknown>): PhaseChangeContent | null {
  const phase = asString(content.phase)
  if (phase === undefined) return null
  return { kind: 'phase_change', phase }
}

function parseContainerKeptAlive(
  content: Record<string, unknown>,
): ContainerKeptAliveContent | null {
  const expires_at = asString(content.expires_at)
  const keep_alive_secs = asNumber(content.keep_alive_secs)
  if (expires_at === undefined || keep_alive_secs === undefined) return null
  return { kind: 'container_kept_alive', expires_at, keep_alive_secs }
}

function dispatchContent(
  eventType: string,
  rawContent: Record<string, unknown>,
): TypedEventContent {
  switch (eventType) {
    case 'reasoning':
      return parseReasoning(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'tool_call':
      return parseToolCall(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'agent_dispatch':
      return parseAgentDispatch(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'error':
      return parseError(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'file_edit':
      return parseFileEdit(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'phase_change':
      return parsePhaseChange(rawContent) ?? toRawRecord(eventType, rawContent)
    case 'container_kept_alive':
      return parseContainerKeptAlive(rawContent) ?? toRawRecord(eventType, rawContent)
    default:
      return toRawRecord(eventType, rawContent)
  }
}

/**
 * Parses a raw `AgentEvent` into a `TypedAgentEvent` with a discriminated
 * content union. Never throws.
 *
 * - If `event_type` is one of the 7 known kinds and `content` matches the
 *   expected shape, returns the narrowed variant.
 * - If `content` is missing required fields or is not an object, or if
 *   `event_type` is not recognized, returns an `UnknownContent` variant
 *   preserving the original `event_type` and raw content.
 */
export function parseAgentEvent(raw: AgentEvent): TypedAgentEvent {
  const eventType = typeof raw.event_type === 'string' ? raw.event_type : ''
  const rawContent = isRecord(raw.content) ? raw.content : {}
  const content = isRecord(raw.content)
    ? dispatchContent(eventType, rawContent)
    : toRawRecord(eventType, rawContent)
  return {
    id: raw.id,
    execution_id: raw.execution_id,
    timestamp: raw.timestamp,
    event_type: raw.event_type,
    content,
  }
}
