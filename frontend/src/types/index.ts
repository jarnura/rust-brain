// ─── Workspace ───────────────────────────────────────────────────────────────

export type WorkspaceStatus =
  | 'cloning'
  | 'indexing'
  | 'ready'
  | 'error'
  | 'archived'

export interface Workspace {
  id: string
  name: string
  github_url: string
  status: WorkspaceStatus
  clone_path: string | null
  volume_name: string | null
  created_at: string
  updated_at: string
}

export interface CreateWorkspaceResponse {
  id: string
  status: string
  message: string
}

// ─── File tree ───────────────────────────────────────────────────────────────

export interface FileNode {
  name: string
  path: string
  is_dir: boolean
  children: FileNode[]
}

// ─── Execution ───────────────────────────────────────────────────────────────

export type ExecutionStatus =
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'timeout'
  | 'aborted'

export interface Execution {
  id: string
  workspace_id: string
  prompt: string
  branch_name: string | null
  session_id: string | null
  status: ExecutionStatus
  container_id: string | null
  exit_code: number | null
  started_at: string | null
  completed_at: string | null
  created_at: string
  // Coming from RUSA-124
  volume_name?: string | null
  opencode_endpoint?: string | null
  workspace_path?: string | null
}

export interface ExecuteResponse {
  id: string
  status: string
  message: string
}

// ─── Agent events (SSE) ──────────────────────────────────────────────────────

export type EventPhase =
  | 'init'
  | 'planning'
  | 'reasoning'
  | 'tool_call'
  | 'tool_result'
  | 'file_edit'
  | 'done'
  | 'error'
  | 'phase_change'
  | 'agent_dispatch'

export interface AgentEvent {
  id: number
  execution_id: string
  timestamp: string
  event_type: string
  content: Record<string, unknown>
}

// ─── Diff ─────────────────────────────────────────────────────────────────────

export interface DiffResponse {
  patch: string
  clean: boolean
}

// ─── Commit / Reset ──────────────────────────────────────────────────────────

export interface CommitResponse {
  sha: string
  message: string
}

export interface ResetResponse {
  message: string
  head_sha: string
}
