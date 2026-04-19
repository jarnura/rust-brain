import { useEffect, useState } from 'react'
import { listWorkspaces } from './api/client'
import { ErrorBoundary } from './components/ErrorBoundary'
import { WorkspaceSwitcher } from './components/WorkspaceSwitcher'
import { WorkspaceDetail } from './pages/WorkspaceDetail'
import { WorkspaceList } from './pages/WorkspaceList'
import { useWorkspaceStore } from './store/workspace'

function AppContent() {
  const { activeWorkspaceId, setWorkspaces } = useWorkspaceStore()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  // Bootstrap workspace list on mount
  useEffect(() => {
    setLoading(true)
    setError(null)
    listWorkspaces()
      .then(setWorkspaces)
      .catch((err) => {
        setError(err instanceof Error ? err.message : 'Failed to load workspaces')
      })
      .finally(() => setLoading(false))
  }, [setWorkspaces])

  if (loading) {
    return (
      <div className="flex flex-col h-screen bg-dark-950 text-dark-100">
        <header className="bg-dark-900 border-b border-dark-800 px-4 py-2 flex items-center gap-3 shrink-0">
          <span className="text-xl">🧠</span>
          <span className="font-bold text-white">Rust Brain</span>
          <span className="text-dark-600">·</span>
          <span className="text-dark-400 text-sm">Editor Playground</span>
        </header>
        <div className="flex-1 flex items-center justify-center">
          <p className="text-dark-400 text-sm animate-pulse">Loading workspaces…</p>
        </div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex flex-col h-screen bg-dark-950 text-dark-100">
        <header className="bg-dark-900 border-b border-dark-800 px-4 py-2 flex items-center gap-3 shrink-0">
          <span className="text-xl">🧠</span>
          <span className="font-bold text-white">Rust Brain</span>
          <span className="text-dark-600">·</span>
          <span className="text-dark-400 text-sm">Editor Playground</span>
        </header>
        <div className="flex-1 flex items-center justify-center">
          <div className="max-w-md p-6 bg-dark-900 border border-dark-700 rounded-lg text-center">
            <p className="text-red-400 text-sm mb-4">{error}</p>
            <button
              onClick={() => window.location.reload()}
              className="px-4 py-2 bg-brand-500 hover:bg-brand-600 rounded text-sm font-medium text-white transition-colors"
            >
              Retry
            </button>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div className="flex flex-col h-screen bg-dark-950 text-dark-100">
      {/* Topbar */}
      <header className="bg-dark-900 border-b border-dark-800 px-4 py-2 flex items-center gap-3 shrink-0">
        <span className="text-xl">🧠</span>
        <span className="font-bold text-white">Rust Brain</span>
        <span className="text-dark-600">·</span>
        <span className="text-dark-400 text-sm">Editor Playground</span>
        {activeWorkspaceId && <WorkspaceSwitcher />}
      </header>

      {/* Main content */}
      {activeWorkspaceId ? <WorkspaceDetail /> : <WorkspaceList />}
    </div>
  )
}

export function App() {
  return (
    <ErrorBoundary>
      <AppContent />
    </ErrorBoundary>
  )
}
