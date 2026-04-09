import { useEffect, useState } from 'react'
import ReactDiffViewer, { DiffMethod } from 'react-diff-viewer-continued'
import { commitWorkspace, getExecution, getWorkspaceDiff, resetWorkspace } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'

// Parse unified patch into per-file old/new string pairs for the diff viewer.
function parsePatch(patch: string): Array<{ filename: string; oldCode: string; newCode: string }> {
  if (!patch.trim()) return []

  const files: Array<{ filename: string; oldCode: string; newCode: string }> = []
  const fileSections = patch.split(/^diff --git /m).filter(Boolean)

  for (const section of fileSections) {
    const lines = section.split('\n')
    const headerLine = lines[0] ?? ''
    const match = headerLine.match(/b\/(.+)$/)
    const filename = match?.[1] ?? 'unknown'

    const oldLines: string[] = []
    const newLines: string[] = []

    for (const line of lines) {
      if (line.startsWith('---') || line.startsWith('+++') || line.startsWith('@@') || line.startsWith('\\')) continue
      if (line.startsWith('-')) {
        oldLines.push(line.slice(1))
      } else if (line.startsWith('+')) {
        newLines.push(line.slice(1))
      } else {
        oldLines.push(line.slice(1) || line)
        newLines.push(line.slice(1) || line)
      }
    }

    files.push({ filename, oldCode: oldLines.join('\n'), newCode: newLines.join('\n') })
  }

  return files
}

export function DiffViewer() {
  const {
    activeWorkspaceId,
    activeExecutionId,
    executions,
    streamDone,
    diff,
    setDiff,
    setActiveExecutionId,
  } = useWorkspaceStore()

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [commitMsg, setCommitMsg] = useState('feat: apply agent changes')
  const [actionLoading, setActionLoading] = useState<'commit' | 'reset' | null>(null)
  const [actionResult, setActionResult] = useState<string | null>(null)

  // Fetch diff when stream finishes.
  // For completed historical executions, use the stored diff_summary.
  // For the currently running execution, use the live workspace diff.
  useEffect(() => {
    if (!activeWorkspaceId || !streamDone || !activeExecutionId) return
    setLoading(true)
    setError(null)

    const exec = executions.find((e) => e.id === activeExecutionId)
    const isCompleted = exec && exec.status !== 'running' && exec.status !== 'pending'

    if (isCompleted) {
      // Fetch execution to get diff_summary (may not be in the list yet)
      getExecution(activeExecutionId)
        .then((full) => {
          if (full.diff_summary) {
            setDiff({ patch: full.diff_summary, clean: false })
          } else {
            setDiff({ patch: '', clean: true })
          }
        })
        .catch((err) => setError(err instanceof Error ? err.message : 'Failed to load diff'))
        .finally(() => setLoading(false))
    } else {
      getWorkspaceDiff(activeWorkspaceId)
        .then(setDiff)
        .catch((err) => setError(err instanceof Error ? err.message : 'Failed to load diff'))
        .finally(() => setLoading(false))
    }
  }, [activeWorkspaceId, activeExecutionId, streamDone, setDiff, executions])

  async function handleCommit() {
    if (!activeWorkspaceId) return
    setActionLoading('commit')
    setActionResult(null)
    try {
      const res = await commitWorkspace(activeWorkspaceId, commitMsg)
      setActionResult(`Committed: ${res.sha.slice(0, 7)}`)
      setDiff(null)
      setActiveExecutionId(null)
    } catch (err) {
      setActionResult(`Error: ${err instanceof Error ? err.message : 'Commit failed'}`)
    } finally {
      setActionLoading(null)
    }
  }

  async function handleReset() {
    if (!activeWorkspaceId) return
    setActionLoading('reset')
    setActionResult(null)
    try {
      const res = await resetWorkspace(activeWorkspaceId)
      setActionResult(`Reset to ${res.head_sha.slice(0, 7)}`)
      setDiff(null)
      setActiveExecutionId(null)
    } catch (err) {
      setActionResult(`Error: ${err instanceof Error ? err.message : 'Reset failed'}`)
    } finally {
      setActionLoading(null)
    }
  }

  if (!streamDone) {
    return (
      <p className="text-dark-500 text-xs italic p-2">
        {activeExecutionId ? 'Waiting for execution to finish…' : 'No active execution.'}
      </p>
    )
  }

  if (loading) {
    return <p className="text-dark-400 text-xs p-2 animate-pulse">Loading diff…</p>
  }

  if (error) {
    return <p className="text-red-400 text-xs p-2">{error}</p>
  }

  if (!diff) return null

  if (diff.clean) {
    return <p className="text-dark-400 text-xs italic p-2">No changes detected.</p>
  }

  const fileDiffs = parsePatch(diff.patch)

  return (
    <div className="space-y-4">
      {/* File diffs */}
      {fileDiffs.map((fd) => (
        <div key={fd.filename} className="rounded overflow-hidden border border-dark-700">
          <div className="bg-dark-800 px-3 py-1.5 text-xs font-mono text-dark-300 border-b border-dark-700">
            {fd.filename}
          </div>
          <div className="overflow-x-auto text-xs">
            <ReactDiffViewer
              oldValue={fd.oldCode}
              newValue={fd.newCode}
              splitView={false}
              compareMethod={DiffMethod.WORDS}
              useDarkTheme={true}
              hideLineNumbers={false}
              styles={{
                variables: {
                  dark: {
                    diffViewerBackground: '#111111',
                    diffViewerColor: '#d4d4d8',
                    addedBackground: '#14532d',
                    addedColor: '#bbf7d0',
                    removedBackground: '#7f1d1d',
                    removedColor: '#fecaca',
                    wordAddedBackground: '#166534',
                    wordRemovedBackground: '#991b1b',
                    codeFoldBackground: '#1f2937',
                    codeFoldGutterBackground: '#111827',
                    codeFoldContentColor: '#6b7280',
                    gutterBackground: '#1a1a1a',
                    gutterColor: '#4b5563',
                  },
                },
              }}
            />
          </div>
        </div>
      ))}

      {/* Accept / Discard */}
      <div className="flex items-center gap-2 pt-2">
        <input
          type="text"
          value={commitMsg}
          onChange={(e) => setCommitMsg(e.target.value)}
          className="flex-1 bg-dark-800 border border-dark-600 rounded px-3 py-1.5 text-sm text-dark-100 font-mono focus:outline-none focus:border-brand-500"
        />
        <button
          onClick={handleCommit}
          disabled={!!actionLoading}
          className="px-4 py-1.5 bg-green-700 hover:bg-green-600 disabled:opacity-50 rounded text-sm font-medium text-white transition-colors"
        >
          {actionLoading === 'commit' ? 'Committing…' : 'Accept'}
        </button>
        <button
          onClick={handleReset}
          disabled={!!actionLoading}
          className="px-4 py-1.5 bg-red-800 hover:bg-red-700 disabled:opacity-50 rounded text-sm font-medium text-white transition-colors"
        >
          {actionLoading === 'reset' ? 'Discarding…' : 'Discard'}
        </button>
      </div>
      {actionResult && (
        <p className="text-xs text-dark-300">{actionResult}</p>
      )}
    </div>
  )
}
