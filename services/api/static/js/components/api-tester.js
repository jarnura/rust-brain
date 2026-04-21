/**
 * api-tester.js — Interactive API endpoint testing panel for the rust-brain playground.
 * Renders into #pane-apitester; grouped endpoints with parameter forms and JSON response display.
 */

// ── Endpoint Catalog ──────────────────────────────────────────────────────────

const ENDPOINTS = [
  // ── Code Intelligence ───────────────────────────────────────────────────
  {
    category: 'Code Intelligence',
    id: 'search_semantic',
    method: 'POST',
    path: '/tools/search_semantic',
    description: 'Semantic vector search via Qdrant',
    params: [
      { name: 'query', type: 'string', required: true, placeholder: 'async trait implementation' },
      { name: 'limit', type: 'number', default: 10 },
      { name: 'score_threshold', type: 'number', placeholder: '0.5' },
      { name: 'crate_filter', type: 'string', placeholder: 'tokio' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'search_docs',
    method: 'POST',
    path: '/tools/search_docs',
    description: 'Documentation search via Qdrant doc_embeddings',
    params: [
      { name: 'query', type: 'string', required: true, placeholder: 'error handling patterns' },
      { name: 'limit', type: 'number', default: 10 },
      { name: 'score_threshold', type: 'number', placeholder: '0.5' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'aggregate_search',
    method: 'POST',
    path: '/tools/aggregate_search',
    description: 'Cross-DB fan-out search (Qdrant + Postgres + Neo4j)',
    params: [
      { name: 'query', type: 'string', required: true, placeholder: 'pipeline orchestration' },
      { name: 'limit', type: 'number', default: 10 },
      { name: 'score_threshold', type: 'number', placeholder: '0.5' },
      { name: 'include_source', type: 'boolean', default: false },
      { name: 'include_graph', type: 'boolean', default: true },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'get_function',
    method: 'GET',
    path: '/tools/get_function',
    description: 'Full function details with source by FQN',
    params: [
      { name: 'fqn', type: 'string', required: true, placeholder: 'rustbrain_common::types::Item' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'get_callers',
    method: 'GET',
    path: '/tools/get_callers',
    description: 'Direct and transitive callers from call graph',
    params: [
      { name: 'fqn', type: 'string', required: true, placeholder: 'ingestion::pipeline::PipelineRunner::run' },
      { name: 'depth', type: 'number', default: 1, placeholder: '1-10' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'get_trait_impls',
    method: 'GET',
    path: '/tools/get_trait_impls',
    description: 'All implementations of a trait',
    params: [
      { name: 'trait_name', type: 'string', required: true, placeholder: 'PipelineStage' },
      { name: 'limit', type: 'number', default: 10 },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'find_usages_of_type',
    method: 'GET',
    path: '/tools/find_usages_of_type',
    description: 'Where a type is used across the codebase',
    params: [
      { name: 'type_name', type: 'string', required: true, placeholder: 'AppState' },
      { name: 'limit', type: 'number', default: 10 },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'get_module_tree',
    method: 'GET',
    path: '/tools/get_module_tree',
    description: 'Module hierarchy for a crate',
    params: [
      { name: 'crate_name', type: 'string', required: true, placeholder: 'rustbrain_common' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'query_graph',
    method: 'POST',
    path: '/tools/query_graph',
    description: 'Raw Cypher query (read-only) against Neo4j',
    params: [
      { name: 'query', type: 'text', required: true, placeholder: 'MATCH (n:Function) RETURN n.name LIMIT 5' },
      { name: 'parameters', type: 'json', default: '{}', placeholder: '{"name": "foo"}' },
      { name: 'limit', type: 'number', default: 10 },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'pg_query',
    method: 'POST',
    path: '/tools/pg_query',
    description: 'Read-only SQL query against Postgres',
    params: [
      { name: 'query', type: 'text', required: true, placeholder: 'SELECT name, kind FROM extracted_items LIMIT 5' },
      { name: 'params', type: 'json', default: '[]', placeholder: '["value1"]' },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'find_calls_with_type',
    method: 'GET',
    path: '/tools/find_calls_with_type',
    description: 'Call sites with a specific type argument (turbofish)',
    params: [
      { name: 'type_name', type: 'string', required: true, placeholder: 'String' },
      { name: 'callee_name', type: 'string', placeholder: 'collect' },
      { name: 'limit', type: 'number', default: 10 },
    ],
  },
  {
    category: 'Code Intelligence',
    id: 'find_trait_impls_for_type',
    method: 'GET',
    path: '/tools/find_trait_impls_for_type',
    description: 'Trait implementations for a concrete type',
    params: [
      { name: 'type_name', type: 'string', required: true, placeholder: 'PipelineRunner' },
      { name: 'limit', type: 'number', default: 10 },
    ],
  },

  // ── Chat ────────────────────────────────────────────────────────────────
  {
    category: 'Chat',
    id: 'chat',
    method: 'POST',
    path: '/tools/chat',
    description: 'Blocking chat (returns full response)',
    params: [
      { name: 'session_id', type: 'string', required: true, placeholder: 'session UUID' },
      { name: 'message', type: 'text', required: true, placeholder: 'What does PipelineRunner do?' },
    ],
  },
  {
    category: 'Chat',
    id: 'chat_send',
    method: 'POST',
    path: '/tools/chat/send',
    description: 'Async chat (returns job ID)',
    params: [
      { name: 'session_id', type: 'string', required: true, placeholder: 'session UUID' },
      { name: 'message', type: 'text', required: true, placeholder: 'Explain the ingestion pipeline' },
    ],
  },
  {
    category: 'Chat',
    id: 'sessions_create',
    method: 'POST',
    path: '/tools/chat/sessions',
    description: 'Create a new chat session',
    params: [],
  },
  {
    category: 'Chat',
    id: 'sessions_list',
    method: 'GET',
    path: '/tools/chat/sessions',
    description: 'List all chat sessions',
    params: [],
  },
  {
    category: 'Chat',
    id: 'sessions_get',
    method: 'GET',
    path: '/tools/chat/sessions/:id',
    description: 'Get a chat session by ID',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'session UUID' },
    ],
  },
  {
    category: 'Chat',
    id: 'sessions_delete',
    method: 'DELETE',
    path: '/tools/chat/sessions/:id',
    description: 'Delete a chat session',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'session UUID' },
    ],
  },
  {
    category: 'Chat',
    id: 'sessions_fork',
    method: 'POST',
    path: '/tools/chat/sessions/:id/fork',
    description: 'Fork a chat session',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'session UUID' },
    ],
  },
  {
    category: 'Chat',
    id: 'sessions_abort',
    method: 'POST',
    path: '/tools/chat/sessions/:id/abort',
    description: 'Abort an in-progress chat session',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'session UUID' },
    ],
  },

  // ── System ──────────────────────────────────────────────────────────────
  {
    category: 'System',
    id: 'health',
    method: 'GET',
    path: '/health',
    description: 'API health check (database connectivity)',
    params: [],
  },
  {
    category: 'System',
    id: 'metrics',
    method: 'GET',
    path: '/metrics',
    description: 'Prometheus metrics endpoint',
    params: [],
  },
  {
    category: 'System',
    id: 'snapshot',
    method: 'GET',
    path: '/api/snapshot',
    description: 'Snapshot info (ingested crate metadata)',
    params: [],
  },
  {
    category: 'System',
    id: 'ingestion_progress',
    method: 'GET',
    path: '/api/ingestion/progress',
    description: 'Current ingestion pipeline progress',
    params: [],
  },
  {
    category: 'System',
    id: 'consistency',
    method: 'GET',
    path: '/api/consistency',
    description: 'Cross-store consistency check (PG/Neo4j/Qdrant)',
    params: [],
  },
  {
    category: 'System',
    id: 'health_consistency',
    method: 'GET',
    path: '/health/consistency',
    description: 'Health-check scoped consistency summary',
    params: [],
  },

  // ── CRUD ────────────────────────────────────────────────────────────────
  {
    category: 'CRUD',
    id: 'artifacts_list',
    method: 'GET',
    path: '/api/artifacts',
    description: 'List all artifacts',
    params: [],
  },
  {
    category: 'CRUD',
    id: 'artifacts_create',
    method: 'POST',
    path: '/api/artifacts',
    description: 'Create an artifact',
    params: [
      { name: 'title', type: 'string', required: true, placeholder: 'My artifact' },
      { name: 'content', type: 'text', required: true, placeholder: 'Artifact content...' },
      { name: 'artifact_type', type: 'string', default: 'note', placeholder: 'note' },
    ],
  },
  {
    category: 'CRUD',
    id: 'artifacts_get',
    method: 'GET',
    path: '/api/artifacts/:id',
    description: 'Get an artifact by ID',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'artifact UUID' },
    ],
  },
  {
    category: 'CRUD',
    id: 'tasks_list',
    method: 'GET',
    path: '/api/tasks',
    description: 'List all tasks',
    params: [],
  },
  {
    category: 'CRUD',
    id: 'tasks_create',
    method: 'POST',
    path: '/api/tasks',
    description: 'Create a task',
    params: [
      { name: 'title', type: 'string', required: true, placeholder: 'Fix pipeline bug' },
      { name: 'description', type: 'text', placeholder: 'Task description...' },
      { name: 'status', type: 'string', default: 'pending', placeholder: 'pending' },
    ],
  },
  {
    category: 'CRUD',
    id: 'tasks_get',
    method: 'GET',
    path: '/api/tasks/:id',
    description: 'Get a task by ID',
    params: [
      { name: 'id', type: 'string', required: true, pathParam: true, placeholder: 'task UUID' },
    ],
  },
];

const CATEGORIES = [...new Set(ENDPOINTS.map(e => e.category))];

const METHOD_COLORS = {
  GET: '#61affe',
  POST: '#49cc90',
  PUT: '#fca130',
  DELETE: '#f93e3e',
};

// ── ApiTesterPanel ────────────────────────────────────────────────────────────

class ApiTesterPanel {
  /** @param {HTMLElement} pane */
  constructor(pane) {
    this.pane = pane;
    this._selectedEndpoint = null;
    this._history = [];
    this._activeCategory = CATEGORIES[0];
    this._render();
    this._attach();
  }

  // ── Markup ────────────────────────────────────────────────────────────────

  _render() {
    this.pane.innerHTML = `
      <div class="at-panel">
        <div class="at-panel__header">
          <h2 class="at-panel__title">API Tester</h2>
          <span class="at-panel__subtitle">${ENDPOINTS.length} endpoints</span>
          <button class="at-panel__history-btn" id="at-history-toggle" title="Request history">History</button>
        </div>

        <div class="at-panel__categories" id="at-categories" role="tablist">
          ${CATEGORIES.map(c => `
            <button class="at-cat${c === this._activeCategory ? ' active' : ''}"
                    data-cat="${c}" role="tab">${c}
              <span class="at-cat__count">${ENDPOINTS.filter(e => e.category === c).length}</span>
            </button>
          `).join('')}
        </div>

        <div class="at-panel__body">
          <div class="at-panel__endpoints" id="at-endpoints"></div>
          <div class="at-panel__workspace" id="at-workspace">
            <div class="at-workspace__empty">Select an endpoint to begin testing</div>
          </div>
        </div>

        <div class="at-panel__history" id="at-history" hidden>
          <div class="at-history__header">
            <span>Request History</span>
            <button id="at-history-clear" class="at-history__clear" title="Clear history">Clear</button>
            <button id="at-history-close" class="at-history__close" title="Close">&times;</button>
          </div>
          <div class="at-history__list" id="at-history-list">
            <p class="at-history__empty">No requests yet</p>
          </div>
        </div>
      </div>
    `;
    this._renderEndpointList();
  }

  _renderEndpointList() {
    const list = this.pane.querySelector('#at-endpoints');
    const filtered = ENDPOINTS.filter(e => e.category === this._activeCategory);
    list.innerHTML = filtered.map(ep => `
      <button class="at-ep${this._selectedEndpoint?.id === ep.id ? ' active' : ''}"
              data-id="${ep.id}" title="${ep.description}">
        <span class="at-ep__method" style="color:${METHOD_COLORS[ep.method]}">${ep.method}</span>
        <span class="at-ep__path">${ep.path}</span>
      </button>
    `).join('');
  }

  _renderWorkspace(ep) {
    const ws = this.pane.querySelector('#at-workspace');
    const paramFields = ep.params.map(p => this._paramField(p)).join('');

    ws.innerHTML = `
      <div class="at-ws">
        <div class="at-ws__header">
          <span class="at-ws__method" style="background:${METHOD_COLORS[ep.method]}">${ep.method}</span>
          <code class="at-ws__path">${ep.path}</code>
        </div>
        <p class="at-ws__desc">${esc(ep.description)}</p>

        <form class="at-ws__form" id="at-form" autocomplete="off">
          ${paramFields || '<p class="at-ws__no-params">No parameters required</p>'}
          <div class="at-ws__actions">
            <button type="submit" class="at-ws__send" id="at-send">
              <span class="at-ws__send-label">Send Request</span>
              <span class="at-ws__spinner" id="at-spinner" hidden></span>
            </button>
            <button type="reset" class="at-ws__reset">Reset</button>
          </div>
        </form>

        <div class="at-ws__response" id="at-response">
          <div class="at-ws__response-header" id="at-resp-header" hidden>
            <span id="at-resp-status"></span>
            <span id="at-resp-time"></span>
            <button class="at-ws__copy" id="at-copy" title="Copy response">Copy</button>
          </div>
          <pre class="at-ws__response-body" id="at-resp-body"><code id="at-resp-code"></code></pre>
        </div>
      </div>
    `;
  }

  _paramField(p) {
    const id = `at-p-${p.name}`;
    const req = p.required ? ' <span class="at-field__req">*</span>' : '';
    const defaultVal = p.default !== undefined ? String(p.default) : '';

    if (p.type === 'boolean') {
      return `
        <label class="at-field at-field--bool">
          <input type="checkbox" id="${id}" name="${p.name}" ${p.default ? 'checked' : ''}>
          <span class="at-field__name">${p.name}</span>
        </label>`;
    }

    if (p.type === 'text' || p.type === 'json') {
      return `
        <div class="at-field">
          <label class="at-field__label" for="${id}">${p.name}${req}</label>
          <textarea id="${id}" name="${p.name}" class="at-field__textarea"
            placeholder="${esc(p.placeholder || '')}" rows="3"
            ${p.required ? 'required' : ''}>${esc(defaultVal)}</textarea>
        </div>`;
    }

    const inputType = p.type === 'number' ? 'number' : 'text';
    return `
      <div class="at-field">
        <label class="at-field__label" for="${id}">${p.name}${req}</label>
        <input type="${inputType}" id="${id}" name="${p.name}" class="at-field__input"
          placeholder="${esc(p.placeholder || '')}" value="${esc(defaultVal)}"
          ${p.required ? 'required' : ''} ${p.type === 'number' ? 'step="any"' : ''}>
      </div>`;
  }

  // ── Events ──────────────────────────────────────────────────────────────

  _attach() {
    this.pane.querySelector('#at-categories').addEventListener('click', e => {
      const btn = e.target.closest('.at-cat');
      if (!btn) return;
      this._activeCategory = btn.dataset.cat;
      this.pane.querySelectorAll('.at-cat').forEach(b => b.classList.toggle('active', b === btn));
      this._renderEndpointList();
      this._attachEndpointClicks();
    });

    this._attachEndpointClicks();

    this.pane.querySelector('#at-history-toggle').addEventListener('click', () => {
      const h = this.pane.querySelector('#at-history');
      h.toggleAttribute('hidden');
    });

    this.pane.querySelector('#at-history-close').addEventListener('click', () => {
      this.pane.querySelector('#at-history').setAttribute('hidden', '');
    });

    this.pane.querySelector('#at-history-clear').addEventListener('click', () => {
      this._history = [];
      this._renderHistory();
    });
  }

  _attachEndpointClicks() {
    this.pane.querySelector('#at-endpoints').addEventListener('click', e => {
      const btn = e.target.closest('.at-ep');
      if (!btn) return;
      const ep = ENDPOINTS.find(ep => ep.id === btn.dataset.id);
      if (!ep) return;
      this._selectedEndpoint = ep;
      this._renderEndpointList();
      this._renderWorkspace(ep);
      this._attachFormSubmit(ep);
    });
  }

  _attachFormSubmit(ep) {
    const form = this.pane.querySelector('#at-form');
    if (!form) return;

    form.addEventListener('submit', async e => {
      e.preventDefault();
      await this._executeRequest(ep);
    });
  }

  // ── Request Execution ───────────────────────────────────────────────────

  async _executeRequest(ep) {
    const spinner = this.pane.querySelector('#at-spinner');
    const sendBtn = this.pane.querySelector('#at-send');
    const respHeader = this.pane.querySelector('#at-resp-header');
    const respStatus = this.pane.querySelector('#at-resp-status');
    const respTime = this.pane.querySelector('#at-resp-time');
    const respCode = this.pane.querySelector('#at-resp-code');
    const copyBtn = this.pane.querySelector('#at-copy');

    spinner?.removeAttribute('hidden');
    if (sendBtn) sendBtn.disabled = true;

    const values = this._collectFormValues(ep);
    let url = ep.path;
    const headers = { 'Content-Type': 'application/json' };

    // Inject workspace header if set
    const wsSelect = document.getElementById('workspace-select');
    if (wsSelect?.value) {
      headers['X-Workspace-Id'] = wsSelect.value;
    }

    // Replace path params
    for (const p of ep.params) {
      if (p.pathParam && values[p.name]) {
        url = url.replace(`:${p.name}`, encodeURIComponent(values[p.name]));
        delete values[p.name];
      }
    }

    let fetchOpts = { method: ep.method, headers };
    if (ep.method === 'GET' || ep.method === 'DELETE') {
      const qs = new URLSearchParams(
        Object.entries(values).filter(([, v]) => v !== '' && v !== undefined && v !== null)
      ).toString();
      if (qs) url += '?' + qs;
    } else {
      const body = {};
      for (const p of ep.params) {
        if (p.pathParam) continue;
        const v = values[p.name];
        if (v === '' || v === undefined) continue;
        if (p.type === 'json') {
          try { body[p.name] = JSON.parse(v); } catch { body[p.name] = v; }
        } else if (p.type === 'number') {
          body[p.name] = Number(v);
        } else if (p.type === 'boolean') {
          body[p.name] = v === true || v === 'true';
        } else {
          body[p.name] = v;
        }
      }
      fetchOpts.body = JSON.stringify(body);
    }

    const t0 = performance.now();
    let status, responseText, ok;

    try {
      const resp = await fetch(url, fetchOpts);
      status = resp.status;
      ok = resp.ok;
      responseText = await resp.text();
    } catch (err) {
      status = 0;
      ok = false;
      responseText = JSON.stringify({ error: err.message }, null, 2);
    }

    const elapsed = Math.round(performance.now() - t0);

    // Try to pretty-print JSON
    let displayText = responseText;
    try {
      displayText = JSON.stringify(JSON.parse(responseText), null, 2);
    } catch { /* not JSON, display raw */ }

    // Show response
    if (respHeader) respHeader.removeAttribute('hidden');
    if (respStatus) {
      respStatus.textContent = `${status} ${statusText(status)}`;
      respStatus.className = ok ? 'at-resp-status--ok' : 'at-resp-status--err';
    }
    if (respTime) respTime.textContent = `${elapsed}ms`;
    if (respCode) {
      respCode.textContent = displayText;
      respCode.className = 'language-json';
      if (window.hljs) hljs.highlightElement(respCode);
    }

    // Copy button
    if (copyBtn) {
      copyBtn.onclick = () => {
        navigator.clipboard.writeText(displayText).then(() => {
          copyBtn.textContent = 'Copied!';
          setTimeout(() => { copyBtn.textContent = 'Copy'; }, 1500);
        });
      };
    }

    spinner?.setAttribute('hidden', '');
    if (sendBtn) sendBtn.disabled = false;

    // Add to history
    this._history.unshift({
      method: ep.method,
      path: url,
      status,
      elapsed,
      time: new Date().toLocaleTimeString(),
      response: displayText,
    });
    if (this._history.length > 50) this._history.length = 50;
    this._renderHistory();
  }

  _collectFormValues(ep) {
    const values = {};
    for (const p of ep.params) {
      if (p.type === 'boolean') {
        const el = this.pane.querySelector(`#at-p-${p.name}`);
        values[p.name] = el ? el.checked : false;
      } else {
        const el = this.pane.querySelector(`#at-p-${p.name}`);
        values[p.name] = el ? el.value.trim() : '';
      }
    }
    return values;
  }

  // ── History ─────────────────────────────────────────────────────────────

  _renderHistory() {
    const list = this.pane.querySelector('#at-history-list');
    if (!list) return;

    if (this._history.length === 0) {
      list.innerHTML = '<p class="at-history__empty">No requests yet</p>';
      return;
    }

    list.innerHTML = this._history.map((h, i) => `
      <button class="at-history__item" data-idx="${i}">
        <span class="at-history__method" style="color:${METHOD_COLORS[h.method]}">${h.method}</span>
        <span class="at-history__path">${esc(h.path)}</span>
        <span class="at-history__status ${h.status >= 200 && h.status < 300 ? 'at-history__status--ok' : 'at-history__status--err'}">${h.status}</span>
        <span class="at-history__time">${h.elapsed}ms</span>
        <span class="at-history__clock">${h.time}</span>
      </button>
    `).join('');

    list.querySelectorAll('.at-history__item').forEach(el => {
      el.addEventListener('click', () => {
        const idx = parseInt(el.dataset.idx, 10);
        const h = this._history[idx];
        if (!h) return;
        const respHeader = this.pane.querySelector('#at-resp-header');
        const respStatus = this.pane.querySelector('#at-resp-status');
        const respTime = this.pane.querySelector('#at-resp-time');
        const respCode = this.pane.querySelector('#at-resp-code');
        if (respHeader) respHeader.removeAttribute('hidden');
        if (respStatus) {
          respStatus.textContent = `${h.status} ${statusText(h.status)}`;
          respStatus.className = h.status >= 200 && h.status < 300 ? 'at-resp-status--ok' : 'at-resp-status--err';
        }
        if (respTime) respTime.textContent = `${h.elapsed}ms`;
        if (respCode) {
          respCode.textContent = h.response;
          respCode.className = 'language-json';
          if (window.hljs) hljs.highlightElement(respCode);
        }
      });
    });
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function esc(s) {
  return String(s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;')
    .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function statusText(code) {
  const map = {
    0: 'Network Error', 200: 'OK', 201: 'Created', 204: 'No Content',
    400: 'Bad Request', 401: 'Unauthorized', 403: 'Forbidden',
    404: 'Not Found', 408: 'Timeout', 409: 'Conflict',
    422: 'Unprocessable', 429: 'Rate Limited', 500: 'Internal Error',
    502: 'Bad Gateway', 503: 'Unavailable',
  };
  return map[code] || '';
}

// ── Module entry point ────────────────────────────────────────────────────────

export function init(pane) {
  new ApiTesterPanel(pane);
}
