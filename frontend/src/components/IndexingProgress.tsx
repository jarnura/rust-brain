import { useEffect, useState } from 'react'
import {
  INDEX_STAGE_NAMES,
  type IndexStageName,
  type Workspace,
  type WorkspaceStatus,
} from '../types'

interface IndexingProgressProps {
  workspace: Workspace
}

type TimelineStep = {
  key: WorkspaceStatus
  label: string
}

const TIMELINE: TimelineStep[] = [
  { key: 'pending', label: 'Pending' },
  { key: 'cloning', label: 'Cloning' },
  { key: 'indexing', label: 'Indexing' },
  { key: 'ready', label: 'Ready' },
]

function timelineIndex(status: WorkspaceStatus): number {
  const idx = TIMELINE.findIndex((step) => step.key === status)
  return idx === -1 ? 0 : idx
}

function formatElapsed(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '—'
  const seconds = Math.floor(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const remSec = seconds % 60
  if (minutes < 60) return `${minutes}m ${remSec}s`
  const hours = Math.floor(minutes / 60)
  const remMin = minutes % 60
  return `${hours}h ${remMin}m`
}

function useElapsed(startedAt: string | null, active: boolean): string {
  const [now, setNow] = useState<number>(() => Date.now())

  useEffect(() => {
    if (!active || !startedAt) return
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [active, startedAt])

  if (!startedAt) return '—'
  const start = Date.parse(startedAt)
  if (Number.isNaN(start)) return '—'
  return formatElapsed(now - start)
}

function stageProgressIndex(
  indexStage: string | null,
  completedStages: Array<string> | undefined,
): number {
  // Returns index of the currently active stage, or -1 if unknown.
  if (indexStage) {
    const idx = INDEX_STAGE_NAMES.findIndex((name) => name === indexStage)
    if (idx !== -1) return idx
  }
  // Fall back to completed_stages — current stage is the first one not completed.
  if (completedStages && completedStages.length > 0) {
    for (let i = 0; i < INDEX_STAGE_NAMES.length; i += 1) {
      if (!completedStages.includes(INDEX_STAGE_NAMES[i])) return i
    }
    return INDEX_STAGE_NAMES.length - 1
  }
  return -1
}

export function IndexingProgress({ workspace }: IndexingProgressProps) {
  const { status, index_started_at, index_stage, index_progress, index_error } =
    workspace

  const isActive = status === 'cloning' || status === 'indexing'
  const elapsed = useElapsed(index_started_at, isActive)

  const currentTimelineIdx = timelineIndex(status)

  const completedStages = Array.isArray(index_progress?.completed_stages)
    ? (index_progress?.completed_stages as string[])
    : undefined
  const activeStageIdx = stageProgressIndex(index_stage, completedStages)

  const percent =
    typeof index_progress?.percent === 'number'
      ? Math.max(0, Math.min(100, index_progress.percent))
      : undefined

  const itemsTotal =
    typeof index_progress?.items_total === 'number' ? index_progress.items_total : undefined
  const itemsProcessed =
    typeof index_progress?.items_processed === 'number'
      ? index_progress.items_processed
      : undefined

  return (
    <div
      className="mt-2 rounded border border-dark-700 bg-dark-900/60 p-2 space-y-2"
      data-testid="indexing-progress"
    >
      {/* Status timeline */}
      <div className="flex items-center gap-1">
        {TIMELINE.map((step, idx) => {
          const reached = idx <= currentTimelineIdx
          const current = idx === currentTimelineIdx
          const isErrorAtStep = status === 'error' && current
          let dotClass = 'bg-dark-700'
          if (isErrorAtStep) dotClass = 'bg-red-500'
          else if (current) dotClass = 'bg-brand-400 ring-2 ring-brand-500/40'
          else if (reached) dotClass = 'bg-brand-500'
          return (
            <div key={step.key} className="flex items-center gap-1 flex-1 last:flex-none">
              <div className="flex flex-col items-center gap-1 min-w-0">
                <span
                  className={`w-2 h-2 rounded-full flex-shrink-0 ${dotClass}`}
                  aria-hidden
                />
                <span
                  className={`text-[9px] uppercase tracking-wider truncate ${
                    current ? 'text-brand-300' : reached ? 'text-dark-200' : 'text-dark-500'
                  }`}
                >
                  {step.label}
                </span>
              </div>
              {idx < TIMELINE.length - 1 && (
                <div
                  className={`h-px flex-1 ${reached && idx < currentTimelineIdx ? 'bg-brand-500' : 'bg-dark-700'}`}
                />
              )}
            </div>
          )
        })}
      </div>

      {/* Stage breakdown bar — only meaningful once indexing has started */}
      {status === 'indexing' && (
        <div>
          <div className="flex items-center justify-between text-[10px] text-dark-400 mb-1">
            <span>
              Stage{' '}
              <span className="text-brand-300 font-medium">
                {index_stage ?? INDEX_STAGE_NAMES[Math.max(0, activeStageIdx)]}
              </span>
            </span>
            {percent !== undefined && (
              <span className="text-dark-300">{percent.toFixed(0)}%</span>
            )}
          </div>
          <div className="flex gap-0.5" aria-label="Ingestion stages">
            {INDEX_STAGE_NAMES.map((name, idx) => {
              let cls = 'bg-dark-700'
              if (activeStageIdx !== -1) {
                if (idx < activeStageIdx) cls = 'bg-brand-500'
                else if (idx === activeStageIdx) cls = 'bg-brand-400 animate-pulse'
              }
              return (
                <div
                  key={name as IndexStageName}
                  className={`flex-1 h-1.5 rounded-sm ${cls}`}
                  title={name}
                  aria-label={name}
                />
              )
            })}
          </div>
          <div className="flex justify-between text-[9px] text-dark-500 mt-1">
            {INDEX_STAGE_NAMES.map((name) => (
              <span key={`label-${name}`} className="flex-1 text-center">
                {name}
              </span>
            ))}
          </div>
          {itemsTotal !== undefined && itemsProcessed !== undefined && (
            <div className="text-[10px] text-dark-400 mt-1">
              {itemsProcessed.toLocaleString()} / {itemsTotal.toLocaleString()} items
            </div>
          )}
        </div>
      )}

      {/* Elapsed */}
      <div className="flex items-center justify-between text-[10px]">
        <span className="text-dark-500">Elapsed</span>
        <span className="text-dark-200 font-mono">{elapsed}</span>
      </div>

      {/* Error */}
      {status === 'error' && index_error && (
        <div className="rounded border border-red-900/60 bg-red-950/40 p-1.5 text-[10px] text-red-300 whitespace-pre-wrap break-words">
          {index_error}
        </div>
      )}
    </div>
  )
}
