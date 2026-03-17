/**
 * audit.js — Audit trail panel
 * Exports: init(pane)
 * Shows filterable, paginated audit log (sample data; no live API yet).
 */

const PAGE_SIZE = 10;

// ── Styles ────────────────────────────────────────────────────────────────────
const CSS = `
.audit-pane{padding:var(--space-4);overflow-y:auto;height:100%}
.audit-filters{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);
  padding:var(--space-4);margin-bottom:var(--space-4);display:flex;flex-wrap:wrap;gap:var(--space-3);align-items:flex-end}
.audit-fg{display:flex;flex-direction:column;gap:4px}
.audit-fg label{font-size:var(--font-size-xs);color:var(--text-secondary)}
.audit-fg input,.audit-fg select{background:var(--bg-overlay);border:1px solid var(--border-default);
  border-radius:var(--radius-sm);padding:4px 8px;color:var(--text-primary);font-size:var(--font-size-sm);outline:none}
.audit-fg input:focus,.audit-fg select:focus{border-color:var(--accent-blue)}
.audit-fg input{min-width:200px}
.audit-clear-btn{padding:4px 10px;border-radius:var(--radius-sm);border:1px solid var(--border-default);
  background:var(--bg-elevated);color:var(--text-secondary);font-size:var(--font-size-xs);cursor:pointer;align-self:flex-end}
.audit-clear-btn:hover{background:var(--bg-overlay);color:var(--text-primary)}
.audit-stats{display:grid;grid-template-columns:repeat(4,1fr);gap:var(--space-3);margin-bottom:var(--space-4)}
@media(max-width:500px){.audit-stats{grid-template-columns:repeat(2,1fr)}}
.audit-stat{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);
  padding:var(--space-3);display:flex;justify-content:space-between;align-items:center}
.audit-stat__label{font-size:var(--font-size-xs);color:var(--text-secondary)}
.audit-stat__val{font-size:var(--font-size-lg);font-weight:600;color:var(--text-primary)}
.audit-table{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);overflow:hidden}
.audit-table__hdr{padding:var(--space-3) var(--space-4);border-bottom:1px solid var(--border-muted);
  display:flex;justify-content:space-between;align-items:center}
.audit-table__title{font-size:var(--font-size-md);font-weight:600;color:var(--text-primary)}
.audit-entry{padding:var(--space-3) var(--space-4);border-bottom:1px solid var(--border-muted);cursor:pointer}
.audit-entry:last-child{border-bottom:none}
.audit-entry:hover{background:var(--bg-hover)}
.audit-entry__row{display:flex;justify-content:space-between;align-items:flex-start;gap:var(--space-3)}
.audit-entry__meta{display:flex;gap:var(--space-2);align-items:center;margin-bottom:4px}
.audit-badge{padding:1px 6px;border-radius:var(--radius-sm);font-size:10px;font-weight:700}
.audit-badge--ingest{background:rgba(63,185,80,0.2);color:var(--accent-green)}
.audit-badge--query{background:rgba(88,166,255,0.2);color:var(--accent-blue)}
.audit-badge--error{background:rgba(248,81,73,0.2);color:var(--accent-red)}
.audit-badge--system{background:rgba(188,140,255,0.2);color:var(--accent-purple)}
.audit-entry__op{font-size:var(--font-size-sm);font-weight:500;color:var(--text-primary)}
.audit-entry__detail{font-size:var(--font-size-xs);color:var(--text-secondary);margin-top:2px}
.audit-entry__ts{text-align:right;font-size:var(--font-size-xs);color:var(--text-muted);white-space:nowrap;flex-shrink:0}
.audit-details{margin-top:var(--space-2);padding:var(--space-3);background:var(--bg-base);
  border-radius:var(--radius-sm);display:none}
.audit-details.open{display:block}
.audit-details pre{font-size:var(--font-size-xs);color:var(--text-secondary);overflow:auto;max-height:120px;margin:0}
.audit-pagination{display:flex;justify-content:center;flex-wrap:wrap;gap:var(--space-2);padding:var(--space-3)}
.audit-page-btn{padding:4px 10px;border-radius:var(--radius-sm);border:1px solid var(--border-default);
  background:var(--bg-elevated);color:var(--text-secondary);font-size:var(--font-size-xs);cursor:pointer}
.audit-page-btn:hover{background:var(--bg-overlay)}
.audit-page-btn.active{background:var(--bg-active);color:var(--accent-blue);border-color:var(--border-accent)}
.audit-empty{text-align:center;padding:var(--space-6);color:var(--text-muted);font-size:var(--font-size-sm)}
`;

