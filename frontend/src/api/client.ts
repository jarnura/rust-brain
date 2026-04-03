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

// Resolve API base from env or default to same-host port 8088
const API_BASE =
  import.meta.env.VITE_API_BASE_URL ??
  `${window.location.protocol}//${window.location.hostname}:8088`

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

export function listFiles(id: string): Promise<FileNode[]> {
  return get(`/workspaces/${id}/files`)
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

export function listExecutions(workspaceId: string): Promise<Execution[]> {
  return get(`/workspaces/${workspaceId}/executions`)
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

  es.onmessage = (e) => {
    if (closed) return
    try {
      const data = JSON.parse(e.data as string) as unknown
      onEvent(data)
    } catch {
      // non-JSON keepalive comment — ignore
    }
  }

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
