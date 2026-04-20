import { useState } from 'react'
import type { ToolCallContent, TypedAgentEvent } from '../types/events'

// ─── Pure helpers (exported for tests) ───────────────────────────────────────

/** Inline display cap for pretty-printed JSON args/result. */
export const INLINE_MAX_BYTES = 2048

/** Status of an atomic tool call — derived purely from whether `result` is present.
 *
 * Per RECONCILIATION.md R-2/R-9 the event is atomic: there are no streaming
 * update phases. Absence of `result` at display time means the buffer was
 * read while the tool was still running server-side; we show a pending
 * indicator rather than inventing a lifecycle. */
export type ToolCallStatus = 'completed' | 'error' | 'pending'

export function deriveToolCallStatus(content: ToolCallContent): ToolCallStatus {
  if (!('result' in content) || content.result === null || content.result === undefined) {
    return 'pending'
  }
  if (isErrorResult(content.result)) return 'error'
  return 'completed'
}

/** A result counts as an error if it's an object with a truthy `error` field,
 *  or an object with `is_error === true`, or a string matching /^error:/i. */
export function isErrorResult(result: unknown): boolean {
  if (typeof result === 'string') return /^\s*error\b/i.test(result)
  if (result !== null && typeof result === 'object') {
    const record = result as Record<string, unknown>
    if (record.is_error === true) return true
    const err = record.error
    if (typeof err === 'string' && err.length > 0) return true
    if (err !== null && err !== undefined && typeof err === 'object') return true
  }
  return false
}

/** Pretty-print any value as JSON. Strings pass through verbatim. Non-JSON
 *  values (undefined, bigint, functions) are coerced via String(). */
export function formatValue(value: unknown): string {
  if (value === undefined) return ''
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

export interface TruncatedText {
  text: string
  truncated: boolean
  originalLength: number
}

/** Truncate to a byte budget. Returns the original text plus a `truncated`
 *  flag so the UI can show a "show full" affordance. */
export function truncateText(text: string, maxBytes: number = INLINE_MAX_BYTES): TruncatedText {
  if (text.length <= maxBytes) {
    return { text, truncated: false, originalLength: text.length }
  }
  return {
    text: text.slice(0, maxBytes),
    truncated: true,
    originalLength: text.length,
  }
}

/** Compact header-line rendering of args for the collapsed state. */
export function summarizeArgs(args: unknown): string {
  if (args === undefined || args === null) return ''
  if (typeof args === 'string') {
    return args.length > 60 ? `${args.slice(0, 60)}…` : args
  }
  if (typeof args === 'object') {
    try {
      const compact = JSON.stringify(args)
      return compact.length > 60 ? `${compact.slice(0, 60)}…` : compact
    } catch {
      return ''
    }
  }
  return String(args)
}

// ─── Component ───────────────────────────────────────────────────────────────

interface ToolCallCardProps {
  event: TypedAgentEvent & { content: ToolCallContent }
}

function StatusIndicator({ status }: { status: ToolCallStatus }) {
  if (status === 'completed') {
    return (
      <span
        aria-label="completed"
        title="completed"
        className="inline-flex items-center justify-center w-4 h-4 rounded-full bg-green-900/60 text-green-300 text-[10px] shrink-0"
      >
        ✓
      </span>
    )
  }
  if (status === 'error') {
    return (
      <span
        aria-label="error"
        title="error"
        className="inline-flex items-center justify-center w-4 h-4 rounded-full bg-red-900/60 text-red-300 text-[10px] shrink-0"
      >
        ✕
      </span>
    )
  }
  return (
    <span
      aria-label="pending"
      title="pending"
      className="inline-flex items-center justify-center w-4 h-4 rounded-full bg-dark-700 text-dark-400 text-[10px] shrink-0"
    >
      –
    </span>
  )
}

function TruncatedBlock({ raw, label }: { raw: string; label: string }) {
  const [showFull, setShowFull] = useState(false)
  const truncated = truncateText(raw)
  const visible = showFull || !truncated.truncated ? raw : truncated.text

  return (
    <div className="mt-2">
      <div className="flex items-center justify-between mb-1">
        <span className="text-[10px] font-semibold uppercase tracking-wide text-dark-400">
          {label}
        </span>
        {truncated.truncated && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              setShowFull((v) => !v)
            }}
            className="text-[10px] text-brand-400 hover:text-brand-300"
          >
            {showFull
              ? 'show less'
              : `show full (${truncated.originalLength.toLocaleString()} chars)`}
          </button>
        )}
      </div>
      <pre className="whitespace-pre-wrap break-all bg-dark-900/60 text-dark-200 text-[11px] leading-snug p-2 rounded max-h-64 overflow-auto">
        {visible}
        {!showFull && truncated.truncated && '\n…'}
      </pre>
    </div>
  )
}

export function ToolCallCard({ event }: ToolCallCardProps) {
  const [expanded, setExpanded] = useState(false)
  const { content, timestamp } = event
  const status = deriveToolCallStatus(content)

  const argsText = formatValue(content.args)
  const resultText = formatValue(content.result)
  const argsSummary = summarizeArgs(content.args)

  const borderColor =
    status === 'error'
      ? 'border-red-900/60'
      : status === 'pending'
        ? 'border-dark-700'
        : 'border-dark-600'

  return (
    <div className={`border ${borderColor} rounded-md bg-dark-900/30`}>
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        className="w-full flex items-center gap-2 px-2 py-1.5 text-left hover:bg-dark-800/60 rounded-md"
      >
        <span className="inline-block px-1.5 py-0.5 rounded text-[10px] font-bold tracking-wide bg-yellow-900 text-yellow-300 shrink-0">
          TOOL
        </span>
        <StatusIndicator status={status} />
        <code className="text-brand-400 font-mono text-[11px] shrink-0">
          {content.tool}
        </code>
        <span className="inline-block px-1 py-0.5 rounded text-[9px] font-medium bg-dark-800 text-dark-400 shrink-0">
          {content.agent}
        </span>
        <span className="text-dark-400 font-mono text-[10px] tabular-nums shrink-0">
          {new Date(timestamp).toLocaleTimeString('en-US', { hour12: false })}
        </span>
        {!expanded && argsSummary && (
          <span className="text-dark-500 font-mono text-[11px] truncate">
            {argsSummary}
          </span>
        )}
        <span className="ml-auto text-dark-500 text-[10px] shrink-0">
          {expanded ? '▼' : '▶'}
        </span>
      </button>

      {expanded && (
        <div className="px-2 pb-2">
          {argsText.length > 0 && <TruncatedBlock raw={argsText} label="args" />}
          {status === 'pending' && (
            <p className="mt-2 text-[11px] italic text-dark-500">
              No result yet — tool call still running when buffer was read.
            </p>
          )}
          {status !== 'pending' && resultText.length > 0 && (
            <div
              className={status === 'error' ? 'text-red-300' : 'text-dark-200'}
            >
              <TruncatedBlock
                raw={resultText}
                label={status === 'error' ? 'error' : 'result'}
              />
            </div>
          )}
        </div>
      )}
    </div>
  )
}
