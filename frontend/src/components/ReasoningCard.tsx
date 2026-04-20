import type { ReasoningContent, TypedAgentEvent } from '../types/events'

interface ReasoningCardProps {
  event: TypedAgentEvent & { content: ReasoningContent }
  expanded: boolean
  onToggle: () => void
  highlighted?: boolean
  focused?: boolean
}

const PREVIEW_LIMIT = 140

export function ReasoningCard({
  event,
  expanded,
  onToggle,
  highlighted = false,
  focused = false,
}: ReasoningCardProps) {
  const { agent, text } = event.content
  const collapsedPreview =
    text.length > PREVIEW_LIMIT ? `${text.slice(0, PREVIEW_LIMIT)}…` : text
  const ringClass = focused
    ? 'ring-1 ring-brand-400/70'
    : highlighted
      ? 'ring-1 ring-yellow-400/60'
      : ''

  return (
    <div className={`border border-blue-900/40 rounded-md bg-dark-900/30 ${ringClass}`}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className="w-full flex items-center gap-2 px-2 py-1.5 text-left hover:bg-dark-800/60 rounded-md"
      >
        <span className="inline-block px-1.5 py-0.5 rounded text-[10px] font-bold tracking-wide bg-blue-900 text-blue-300 shrink-0">
          REASONING
        </span>
        <span className="inline-block px-1 py-0.5 rounded text-[9px] font-medium bg-dark-800 text-dark-400 shrink-0">
          {agent}
        </span>
        <span className="text-dark-400 font-mono text-[10px] tabular-nums shrink-0">
          {new Date(event.timestamp).toLocaleTimeString('en-US', { hour12: false })}
        </span>
        {!expanded && collapsedPreview && (
          <span className="text-dark-300 font-mono text-[11px] truncate">
            {collapsedPreview}
          </span>
        )}
        <span className="ml-auto text-dark-500 text-[10px] shrink-0">
          {expanded ? '▼' : '▶'}
        </span>
      </button>
      {expanded && (
        <div className="px-2 pb-2">
          <pre className="whitespace-pre-wrap break-words bg-dark-900/60 text-dark-200 text-[11px] leading-snug p-2 rounded max-h-64 overflow-auto">
            {text}
          </pre>
        </div>
      )}
    </div>
  )
}
