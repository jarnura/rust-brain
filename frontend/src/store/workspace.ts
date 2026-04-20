import { create } from 'zustand'
import type { ConnectionState } from '../api/sse-reconnect'
import type { AgentEvent, DiffResponse, Execution, FileNode, Workspace } from '../types'

export type StreamConnectionState = ConnectionState

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
  /**
   * Append an event, deduping by `event.id` (server-assigned seq). Required
   * for gap-free backfill: the reconnect client already dedups within a
   * session, but a second subscriber on the same store (e.g. StrictMode
   * double-invoke, tab refocus) may replay events we have already seen.
   */
  appendStreamEvent: (event: AgentEvent) => void
  clearStreamEvents: () => void
  streamDone: boolean
  setStreamDone: (done: boolean) => void

  // SSE connection health (RUSA-257)
  streamConnectionState: StreamConnectionState
  setStreamConnectionState: (state: StreamConnectionState) => void
  /** Number of reconnect-gap events — see `onGap` in api/client.ts. */
  streamGapCount: number
  recordStreamGap: () => void
  clearStreamGap: () => void

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
        streamConnectionState: 'connecting' as StreamConnectionState,
        streamGapCount: 0,
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
    set((state) => {
      // Dedup by seq (stored as `id` on AgentEvent). If we have already
      // persisted this event, drop the duplicate so the UI never renders the
      // same row twice during an overlapping backfill + live window.
      if (event.id !== undefined) {
        for (const existing of state.streamEvents) {
          if (existing.id === event.id) return state
        }
      }
      return { streamEvents: [...state.streamEvents, event] }
    }),
  clearStreamEvents: () =>
    set({
      streamEvents: [],
      streamDone: false,
      streamConnectionState: 'connecting',
      streamGapCount: 0,
    }),
  streamDone: false,
  setStreamDone: (streamDone) => set({ streamDone }),

  streamConnectionState: 'connecting',
  setStreamConnectionState: (streamConnectionState) =>
    set({ streamConnectionState }),
  streamGapCount: 0,
  recordStreamGap: () =>
    set((state) => ({ streamGapCount: state.streamGapCount + 1 })),
  clearStreamGap: () => set({ streamGapCount: 0 }),

  diff: null,
  setDiff: (diff) => set({ diff }),
}))
