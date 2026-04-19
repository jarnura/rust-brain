import { create } from 'zustand'
import type { AgentEvent, DiffResponse, Execution, FileNode, Workspace } from '../types'

interface WorkspaceState {
  // workspace list
  workspaces: Workspace[]
  setWorkspaces: (ws: Workspace[]) => void
  upsertWorkspace: (ws: Workspace) => void
  removeWorkspace: (id: string) => void

  // active workspace
  activeWorkspaceId: string | null
  setActiveWorkspaceId: (id: string | null) => void

  // file tree
  files: FileNode[]
  setFiles: (files: FileNode[]) => void

  // selected file path (for display)
  selectedFile: string | null
  setSelectedFile: (path: string | null) => void

  // execution
  activeExecutionId: string | null
  setActiveExecutionId: (id: string | null) => void
  executions: Execution[]
  setExecutions: (execs: Execution[]) => void
  upsertExecution: (exec: Execution) => void

  // SSE event stream
  streamEvents: AgentEvent[]
  appendStreamEvent: (event: AgentEvent) => void
  clearStreamEvents: () => void
  streamDone: boolean
  setStreamDone: (done: boolean) => void

  // diff
  diff: DiffResponse | null
  setDiff: (diff: DiffResponse | null) => void
}

export const useWorkspaceStore = create<WorkspaceState>((set) => ({
  workspaces: [],
  setWorkspaces: (workspaces) => set({ workspaces }),
  upsertWorkspace: (ws) =>
    set((state) => {
      const idx = state.workspaces.findIndex((w) => w.id === ws.id)
      if (idx === -1) return { workspaces: [ws, ...state.workspaces] }
      const updated = [...state.workspaces]
      updated[idx] = ws
      return { workspaces: updated }
    }),
  removeWorkspace: (id) =>
    set((state) => {
      const workspaces = state.workspaces.filter((w) => w.id !== id)
      if (state.activeWorkspaceId !== id) return { workspaces }
      return {
        workspaces,
        activeWorkspaceId: null,
        files: [],
        selectedFile: null,
        activeExecutionId: null,
        streamEvents: [],
        streamDone: false,
        diff: null,
      }
    }),

  activeWorkspaceId: null,
  setActiveWorkspaceId: (id) => set({ activeWorkspaceId: id }),

  files: [],
  setFiles: (files) => set({ files }),

  selectedFile: null,
  setSelectedFile: (path) => set({ selectedFile: path }),

  activeExecutionId: null,
  setActiveExecutionId: (id) => set({ activeExecutionId: id }),

  executions: [],
  setExecutions: (executions) => set({ executions }),
  upsertExecution: (exec) =>
    set((state) => {
      const idx = state.executions.findIndex((e) => e.id === exec.id)
      if (idx === -1) return { executions: [exec, ...state.executions] }
      const updated = [...state.executions]
      updated[idx] = exec
      return { executions: updated }
    }),

  streamEvents: [],
  appendStreamEvent: (event) =>
    set((state) => ({ streamEvents: [...state.streamEvents, event] })),
  clearStreamEvents: () => set({ streamEvents: [], streamDone: false }),
  streamDone: false,
  setStreamDone: (streamDone) => set({ streamDone }),

  diff: null,
  setDiff: (diff) => set({ diff }),
}))
