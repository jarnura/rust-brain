import { useState } from 'react'
import { executeWorkspace } from '../api/client'
import { useWorkspaceStore } from '../store/workspace'

export function PromptInput() {
  const {
    activeWorkspaceId,
    setActiveExecutionId,
    clearStreamEvents,
    upsertExecution,
    setDiff,
  } = useWorkspaceStore()

  const [prompt, setPrompt] = useState('')
  const [branch, setBranch] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    if (!activeWorkspaceId || !prompt.trim()) return

    setError(null)
    setSubmitting(true)
    clearStreamEvents()
    setDiff(null)

    try {
      const res = await executeWorkspace(
        activeWorkspaceId,
        prompt.trim(),
        branch.trim() || undefined,
      )
      setActiveExecutionId(res.id)
      // Seed the store with a pending execution
      upsertExecution({
        id: res.id,
        workspace_id: activeWorkspaceId,
        prompt: prompt.trim(),
        branch_name: branch.trim() || null,
        session_id: null,
        status: 'pending',
        container_id: null,
        exit_code: null,
        started_at: null,
        completed_at: null,
        created_at: new Date().toISOString(),
      })
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Execution failed')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-2">
      <textarea
        rows={4}
        placeholder="Describe the feature or change you want…"
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        disabled={!activeWorkspaceId || submitting}
        className="w-full bg-dark-800 border border-dark-600 rounded px-3 py-2 text-sm text-dark-100 placeholder-dark-500 font-sans resize-none focus:outline-none focus:border-brand-500 disabled:opacity-50"
      />
      <div className="flex items-center gap-2">
        <input
          type="text"
          placeholder="Branch name (optional)"
          value={branch}
          onChange={(e) => setBranch(e.target.value)}
          disabled={!activeWorkspaceId || submitting}
          className="flex-1 bg-dark-800 border border-dark-600 rounded px-3 py-1.5 text-sm text-dark-100 placeholder-dark-500 focus:outline-none focus:border-brand-500 disabled:opacity-50"
        />
        <button
          type="submit"
          disabled={!activeWorkspaceId || !prompt.trim() || submitting}
          className="px-5 py-1.5 bg-brand-500 hover:bg-brand-600 disabled:opacity-50 disabled:cursor-not-allowed rounded text-sm font-medium text-white transition-colors"
        >
          {submitting ? 'Starting…' : 'Execute'}
        </button>
      </div>
      {error && <p className="text-red-400 text-xs">{error}</p>}
      {!activeWorkspaceId && (
        <p className="text-dark-500 text-xs italic">Select a ready workspace first.</p>
      )}
    </form>
  )
}
