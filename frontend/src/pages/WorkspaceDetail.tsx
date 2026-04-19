import { DiffViewer } from '../components/DiffViewer'
import { ExecutionStream } from '../components/ExecutionStream'
import { PromptInput } from '../components/PromptInput'
import { RuntimeInfoPanel } from '../components/RuntimeInfoPanel'
import { SessionHistory } from '../components/SessionHistory'
import { WorkspaceStatsHeader } from '../components/WorkspaceStatsHeader'
import { WorkspaceView } from '../components/WorkspaceView'
import { useWorkspaceStore } from '../store/workspace'

export function WorkspaceDetail() {
  const { activeWorkspaceId, workspaces, setActiveWorkspaceId } = useWorkspaceStore()

  const workspace = workspaces.find((w) => w.id === activeWorkspaceId)

  if (!workspace) {
    return (
      <div className="flex-1 flex items-center justify-center">
        <div className="text-center">
          <p className="text-dark-400 text-sm mb-4">Workspace not found.</p>
          <button
            onClick={() => setActiveWorkspaceId(null)}
            className="px-4 py-2 bg-dark-800 hover:bg-dark-700 rounded text-sm text-dark-200 transition-colors"
          >
            Back to Workspaces
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
      <WorkspaceStatsHeader workspace={workspace} />

      <div className="flex-1 flex min-h-0 overflow-hidden">
        {/* Left: file tree */}
        <aside className="w-56 shrink-0 border-r border-dark-800 flex flex-col overflow-hidden">
          <div className="px-3 py-2 border-b border-dark-800">
            <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">Files</span>
          </div>
          <div className="flex-1 overflow-y-auto p-1">
            <WorkspaceView />
          </div>
        </aside>

        {/* Center: stream + diff */}
        <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
          {/* Prompt input bar */}
          <div className="px-4 py-3 border-b border-dark-800">
            <PromptInput />
          </div>

          {/* Collapsible runtime info */}
          <RuntimeInfoPanel />

          {/* SSE stream */}
          <div className="flex-1 overflow-hidden flex flex-col px-4 py-3 min-h-0">
            <div className="flex-1 overflow-y-auto min-h-0">
              <ExecutionStream />
            </div>
          </div>

          {/* Diff viewer */}
          <div className="border-t border-dark-800 px-4 py-3 overflow-y-auto max-h-96">
            <div className="flex items-center mb-2">
              <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">Changes</span>
            </div>
            <DiffViewer />
          </div>
        </main>

        {/* Right: session history */}
        <aside className="w-56 shrink-0 border-l border-dark-800 overflow-y-auto">
          <div className="px-3 py-2 border-b border-dark-800">
            <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">History</span>
          </div>
          <div className="p-2">
            <SessionHistory />
          </div>
        </aside>
      </div>
    </div>
  )
}
