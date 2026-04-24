import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { ReasoningContent, TypedAgentEvent } from '../types/events'

interface ReasoningCardProps {
  event: TypedAgentEvent & { content: ReasoningContent }
  expanded: boolean
  onToggle: () => void
  highlighted?: boolean
  focused?: boolean
}

const PREVIEW_LIMIT = 140

function StepIndicator({ content }: { content: ReasoningContent }) {
  if (content.step_start) {
    return (
      <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[9px] font-medium bg-indigo-900/60 text-indigo-300 shrink-0">
        <span className="inline-block w-1 h-1 rounded-full bg-indigo-400 animate-pulse" />
        STEP
      </span>
    )
  }
  if (content.step_finish) {
    return (
      <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded text-[9px] font-medium bg-green-900/60 text-green-300 shrink-0">
        <span className="inline-block w-1 h-1 rounded-full bg-green-400" />
        DONE
      </span>
    )
  }
  return null
}

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

  const isStepEvent = Boolean(event.content.step_start || event.content.step_finish)

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
        <StepIndicator content={event.content} />
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
          {isStepEvent ? (
            <div className="bg-dark-900/60 text-dark-200 text-[11px] leading-snug p-2 rounded">
              {text}
            </div>
          ) : (
            <div className="prose prose-invert prose-xs max-w-none bg-dark-900/60 p-2 rounded max-h-64 overflow-auto [&_pre]:bg-dark-800 [&_pre]:text-[11px] [&_pre]:p-2 [&_pre]:rounded [&_code]:text-brand-300 [&_code]:text-[11px] [&_table]:text-[11px] [&_a]:text-brand-400 [&_a]:no-underline [&_a:hover]:underline [&_p]:text-[11px] [&_p]:leading-snug [&_p]:text-dark-200 [&_li]:text-[11px] [&_li]:text-dark-200 [&_h1]:text-sm [&_h2]:text-xs [&_h3]:text-xs [&_h1]:text-dark-100 [&_h2]:text-dark-100 [&_h3]:text-dark-200 [&_hr]:border-dark-700 [&_blockquote]:border-dark-600 [&_blockquote]:text-dark-300">
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
