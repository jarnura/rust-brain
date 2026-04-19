import { useEffect, useState } from 'react'
import { getWorkspaceStats } from '../api/client'
import type { Workspace, WorkspaceStats } from '../types'

interface WorkspaceStatsHeaderProps {
  workspace: Workspace
}

function formatNumber(n: number | null | undefined): string {
  if (n === null || n === undefined) return '—'
  return n.toLocaleString()
}

function formatDuration(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds)) return '—'
  if (seconds < 60) return `${seconds}s`
  const m = Math.floor(seconds / 60)
  const s = Math.floor(seconds % 60)
  if (m < 60) return `${m}m ${s}s`
  const h = Math.floor(m / 60)
  const mm = m % 60
  return `${h}h ${mm}m`
}

function formatTimestamp(iso: string | null | undefined): string {
  if (!iso) return '—'
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

/**
 * Header strip shown above the workspace detail view with per-workspace
 * counts (items, graph nodes/edges, vectors), identity fields, and
 * indexing timestamps. Pulls from `GET /workspaces/:id/stats`.
 */
export function WorkspaceStatsHeader({ workspace }: WorkspaceStatsHeaderProps) {
  const [stats, setStats] = useState<WorkspaceStats | null>(null)
  const [loading, setLoading] = useState<boolean>(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setError(null)
    getWorkspaceStats(workspace.id)
      .then((data) => {
        if (cancelled) return
        setStats(data)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setError(err instanceof Error ? err.message : 'Failed to load stats')
      })
      .finally(() => {
        if (cancelled) return
        setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [workspace.id])

  const computedDurationSec = (() => {
    if (stats?.index_duration_seconds != null) return stats.index_duration_seconds
    if (workspace.index_started_at && workspace.index_completed_at) {
      const start = Date.parse(workspace.index_started_at)
      const end = Date.parse(workspace.index_completed_at)
      if (!Number.isNaN(start) && !Number.isNaN(end) && end >= start) {
        return Math.round((end - start) / 1000)
      }
    }
    return null
  })()

  return (
    <section
      className="border-b border-dark-800 bg-dark-900/40 px-4 py-3"
      data-testid="workspace-stats-header"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h2 className="text-sm font-semibold text-white truncate">{workspace.name}</h2>
            <span className="text-dark-600">·</span>
            <span className="text-xs text-dark-400 truncate">{workspace.source_url}</span>
          </div>
          <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[11px] text-dark-500">
            {workspace.schema_name && (
              <span>
                schema{' '}
                <span className="font-mono text-dark-300">{workspace.schema_name}</span>
              </span>
            )}
            {workspace.default_branch && (
              <span>
                branch{' '}
                <span className="font-mono text-dark-300">{workspace.default_branch}</span>
              </span>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-3 text-[11px] text-dark-400">
          <span>
            created <span className="text-dark-200">{formatTimestamp(workspace.created_at)}</span>
          </span>
          <span>
            indexed{' '}
            <span className="text-dark-200">
              {formatTimestamp(stats?.indexed_at ?? workspace.index_completed_at)}
            </span>
          </span>
          <span>
            duration{' '}
            <span className="text-dark-200 font-mono">{formatDuration(computedDurationSec)}</span>
          </span>
        </div>
      </div>

      <div className="mt-3 grid grid-cols-2 sm:grid-cols-4 gap-2">
        <StatTile label="Items" value={stats?.pg_items_count} loading={loading} error={!!error} />
        <StatTile
          label="Graph nodes"
          value={stats?.neo4j_nodes_count}
          loading={loading}
          error={!!error}
        />
        <StatTile
          label="Graph edges"
          value={stats?.neo4j_edges_count}
          loading={loading}
          error={!!error}
        />
        <StatTile
          label="Vectors"
          value={stats?.qdrant_vectors_count}
          loading={loading}
          error={!!error}
        />
      </div>

      {error && (
        <p className="mt-2 text-[11px] text-red-400" role="alert">
          Stats unavailable: {error}
        </p>
      )}
    </section>
  )
}

interface StatTileProps {
  label: string
  value: number | null | undefined
  loading: boolean
  error: boolean
}

function StatTile({ label, value, loading, error }: StatTileProps) {
  let display: string
  if (error) display = '—'
  else if (loading && value === undefined) display = '…'
  else display = formatNumber(value)

  return (
    <div className="rounded border border-dark-700 bg-dark-900/60 px-2.5 py-1.5">
      <div className="text-[10px] uppercase tracking-wider text-dark-500">{label}</div>
      <div className="mt-0.5 text-sm font-mono text-dark-100">{display}</div>
    </div>
  )
}
