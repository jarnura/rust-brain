import type {
  CommitResponse,
  CreateWorkspaceResponse,
  DiffResponse,
  Execution,
  ExecuteResponse,
  FileNode,
  ResetResponse,
  Workspace,
  WorkspaceStats,
} from '../types'
import {
  openReconnectingEventSource,
  type ConnectionState,
} from './sse-reconnect'

export type { ConnectionState } from './sse-reconnect'

// Resolve API base: use env override if set, otherwise derive from the
// current page hostname so the playground works from any device (Tailscale, LAN, etc.)
const envBase = import.meta.env.VITE_API_BASE_URL as string | undefined
export const API_BASE =
  envBase && !envBase.includes('localhost')
    ? envBase
    : `${window.location.protocol}//${window.location.hostname}:8088`

// ─── HTTP helpers ─────────────────────────────────────────────────────────────

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = `${API_BASE}${path}`
  const res = await fetch(url, {
    headers: { 'Content-Type': 'application/json', ...init?.headers },
    ...init,
  })
  if (!res.ok) {
    const body = await res.json().catch(() => ({}))
    const msg = (body as { error?: string }).error ?? `HTTP ${res.status}`
    throw new Error(msg)
  }
  return res.json() as Promise<T>
}

function get<T>(path: string): Promise<T> {
  return request<T>(path)
}

function post<T>(path: string, body?: unknown): Promise<T> {
  return request<T>(path, {
    method: 'POST',
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
}

async function del(path: string): Promise<void> {
  const url = `${API_BASE}${path}`
  const res = await fetch(url, { method: 'DELETE' })
  if (!res.ok && res.status !== 204) {
    const body = await res.json().catch(() => ({}))
    const msg = (body as { error?: string }).error ?? `HTTP ${res.status}`
    throw new Error(msg)
  }
}

// ─── Workspace API ────────────────────────────────────────────────────────────

export function createWorkspace(
  github_url: string,
  name?: string,
): Promise<CreateWorkspaceResponse> {
  return post('/workspaces', { github_url, name })
}

export function listWorkspaces(): Promise<Workspace[]> {
  return get('/workspaces')
}

export function getWorkspace(id: string): Promise<Workspace> {
  return get(`/workspaces/${id}`)
}

/**
 * Fetch per-workspace stats: item counts across Postgres, Neo4j, and Qdrant,
 * plus consistency deltas and isolation checks. See `GET /workspaces/:id/stats`.
 */
export function getWorkspaceStats(id: string): Promise<WorkspaceStats> {
  return get(`/workspaces/${id}/stats`)
}

/**
 * Archive and clean up a workspace.
 *
 * The backend's `DELETE /workspaces/:id` is a soft delete: it marks the
 * workspace as `archived`, drops the per-workspace Postgres schema + Qdrant
 * collections, removes the Docker volume, and cleans the clone directory.
 * The workspace row itself is retained for audit history.
 *
 * Returns `204 No Content` on success, `404` if not found.
 */
export function deleteWorkspace(id: string): Promise<void> {
  return del(`/workspaces/${id}`)
}

export async function listFiles(id: string): Promise<FileNode[]> {
  const root = await get<FileNode | FileNode[]>(`/workspaces/${id}/files`)
  // API returns a single root FileNode; extract its children as the tree.
  if (Array.isArray(root)) return root
  return root.children ?? []
}

// ─── Execution API ────────────────────────────────────────────────────────────

export function executeWorkspace(
  workspaceId: string,
  prompt: string,
  branchName?: string,
): Promise<ExecuteResponse> {
  return post(`/workspaces/${workspaceId}/execute`, {
    prompt,
    branch_name: branchName,
  })
}

export function getExecution(id: string): Promise<Execution> {
  return get(`/executions/${id}`)
}

export async function listExecutions(workspaceId: string): Promise<Execution[]> {
  const data = await get<Execution[]>(`/workspaces/${workspaceId}/executions`)
  return Array.isArray(data) ? data : []
}

// ─── Diff / Commit / Reset ────────────────────────────────────────────────────

export function getWorkspaceDiff(id: string): Promise<DiffResponse> {
  return get(`/workspaces/${id}/diff`)
}

export function commitWorkspace(
  id: string,
  message: string,
): Promise<CommitResponse> {
  return post(`/workspaces/${id}/commit`, { message })
}

export function resetWorkspace(id: string): Promise<ResetResponse> {
  return post(`/workspaces/${id}/reset`)
}

// ─── SSE stream ───────────────────────────────────────────────────────────────

export interface ExecutionStreamCallbacks {
  /** Called for every live or backfilled agent event (deduped by seq). */
  onEvent: (event: unknown) => void
  /** Called when the transport state changes (connecting/connected/reconnecting/disconnected). */
  onStateChange?: (state: ConnectionState) => void
  /**
   * Called when the server skips past the expected seq after a reconnect.
   * `expected` is `lastSeq + 1`; `actual` is the seq of the first event
   * received after the reconnect. In normal operation the backend backfills
   * the full range, so this should never fire — when it does, it indicates
   * the client missed events that the server has already evicted from its
   * buffer (the ring buffer only holds the last N events).
   */
  onGap?: (expected: number, actual: number) => void
  /** Called once when the server sends the terminal `done` event. */
  onDone: () => void
}

/**
 * Open a reconnecting SSE stream for a given execution.
 *
 * The underlying transport is `ReconnectingEventSource`: on error the
 * connection is retried with exponential backoff (1s → 30s) and the
 * `?last_event_id=<seq>` cursor so the backend (RUSA-252) backfills any
 * events missed during the outage. Events are keyed by seq and forwarded at
 * most once, so overlapping backfill + live windows are a no-op for the
 * caller (RUSA-257, FR-24 / NFR-11).
 *
 * @returns A cleanup function that closes the stream permanently.
 */
export function openExecutionStream(
  executionId: string,
  callbacks: ExecutionStreamCallbacks,
): () => void {
  const url = `${API_BASE}/executions/${executionId}/events`
  const handle = openReconnectingEventSource(
    {
      url,
      eventName: 'agent_event',
    },
    {
      onEvent: (data) => callbacks.onEvent(data),
      onStateChange: (state) => callbacks.onStateChange?.(state),
      onGap: (expected, actual) => callbacks.onGap?.(expected, actual),
      onDone: () => callbacks.onDone(),
    },
  )

  return () => handle.close()
}
