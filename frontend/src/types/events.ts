// Typed agent event model for OpenCode trace stream.
//
// Events are delivered via SSE from `/executions/:id/events` and persisted in
// Postgres `agent_events` (see services/api/migrations/20260403000003_agent_events.sql).
// Per docs/opencode-tracing/RECONCILIATION.md R-1/R-2, tool calls are atomic —
// each event is self-contained, no streaming update phases, no correlation keys.
//
// The discriminant is `content.kind`, which mirrors `event_type`. Consumers
// should switch on `event.content.kind` to narrow safely. An `unknown` variant
// is used for new event types and for malformed content that fails validation.

import type { AgentEvent } from './index'

/** Reasoning / text content emitted by an agent.
 *
 * Real OpenCode events may carry `step_start` / `step_finish` instead of
 * `text`. The parser synthesizes a display `text` from those fields so the
 * rest of the UI only needs to read `text`. The originals are preserved for
 * step-indicator rendering. */
export interface ReasoningContent {
  kind: 'reasoning'
  agent: string
  text: string
  step_start?: string
  step_finish?: string
}

/** Atomic tool invocation — both `args` and `result` are delivered together. */
export interface ToolCallContent {
  kind: 'tool_call'
  agent: string
  tool: string
  args?: unknown
  result?: unknown
}

/** Sub-agent dispatch event minted when a `task` tool call is detected. */
export interface AgentDispatchContent {
  kind: 'agent_dispatch'
  agent: string
}

/** Runner-side error. `stage` is present when the failure is tied to a pipeline stage. */
export interface ErrorContent {
  kind: 'error'
  error: string
  stage?: string
}

/** File edit event. The extra fields are best-effort — only `path` is required. */
export interface FileEditContent {
  kind: 'file_edit'
  path: string
  [key: string]: unknown
}

/** Legacy phase transition event (superseded by agent_dispatch but still emitted). */
export interface PhaseChangeContent {
  kind: 'phase_change'
  phase: string
}

/** Container kept-alive heartbeat. */
export interface ContainerKeptAliveContent {
  kind: 'container_kept_alive'
  expires_at: string
  keep_alive_secs: number
}

/**
 * Fallback for unrecognized event types or content that fails shape validation.
 * Preserves the original `event_type` and raw `content` for display / debugging.
 */
export interface UnknownContent {
  kind: 'unknown'
  raw_event_type: string
  raw: Record<string, unknown>
}

/** All recognized content variants, discriminated by `kind`. */
export type TypedEventContent =
  | ReasoningContent
  | ToolCallContent
  | AgentDispatchContent
  | ErrorContent
  | FileEditContent
  | PhaseChangeContent
  | ContainerKeptAliveContent
  | UnknownContent

/**
 * An agent event with `content` narrowed to a typed discriminated union.
 * `event_type` is retained from the wire format for backward compatibility,
 * but new code should switch on `content.kind` instead.
 */
export type TypedAgentEvent = Omit<AgentEvent, 'content'> & {
  content: TypedEventContent
}

/** Literal list of known kinds, used by the parser for dispatch. */
export const KNOWN_EVENT_KINDS = [
  'reasoning',
  'tool_call',
  'agent_dispatch',
  'error',
  'file_edit',
  'phase_change',
  'container_kept_alive',
] as const

export type KnownEventKind = (typeof KNOWN_EVENT_KINDS)[number]

// ─── Type guards ─────────────────────────────────────────────────────────────

export function isReasoningEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: ReasoningContent } {
  return event.content.kind === 'reasoning'
}

export function isToolCallEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: ToolCallContent } {
  return event.content.kind === 'tool_call'
}

export function isAgentDispatchEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: AgentDispatchContent } {
  return event.content.kind === 'agent_dispatch'
}

export function isErrorEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: ErrorContent } {
  return event.content.kind === 'error'
}

export function isFileEditEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: FileEditContent } {
  return event.content.kind === 'file_edit'
}

export function isPhaseChangeEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: PhaseChangeContent } {
  return event.content.kind === 'phase_change'
}

export function isContainerKeptAliveEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: ContainerKeptAliveContent } {
  return event.content.kind === 'container_kept_alive'
}

export function isUnknownEvent(
  event: TypedAgentEvent,
): event is TypedAgentEvent & { content: UnknownContent } {
  return event.content.kind === 'unknown'
}
