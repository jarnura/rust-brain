import { useState } from 'react'
import { createWorkspace, listWorkspaces } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'
import type { Workspace } from '../types'

function statusBadge(status: Workspace['status']) {
  const map: Record<string, string> = {
    cloning: 'bg-yellow-900 text-yellow-300',
    indexing: 'bg-blue-900 text-blue-300',
    ready: 'bg-green-900 text-green-300',
    error: 'bg-red-900 text-red-300',
    archived: 'bg-dark-700 text-dark-400',
  }
  return `inline-block px-2 py-0.5 rounded text-xs font-medium ${map[status] ?? 'bg-dark-700 text-dark-400'}`
}

export function RepoManager() {
  const { workspaces, setWorkspaces, upsertWorkspace, setActiveWorkspaceId, setFiles, clearStreamEvents } =
    useWorkspaceStore()

  const [url, setUrl] = useState('')
  const [name, setName] = useState('')
  const [cloning, setCloning] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Poll workspace until it leaves the 'cloning'/'indexing' state
  function pollWorkspace(id: string) {
    const interval = setInterval(async () => {
      try {
        const res = await fetch(
          `${import.meta.env.VITE_API_BASE_URL ?? `${window.location.protocol}//${window.location.hostname}:8088`}/workspaces/${id}`,
        )
        if (!res.ok) return
        const ws = (await res.json()) as Workspace
        upsertWorkspace(ws)
        if (ws.status !== 'cloning' && ws.status !== 'indexing') {
          clearInterval(interval)
        }
      } catch {
        clearInterval(interval)
      }
    }, 2000)
  }

  async function handleClone(e: React.FormEvent) {
    e.preventDefault()
    if (!url.trim()) return
    setError(null)
    setCloning(true)
    try {
      const res = await createWorkspace(url.trim(), name.trim() || undefined)
      // Optimistically show the new workspace
      const placeholder: Workspace = {
        id: res.id,
        name: name.trim() || url.split('/').pop() || res.id,
        github_url: url.trim(),
        status: 'cloning',
        clone_path: null,
        volume_name: null,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      }
      upsertWorkspace(placeholder)
      setUrl('')
      setName('')
      pollWorkspace(res.id)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Clone failed')
    } finally {
      setCloning(false)
    }
  }

  async function handleRefresh() {
    try {
      const ws = await listWorkspaces()
      setWorkspaces(ws)
    } catch {
      // ignore
    }
  }

  function handleSelect(ws: Workspace) {
    if (ws.status !== 'ready') return
    setActiveWorkspaceId(ws.id)
    setFiles([])
    clearStreamEvents()
  }

  return (
    <div className="space-y-4">
      {/* Clone form */}
      <form onSubmit={handleClone} className="flex flex-col gap-2">
        <div className="flex gap-2">
          <input
            type="url"
            placeholder="https://github.com/org/repo"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            required
            className="flex-1 bg-dark-800 border border-dark-600 rounded px-3 py-1.5 text-sm text-dark-100 placeholder-dark-500 focus:outline-none focus:border-brand-500"
          />
          <input
            type="text"
            placeholder="Name (optional)"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-36 bg-dark-800 border border-dark-600 rounded px-3 py-1.5 text-sm text-dark-100 placeholder-dark-500 focus:outline-none focus:border-brand-500"
          />
          <button
            type="submit"
            disabled={cloning}
            className="px-4 py-1.5 bg-brand-500 hover:bg-brand-600 disabled:opacity-50 rounded text-sm font-medium text-white transition-colors"
          >
            {cloning ? 'Cloning…' : 'Clone'}
          </button>
        </div>
        {error && <p className="text-red-400 text-xs">{error}</p>}
      </form>

      {/* Workspace list */}
      <div className="flex items-center justify-between">
        <span className="text-xs text-dark-400 font-medium uppercase tracking-wider">
          Workspaces
        </span>
        <button
          onClick={handleRefresh}
          className="text-xs text-dark-400 hover:text-dark-200 transition-colors"
        >
          Refresh
        </button>
      </div>

      {workspaces.length === 0 ? (
        <p className="text-dark-500 text-sm italic">No workspaces yet.</p>
      ) : (
        <ul className="space-y-1.5">
          {workspaces.map((ws) => (
            <li key={ws.id}>
              <button
                onClick={() => handleSelect(ws)}
                disabled={ws.status !== 'ready'}
                className="w-full text-left px-3 py-2 rounded bg-dark-800 hover:bg-dark-700 disabled:opacity-60 disabled:cursor-default transition-colors group"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="text-sm text-dark-100 truncate group-hover:text-white transition-colors">
                    {ws.name}
                  </span>
                  <span className={statusBadge(ws.status)}>{ws.status}</span>
                </div>
                <p className="text-xs text-dark-500 truncate mt-0.5">{ws.github_url}</p>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