function injectStyles() {
  if (document.getElementById('audit-styles')) return;
  const s = document.createElement('style');
  s.id = 'audit-styles';
  s.textContent = CSS;
  document.head.appendChild(s);
}

// ── Sample data ───────────────────────────────────────────────────────────────
function buildSampleData() {
  const now = Date.now();
  return [
    { id: 1,  type: 'system', operation: 'System Startup',         details: 'All services initialized successfully',           timestamp: new Date(now - 864e5).toISOString(),  duration: null,  status: 'success', metadata: null },
    { id: 2,  type: 'ingest', operation: 'Repository Ingestion',   details: '42 files processed, 384 items extracted',         timestamp: new Date(now - 828e5).toISOString(),  duration: 45230, status: 'success', metadata: { files: 42, items: 384, errors: 0 } },
    { id: 3,  type: 'query',  operation: 'Semantic Search',        details: 'Query: "serialize function" — 5 results',         timestamp: new Date(now - 72e5).toISOString(),   duration: 156,   status: 'success', metadata: { query: 'serialize function', results: 5 } },
    { id: 4,  type: 'query',  operation: 'Graph Query',            details: 'MATCH (n:Function) RETURN n LIMIT 10',            timestamp: new Date(now - 36e5).toISOString(),   duration: 89,    status: 'success', metadata: null },
    { id: 5,  type: 'error',  operation: 'API Error',              details: 'Failed to connect to Neo4j: Connection refused',  timestamp: new Date(now - 18e5).toISOString(),   duration: null,  status: 'error',   metadata: { code: 'NEO4J_CONNECTION_ERROR', retries: 3 } },
    { id: 6,  type: 'query',  operation: 'Get Callers',            details: 'serde::ser::to_string — 3 callers found',         timestamp: new Date(now - 9e5).toISOString(),    duration: 45,    status: 'success', metadata: { fqn: 'serde::ser::to_string', callers: 3 } },
    { id: 7,  type: 'ingest', operation: 'Partial Ingestion',      details: '15 files processed, 3 errors',                   timestamp: new Date(now - 6e5).toISOString(),    duration: 12000, status: 'partial', metadata: { files: 15, items: 120, errors: 3 } },
    { id: 8,  type: 'query',  operation: 'Trait Implementations',  details: 'Trait: Serialize — 12 implementations found',    timestamp: new Date(now - 3e5).toISOString(),    duration: 78,    status: 'success', metadata: { trait: 'Serialize', impls: 12 } },
    { id: 9,  type: 'system', operation: 'Health Check',           details: 'All dependencies healthy',                       timestamp: new Date(now - 6e4).toISOString(),    duration: 234,   status: 'success', metadata: { postgres: 'healthy', neo4j: 'healthy', qdrant: 'healthy' } },
    { id: 10, type: 'query',  operation: 'Module Tree',            details: 'Crate: serde — module tree generated',           timestamp: new Date(now - 3e4).toISOString(),    duration: 56,    status: 'success', metadata: { crate: 'serde', modules: 24 } },
  ];
}

function formatTs(iso) {
  const diff = Date.now() - new Date(iso).getTime();
  if (diff < 6e4)   return 'just now';
  if (diff < 36e5)  return `${Math.floor(diff / 6e4)}m ago`;
  if (diff < 864e5) return `${Math.floor(diff / 36e5)}h ago`;
  return new Date(iso).toLocaleDateString();
}

// ── Module state ──────────────────────────────────────────────────────────────
let pane, allData = [], filtered = [], currentPage = 1;

function applyFilters() {
  const search = pane.querySelector('#aud-search').value.toLowerCase();
  const type   = pane.querySelector('#aud-type').value;
  const time   = pane.querySelector('#aud-time').value;
  const now    = Date.now();
  const cutoffs = { '1h': 36e5, '24h': 864e5, '7d': 6048e5 };

  filtered = allData.filter(e => {
    if (type !== 'all' && e.type !== type) return false;
    if (search && !e.operation.toLowerCase().includes(search) &&
        !e.details.toLowerCase().includes(search)) return false;
    if (time !== 'all' && now - new Date(e.timestamp).getTime() > cutoffs[time]) return false;
    return true;
  });

  currentPage = 1;
  render();
}

