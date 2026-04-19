import { useEffect, useRef, useState } from 'react'
import { useWorkspaceStore } from '../store/workspace'
import type { Workspace } from '../types'

function badgeClass(status: Workspace['status']): string {
  const map: Record<string, string> = {
    pending: 'bg-dark-700 text-dark-300',
    cloning: 'bg-yellow-900 text-yellow-300',
    indexing: 'bg-blue-900 text-blue-300',
    ready: 'bg-green-900 text-green-300',
    error: 'bg-red-900 text-red-300',
    archived: 'bg-dark-700 text-dark-400',
  }
  return `inline-block px-1.5 py-0.5 rounded text-[10px] font-medium uppercase tracking-wider ${
    map[status] ?? 'bg-dark-700 text-dark-400'
  }`
}

/**
 * Header control shown only when a workspace is active. Displays the current
 * workspace name + status badge, and opens a dropdown that lets the user
 * quickly switch to any other `ready` workspace or return to the workspace list.
 * Replaces the standalone close (✕) button.
 */
export function WorkspaceSwitcher() {
  const { activeWorkspaceId, workspaces, setActiveWorkspaceId, setFiles, clearStreamEvents } =
    useWorkspaceStore()
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open) return
    function onDocClick(e: MouseEvent) {
      if (!rootRef.current) return
      if (!rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDocClick)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDocClick)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const active = workspaces.find((w) => w.id === activeWorkspaceId) ?? null
  if (!active) return null

  const switchable = workspaces.filter((w) => w.status === 'ready' && w.id !== active.id)

  function selectWorkspace(id: string) {
    if (id === active?.id) {
      setOpen(false)
      return
    }
    setActiveWorkspaceId(id)
    setFiles([])
    clearStreamEvents()
    setOpen(false)
  }

  function backToList() {
    setActiveWorkspaceId(null)
    setFiles([])
    clearStreamEvents()
    setOpen(false)
  }

  return (
    <div ref={rootRef} className="relative ml-auto" data-testid="workspace-switcher">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="listbox"
        aria-expanded={open}
        className="flex items-center gap-2 px-2.5 py-1 rounded bg-dark-800 hover:bg-dark-700 border border-dark-700 transition-colors"
      >
        <span className="text-xs font-medium text-dark-100 max-w-[16rem] truncate">
          {active.name}
        </span>
        <span className={badgeClass(active.status)}>{active.status}</span>
        <span className="text-dark-400 text-[10px]">▾</span>
      </button>

      {open && (
        <div
          role="listbox"
          className="absolute right-0 mt-1 w-72 max-h-80 overflow-y-auto bg-dark-900 border border-dark-700 rounded shadow-lg z-50"
        >
          <div className="px-3 py-1.5 text-[10px] uppercase tracking-wider text-dark-500 border-b border-dark-800">
            Switch workspace
          </div>
          {switchable.length === 0 ? (
            <div className="px-3 py-2 text-xs text-dark-500 italic">
              No other ready workspaces.
            </div>
          ) : (
            <ul>
              {switchable.map((ws) => (
                <li key={ws.id}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={false}
                    onClick={() => selectWorkspace(ws.id)}
                    className="w-full flex items-center justify-between gap-2 px-3 py-1.5 text-left hover:bg-dark-800 transition-colors"
                  >
                    <span className="flex flex-col min-w-0">
                      <span className="text-xs text-dark-100 truncate">{ws.name}</span>
                      <span className="text-[10px] text-dark-500 truncate">{ws.source_url}</span>
                    </span>
                    <span className={badgeClass(ws.status)}>{ws.status}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          <div className="border-t border-dark-800">
            <button
              type="button"
              onClick={backToList}
              className="w-full text-left px-3 py-1.5 text-xs text-dark-200 hover:bg-dark-800 transition-colors"
            >
              ← All workspaces
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
