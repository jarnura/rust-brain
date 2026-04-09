import { useEffect } from 'react'
import { listExecutions } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { Execution, ExecutionStatus } from '../types'

function statusBadge(status: ExecutionStatus) {
  const map: Record<ExecutionStatus, string> = {
    pending: 'bg-dark-700 text-dark-300',
    running: 'bg-blue-900 text-blue-300',
    completed: 'bg-green-900 text-green-300',
    failed: 'bg-red-900 text-red-300',
    timeout: 'bg-yellow-900 text-yellow-300',
    aborted: 'bg-dark-700 text-dark-400',
  }
  return `inline-block px-1.5 py-0.5 rounded text-[10px] font-bold ${map[status] ?? 'bg-dark-700 text-dark-400'}`
}

function formatTs(ts: string | null): string {
  if (!ts) return '—'
  return new Date(ts).toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
}

export function SessionHistory() {
  const {
    activeWorkspaceId,
    executions,
    setExecutions,
    activeExecutionId,
    setActiveExecutionId,
    clearStreamEvents,
    setDiff,
  } = useWorkspaceStore()

  useEffect(() => {
    if (!activeWorkspaceId) return
    listExecutions(activeWorkspaceId)
      .then(setExecutions)
      .catch(() => {})
  }, [activeWorkspaceId, setExecutions])

  function handleSelect(exec: Execution) {
    clearStreamEvents()
    setDiff(null)
    // Don't set streamDone here — let the SSE stream's "done" event set it.
    // clearStreamEvents() already resets streamDone to false.
    // For completed executions, the backend replays events and sends "done".
    setActiveExecutionId(exec.id)
  }

  if (!activeWorkspaceId) {
    return <p className="text-dark-500 text-xs italic p-2">No workspace selected.</p>
  }

  if (executions.length === 0) {
    return <p className="text-dark-500 text-xs italic p-2">No executions yet.</p>
  }

  return (
    <ul className="space-y-1.5">
      {executions.map((exec) => (
        <li key={exec.id}>
          <button
            onClick={() => handleSelect(exec)}
            className={`w-full text-left px-3 py-2 rounded transition-colors ${
              activeExecutionId === exec.id
                ? 'bg-brand-500/20 border border-brand-500/40'
                : 'bg-dark-800 hover:bg-dark-700'
            }`}
          >
            <div className="flex items-center justify-between gap-2 mb-0.5">
              <span className={statusBadge(exec.status)}>{exec.status}</span>
              <span className="text-dark-500 text-[10px] tabular-nums">
                {formatTs(exec.created_at)}
              </span>
            </div>
            <p className="text-xs text-dark-300 truncate">{exec.prompt}</p>
          </button>
        </li>
      ))}
    </ul>
  )
}