function render() {
  pane.querySelector('#aud-total').textContent   = filtered.length;
  pane.querySelector('#aud-queries').textContent = filtered.filter(e => e.type === 'query').length;
  pane.querySelector('#aud-ingests').textContent = filtered.filter(e => e.type === 'ingest').length;
  pane.querySelector('#aud-errors').textContent  = filtered.filter(e => e.type === 'error').length;

  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const start = (currentPage - 1) * PAGE_SIZE;
  const page  = filtered.slice(start, start + PAGE_SIZE);
  const list  = pane.querySelector('#aud-list');

  if (page.length === 0) {
    list.innerHTML = '<div class="audit-empty">No audit entries found</div>';
  } else {
    list.innerHTML = page.map(e => {
      const statusMark  = e.status === 'success' ? '✓' : e.status === 'error' ? '✗' : '⚠';
      const statusColor = e.status === 'success' ? 'var(--accent-green)'
                        : e.status === 'error'   ? 'var(--accent-red)'
                        : 'var(--accent-yellow)';
      const detailBlock = e.metadata
        ? `<div class="audit-details" id="aud-det-${e.id}"><pre>${JSON.stringify(e.metadata, null, 2)}</pre></div>`
        : '';
      return `
        <div class="audit-entry" data-id="${e.id}">
          <div class="audit-entry__row">
            <div>
              <div class="audit-entry__meta">
                <span class="audit-badge audit-badge--${e.type}">${e.type.toUpperCase()}</span>
                <span class="audit-entry__op">${e.operation}</span>
                <span style="color:${statusColor}">${statusMark}</span>
              </div>
              <div class="audit-entry__detail">${e.details}</div>
              ${detailBlock}
            </div>
            <div class="audit-entry__ts">
              <div>${formatTs(e.timestamp)}</div>
              ${e.duration != null ? `<div>${e.duration}ms</div>` : ''}
            </div>
          </div>
        </div>`;
    }).join('');
  }

  pane.querySelector('#aud-pages').innerHTML = Array.from({ length: pages }, (_, i) => i + 1)
    .map(p => `<button class="audit-page-btn${p === currentPage ? ' active' : ''}" data-page="${p}">${p}</button>`)
    .join('');
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function init(paneEl) {
  injectStyles();
  pane = paneEl;

  pane.innerHTML = `
    <div class="audit-pane">
      <div class="audit-filters">
        <div class="audit-fg">
          <label>Search</label>
          <input id="aud-search" placeholder="Search operations…">
        </div>
        <div class="audit-fg">
          <label>Type</label>
          <select id="aud-type">
            <option value="all">All Types</option>
            <option value="ingest">Ingest</option>
            <option value="query">Query</option>
            <option value="error">Error</option>
            <option value="system">System</option>
          </select>
        </div>
        <div class="audit-fg">
          <label>Time Range</label>
          <select id="aud-time">
            <option value="all">All Time</option>
            <option value="1h">Last Hour</option>
            <option value="24h">Last 24h</option>
            <option value="7d">Last 7 Days</option>
          </select>
        </div>
        <button id="aud-clear" class="audit-clear-btn">Clear</button>
      </div>
      <div class="audit-stats">
        <div class="audit-stat"><span class="audit-stat__label">Total</span><span id="aud-total" class="audit-stat__val">0</span></div>
        <div class="audit-stat"><span class="audit-stat__label">Queries</span><span id="aud-queries" class="audit-stat__val" style="color:var(--accent-blue)">0</span></div>
        <div class="audit-stat"><span class="audit-stat__label">Ingestions</span><span id="aud-ingests" class="audit-stat__val" style="color:var(--accent-green)">0</span></div>
        <div class="audit-stat"><span class="audit-stat__label">Errors</span><span id="aud-errors" class="audit-stat__val" style="color:var(--accent-red)">0</span></div>
      </div>
      <div class="audit-table">
        <div class="audit-table__hdr">
          <span class="audit-table__title">Audit Log</span>
        </div>
        <div id="aud-list"></div>
        <div id="aud-pages" class="audit-pagination"></div>
      </div>
    </div>`;

  allData  = buildSampleData();
  filtered = [...allData];

  pane.querySelector('#aud-search').addEventListener('input',  applyFilters);
  pane.querySelector('#aud-type').addEventListener('change',   applyFilters);
  pane.querySelector('#aud-time').addEventListener('change',   applyFilters);

  pane.querySelector('#aud-clear').addEventListener('click', () => {
    pane.querySelector('#aud-search').value = '';
    pane.querySelector('#aud-type').value   = 'all';
    pane.querySelector('#aud-time').value   = 'all';
    filtered = [...allData];
    currentPage = 1;
    render();
  });

  pane.querySelector('#aud-list').addEventListener('click', e => {
    const entry = e.target.closest('.audit-entry');
    if (!entry) return;
    const det = entry.querySelector('.audit-details');
    if (det) det.classList.toggle('open');
  });

  pane.querySelector('#aud-pages').addEventListener('click', e => {
    const btn = e.target.closest('[data-page]');
    if (!btn) return;
    currentPage = parseInt(btn.dataset.page, 10);
    render();
  });

  render();
}
