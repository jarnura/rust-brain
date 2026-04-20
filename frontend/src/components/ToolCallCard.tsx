import { useCallback, useState } from 'react'
import type { ToolCallContent, TypedAgentEvent } from '../types/events'

// ─── Pure helpers (exported for tests) ───────────────────────────────────────

export type JsonTokenType =
  | 'key'
  | 'string'
  | 'number'
  | 'boolean'
  | 'null'
  | 'punct'
  | 'whitespace'
  | 'text'

export interface JsonToken {
  type: JsonTokenType
  value: string
}

/** Tokenize pretty-printed JSON for syntax highlighting. If the input is not
 *  valid JSON, returns a single `text` token so the caller can render it
 *  verbatim without losing information. */
export function tokenizeJson(text: string): JsonToken[] {
  if (text.length === 0) return []
  try {
    JSON.parse(text)
  } catch {
    return [{ type: 'text', value: text }]
  }

  const tokens: JsonToken[] = []
  const pattern =
    /("(?:[^"\\]|\\.)*")(?=\s*:)|("(?:[^"\\]|\\.)*")|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|\b(true|false)\b|\b(null)\b|(\s+)|([{}[\]:,])/g

  let lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      tokens.push({ type: 'text', value: text.slice(lastIndex, match.index) })
    }
    if (match[1] !== undefined) tokens.push({ type: 'key', value: match[1] })
    else if (match[2] !== undefined) tokens.push({ type: 'string', value: match[2] })
    else if (match[3] !== undefined) tokens.push({ type: 'number', value: match[3] })
    else if (match[4] !== undefined) tokens.push({ type: 'boolean', value: match[4] })
    else if (match[5] !== undefined) tokens.push({ type: 'null', value: match[5] })
    else if (match[6] !== undefined) tokens.push({ type: 'whitespace', value: match[6] })
    else if (match[7] !== undefined) tokens.push({ type: 'punct', value: match[7] })
    lastIndex = pattern.lastIndex
  }
  if (lastIndex < text.length) {
    tokens.push({ type: 'text', value: text.slice(lastIndex) })
  }
  return tokens
}

/** Copy text to the clipboard. Returns true on success. Async so callers can
 *  await the permissioned clipboard API, but it swallows errors (e.g. denied
 *  permission, insecure context) to keep the UI non-fatal. */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    return false
  }
  return false
}

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
  /** Controlled `expanded` state. When omitted, the card manages its own. */
  expanded?: boolean
  onToggle?: () => void
  /** Highlight ring (search match). */
  highlighted?: boolean
  /** Focus ring (keyboard navigation). */
  focused?: boolean
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

const TOKEN_CLASS: Record<JsonTokenType, string> = {
  key: 'text-brand-300',
  string: 'text-green-300',
  number: 'text-yellow-300',
  boolean: 'text-purple-300',
  null: 'text-purple-300',
  punct: 'text-dark-400',
  whitespace: '',
  text: 'text-dark-200',
}

function HighlightedJson({ text }: { text: string }) {
  const tokens = tokenizeJson(text)
  if (tokens.length === 1 && tokens[0].type === 'text') {
    return <>{tokens[0].value}</>
  }
  return (
    <>
      {tokens.map((token, i) => {
        const className = TOKEN_CLASS[token.type]
        return className ? (
          <span key={i} className={className}>
            {token.value}
          </span>
        ) : (
          <span key={i}>{token.value}</span>
        )
      })}
    </>
  )
}

function TruncatedBlock({ raw, label }: { raw: string; label: string }) {
  const [showFull, setShowFull] = useState(false)
  const [copied, setCopied] = useState(false)
  const truncated = truncateText(raw)
  const visible = showFull || !truncated.truncated ? raw : truncated.text

  const handleCopy = useCallback(
    async (e: React.MouseEvent<HTMLButtonElement>) => {
      e.stopPropagation()
      const ok = await copyToClipboard(raw)
      if (ok) {
        setCopied(true)
        setTimeout(() => setCopied(false), 1500)
      }
    },
    [raw],
  )

  return (
    <div className="mt-2">
      <div className="flex items-center justify-between mb-1 gap-2">
        <span className="text-[10px] font-semibold uppercase tracking-wide text-dark-400">
          {label}
        </span>
        <div className="flex items-center gap-3">
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
          <button
            type="button"
            onClick={handleCopy}
            aria-label={`copy ${label}`}
            className="text-[10px] text-dark-400 hover:text-dark-200"
          >
            {copied ? 'copied' : 'copy'}
          </button>
        </div>
      </div>
      <pre className="whitespace-pre-wrap break-all bg-dark-900/60 text-dark-200 text-[11px] leading-snug p-2 rounded max-h-64 overflow-auto">
        <HighlightedJson text={visible} />
        {!showFull && truncated.truncated && '\n…'}
      </pre>
    </div>
  )
}

export function ToolCallCard({
  event,
  expanded: expandedProp,
  onToggle,
  highlighted = false,
  focused = false,
}: ToolCallCardProps) {
  const [internalExpanded, setInternalExpanded] = useState(false)
  const isControlled = expandedProp !== undefined
  const expanded = isControlled ? expandedProp : internalExpanded
  const handleToggle = () => {
    if (onToggle) onToggle()
    if (!isControlled) setInternalExpanded((v) => !v)
  }
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
  const ringClass = focused
    ? 'ring-1 ring-brand-400/70'
    : highlighted
      ? 'ring-1 ring-yellow-400/60'
      : ''

  return (
    <div className={`border ${borderColor} rounded-md bg-dark-900/30 ${ringClass}`}>
      <button
        type="button"
        onClick={handleToggle}
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
