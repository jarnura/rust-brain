import { useEffect, useState } from 'react'
import { getExecution } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { Execution } from '../types'

interface InfoRowProps {
  label: string
  value: string | null | undefined
}

function InfoRow({ label, value }: InfoRowProps) {
  if (value == null) return null
  return (
    <div className="flex items-center gap-2 py-0.5">
      <span className="text-dark-500 text-[10px] uppercase tracking-wider w-28 shrink-0">
        {label}
      </span>
      <code className="text-dark-200 text-[11px] font-mono truncate" title={value}>
        {value}
      </code>
    </div>
  )
}

export function RuntimeInfoPanel() {
  const { activeExecutionId } = useWorkspaceStore()
  const [execution, setExecution] = useState<Execution | null>(null)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    if (!activeExecutionId) {
      setExecution(null)
      return
    }

    let cancelled = false

    getExecution(activeExecutionId)
      .then((exec) => {
        if (!cancelled) setExecution(exec)
      })
      .catch(() => {
        if (!cancelled) setExecution(null)
      })

    return () => {
      cancelled = true
    }
  }, [activeExecutionId])

  if (!execution) return null

  const hasRuntimeFields =
    execution.session_id ||
    execution.container_id ||
    execution.volume_name ||
    execution.opencode_endpoint ||
    execution.workspace_path

  if (!hasRuntimeFields) return null

  return (
    <div className="border-b border-dark-800">
      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        className="w-full flex items-center gap-2 px-4 py-1.5 text-left hover:bg-dark-800/50 transition-colors"
      >
        <span
          className={`text-dark-500 text-[10px] transition-transform ${open ? 'rotate-90' : ''}`}
        >
          &#9654;
        </span>
        <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">
          Runtime Info
        </span>
        <span className="text-[10px] text-dark-600 font-mono ml-auto">
          {execution.status}
        </span>
      </button>
      {open && (
        <div className="px-4 pb-2 pl-8">
          <InfoRow label="Session ID" value={execution.session_id} />
          <InfoRow label="Container ID" value={execution.container_id} />
          <InfoRow label="Volume" value={execution.volume_name} />
          <InfoRow label="Endpoint" value={execution.opencode_endpoint} />
          <InfoRow label="Workspace" value={execution.workspace_path} />
        </div>
      )}
    </div>
  )
}
