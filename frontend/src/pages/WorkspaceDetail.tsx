import { DiffViewer } from '../components/DiffViewer'
import { ExecutionStream } from '../components/ExecutionStream'
import { PromptInput } from '../components/PromptInput'
import { SessionHistory } from '../components/SessionHistory'
import { WorkspaceView } from '../components/WorkspaceView'
import { useWorkspaceStore } from '../store/workspace'

export function WorkspaceDetail() {
  const { activeWorkspaceId, workspaces, setActiveWorkspaceId } = useWorkspaceStore()

  const workspace = workspaces.find((w) => w.id === activeWorkspaceId)

  return (
    <div className="flex-1 flex min-h-0 overflow-hidden">
      {/* Left: file tree */}
      <aside className="w-56 shrink-0 border-r border-dark-800 flex flex-col overflow-hidden">
        <div className="px-3 py-2 border-b border-dark-800 flex items-center justify-between">
          <span className="text-xs font-medium text-dark-400 uppercase tracking-wider">Files</span>
          <button
            onClick={() => setActiveWorkspaceId(null)}
            className="text-dark-500 hover:text-dark-300 text-xs transition-colors"
            title="Close workspace"
          >
            ✕
          </button>
        </div>
        {workspace && (
          <div className="px-3 py-1.5 border-b border-dark-800">
            <p className="text-xs font-medium text-dark-200 truncate">{workspace.name}</p>
            <p className="text-[10px] text-dark-500 truncate">{workspace.github_url}</p>
          </div>
        )}
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
  )
}
