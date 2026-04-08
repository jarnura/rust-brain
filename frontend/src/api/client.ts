import type {
  CommitResponse,
  CreateWorkspaceResponse,
  DiffResponse,
  Execution,
  ExecuteResponse,
  FileNode,
  ResetResponse,
  Workspace,
} from '../types'

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

/**
 * Open an EventSource SSE stream for a given execution.
 *
 * Prefers `/executions/:id/events` (cleaner route). Falls back to
 * `/workspaces/:wsId/stream?execution_id=:id` if needed.
 *
 * @returns A cleanup function that closes the EventSource.
 */
export function openExecutionStream(
  executionId: string,
  onEvent: (event: unknown) => void,
  onError: (err: Event) => void,
  onDone: () => void,
): () => void {
  const url = `${API_BASE}/executions/${executionId}/events`
  const es = new EventSource(url)

  // Track reconnect attempts for graceful back-off
  let closed = false

  const handleMessage = (e: MessageEvent) => {
    if (closed) return
    try {
      const data = JSON.parse(e.data as string) as unknown
      onEvent(data)
    } catch {
      // non-JSON keepalive comment — ignore
    }
  }

  // Listen for typed "agent_event" SSE events (what the backend actually sends)
  es.addEventListener('agent_event', handleMessage)

  // Also keep onmessage as fallback for generic/unnamed SSE messages
  es.onmessage = handleMessage

  es.addEventListener('done', () => {
    if (!closed) {
      closed = true
      es.close()
      onDone()
    }
  })

  es.onerror = (e) => {
    if (closed) return
    onError(e)
    // EventSource auto-reconnects; close explicitly on error to avoid loops
    closed = true
    es.close()
    onDone()
  }

  return () => {
    closed = true
    es.close()
  }
}
