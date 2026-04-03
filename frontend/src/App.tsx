import { useEffect } from 'react'
import { listWorkspaces } from './api/client'
import { WorkspaceDetail } from './pages/WorkspaceDetail'
import { WorkspaceList } from './pages/WorkspaceList'
import { useWorkspaceStore } from './store/workspace'

export function App() {
  const { activeWorkspaceId, setWorkspaces } = useWorkspaceStore()

  // Bootstrap workspace list on mount
  useEffect(() => {
    listWorkspaces()
      .then(setWorkspaces)
      .catch(() => {})
  }, [setWorkspaces])

  return (
    <div className="flex flex-col h-screen bg-dark-950 text-dark-100">
      {/* Topbar */}
      <header className="bg-dark-900 border-b border-dark-800 px-4 py-2 flex items-center gap-3 shrink-0">
        <span className="text-xl">🧠</span>
        <span className="font-bold text-white">Rust Brain</span>
        <span className="text-dark-600">·</span>
        <span className="text-dark-400 text-sm">Editor Playground</span>
      </header>

      {/* Main content */}
      {activeWorkspaceId ? <WorkspaceDetail /> : <WorkspaceList />}
    </div>
  )
}
