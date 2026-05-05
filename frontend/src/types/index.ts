// ─── Workspace ───────────────────────────────────────────────────────────────

export type WorkspaceStatus =
  | 'pending'
  | 'cloning'
  | 'indexing'
  | 'ready'
  | 'error'
  | 'archived'

export type WorkspaceSourceType = 'github' | 'local'

/**
 * Names of the 6 ingestion pipeline stages, in order.
 * Keep aligned with `services/ingestion/src/pipeline/mod.rs::STAGE_NAMES`.
 */
export const INDEX_STAGE_NAMES = [
  'expand',
  'parse',
  'typecheck',
  'extract',
  'graph',
  'embed',
] as const

export type IndexStageName = (typeof INDEX_STAGE_NAMES)[number]

/**
 * Shape of the `index_progress` JSON column. The ingestion pipeline writes
 * arbitrary metadata here — these fields are best-effort and may be missing.
 */
export interface IndexProgress {
  current_stage?: IndexStageName | string
  completed_stages?: Array<IndexStageName | string>
  percent?: number
  items_total?: number
  items_processed?: number
  [key: string]: unknown
}

export interface Workspace {
  id: string
  name: string
  source_type: WorkspaceSourceType | string
  source_url: string
  schema_name: string | null
  status: WorkspaceStatus
  clone_path: string | null
  volume_name: string | null
  default_branch: string | null
  github_auth_method?: string | null
  index_started_at: string | null
  index_completed_at: string | null
  index_stage: IndexStageName | string | null
  index_progress: IndexProgress | null
  index_error: string | null
  created_at: string
  updated_at: string
}

export interface CreateWorkspaceResponse {
  id: string
  status: string
  message: string
}

// ─── Workspace stats ─────────────────────────────────────────────────────────

/**
 * Cross-store consistency deltas returned by `GET /workspaces/:id/stats`.
 * Matches `ConsistencyInfo` in `services/api/src/handlers/workspace_stats.rs`.
 */
export interface WorkspaceStatsConsistency {
  pg_vs_neo4j_delta: number
  pg_vs_qdrant_delta: number
  status: 'consistent' | 'inconsistent' | string
}

/**
 * Workspace isolation checks returned by `GET /workspaces/:id/stats`.
 * Matches `IsolationInfo` in `services/api/src/handlers/workspace_stats.rs`.
 */
export interface WorkspaceStatsIsolation {
  multi_label_nodes: number
  cross_workspace_edges: number
  label_mismatches: number
}

/**
 * Per-workspace stats returned by `GET /workspaces/:id/stats` (RUSA-215).
 * Values come from Postgres, Neo4j, and Qdrant scoped to this workspace.
 */
export interface WorkspaceStats {
  workspace_id: string
  status: string
  pg_items_count: number
  neo4j_nodes_count: number
  neo4j_edges_count: number
  qdrant_vectors_count: number
  consistency: WorkspaceStatsConsistency
  isolation: WorkspaceStatsIsolation
  /** Seconds the last indexing run took, if known. */
  index_duration_seconds?: number | null
  created_at: string
  indexed_at?: string | null
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
  diff_summary?: string | Record<string, unknown> | null
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

// Re-export the typed event model (RUSA-253). Consumers should prefer
// `TypedAgentEvent` + `parseAgentEvent` over the untyped `AgentEvent` above.
export type {
  AgentDispatchContent,
  ContainerKeptAliveContent,
  ErrorContent,
  FileEditContent,
  KnownEventKind,
  PhaseChangeContent,
  ReasoningContent,
  ToolCallContent,
  TypedAgentEvent,
  TypedEventContent,
  UnknownContent,
} from './events'
export {
  KNOWN_EVENT_KINDS,
  isAgentDispatchEvent,
  isContainerKeptAliveEvent,
  isErrorEvent,
  isFileEditEvent,
  isPhaseChangeEvent,
  isReasoningEvent,
  isToolCallEvent,
  isUnknownEvent,
} from './events'

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

// ─── Call Graph Traversal (REQ-DP-03) ────────────────────────────────────────

/** Dispatch provenance for a call graph edge. */
export type EdgeProvenance = 'direct' | 'monomorph' | 'dyn_candidate'

/** A node discovered during BFS traversal of the call graph. */
export interface TraversalNode {
  fqn: string
  name: string
  kind?: string
  file_path?: string
  line?: number
}

/** A directed edge discovered during BFS traversal. */
export interface TraversalEdge {
  from_fqn: string
  to_fqn: string
  /** BFS depth at which this edge was discovered (1-indexed from root). */
  depth: number
  provenance: EdgeProvenance
}

/**
 * Response from GET /v1/repos/{repo_id}/items/{fqn_b64}/callers
 * and GET /v1/repos/{repo_id}/items/{fqn_b64}/callees.
 */
export interface TraversalResult {
  root: TraversalNode
  nodes: TraversalNode[]
  edges: TraversalEdge[]
  cycles_detected: boolean
  next_cursor?: string | null
}
