/**
 * dashboard.js — System status + statistics panel
 * Exports: init(pane)
 * Auto-refreshes every 30s. Uses apiClient.getHealth() + queryGraph().
 */
import { apiClient } from '../lib/api-client.js';

const SERVICES = [
  { key: 'postgres', label: 'Postgres' },
  { key: 'neo4j',    label: 'Neo4j'    },
  { key: 'qdrant',   label: 'Qdrant'   },
  { key: 'ollama',   label: 'Ollama'   },
  { key: 'opencode', label: 'OpenCode' },
  { key: 'mcp_sse',  label: 'MCP-SSE'  },
  { key: 'litellm',  label: 'LiteLLM'  },
];

const STAT_QUERIES = [
  { id: 'functions', label: 'Functions', color: 'var(--kind-fn)',     query: 'MATCH (n:Function) RETURN count(n) as c' },
  { id: 'structs',   label: 'Structs',   color: 'var(--kind-struct)', query: 'MATCH (n:Struct)   RETURN count(n) as c' },
  { id: 'traits',    label: 'Traits',    color: 'var(--kind-trait)',  query: 'MATCH (n:Trait)    RETURN count(n) as c' },
  { id: 'modules',   label: 'Modules',   color: 'var(--kind-mod)',    query: 'MATCH (n:Module)   RETURN count(n) as c' },
];

// ── Styles ────────────────────────────────────────────────────────────────────
const CSS = `
.dash-pane{padding:var(--space-4);overflow-y:auto;height:100%}
.dash-section{margin-bottom:var(--space-6)}
.dash-section__hdr{display:flex;align-items:center;gap:var(--space-3);margin-bottom:var(--space-3)}
.dash-section__title{font-size:var(--font-size-md);font-weight:600;color:var(--text-primary);flex:1;margin:0}
.dash-ts{font-size:var(--font-size-xs);color:var(--text-muted)}
.dash-btn{padding:4px 10px;border-radius:var(--radius-sm);border:1px solid var(--border-default);
  background:var(--bg-elevated);color:var(--text-secondary);font-size:var(--font-size-xs);cursor:pointer}
.dash-btn:hover{background:var(--bg-overlay);color:var(--text-primary)}
.dash-services{display:grid;grid-template-columns:repeat(7,1fr);gap:var(--space-3)}
@media(max-width:900px){.dash-services{grid-template-columns:repeat(4,1fr)}}
@media(max-width:500px){.dash-services{grid-template-columns:repeat(2,1fr)}}
.dash-svc{border-radius:var(--radius-md);padding:var(--space-3);border:1px solid var(--border-muted)}
.dash-svc--healthy{background:rgba(63,185,80,0.08);border-color:rgba(63,185,80,0.3)}
.dash-svc--unhealthy{background:rgba(248,81,73,0.08);border-color:rgba(248,81,73,0.3)}
.dash-svc--unknown{background:var(--bg-elevated);border-color:var(--border-muted)}
.dash-svc__row{display:flex;justify-content:space-between;align-items:center;margin-bottom:2px}
.dash-svc__name{font-size:var(--font-size-sm);font-weight:500;color:var(--text-primary)}
.dash-svc__sub{font-size:var(--font-size-xs);color:var(--text-muted)}
.dash-stats{display:grid;grid-template-columns:repeat(5,1fr);gap:var(--space-3)}
@media(max-width:700px){.dash-stats{grid-template-columns:repeat(3,1fr)}}
.dash-stat{background:var(--bg-elevated);border:1px solid var(--border-muted);
  border-radius:var(--radius-md);padding:var(--space-4)}
.dash-stat__label{font-size:var(--font-size-xs);color:var(--text-secondary);margin-bottom:var(--space-1)}
.dash-stat__value{font-size:20px;font-weight:600;color:var(--text-primary);font-family:var(--font-mono)}
.dash-err{grid-column:1/-1;padding:var(--space-3);border-radius:var(--radius-md);
  background:rgba(248,81,73,0.08);color:var(--accent-red);font-size:var(--font-size-sm)}
.ingest{border-radius:var(--radius-md);padding:var(--space-4);border:1px solid var(--border-muted);
  background:var(--bg-elevated)}
.ingest--running{border-color:rgba(56,139,253,0.4);background:rgba(56,139,253,0.06)}
.ingest--completed{border-color:rgba(63,185,80,0.4);background:rgba(63,185,80,0.06)}
.ingest--failed{border-color:rgba(248,81,73,0.4);background:rgba(248,81,73,0.06)}
.ingest__header{display:flex;align-items:center;gap:var(--space-3);margin-bottom:var(--space-3)}
.ingest__status{font-size:var(--font-size-sm);font-weight:600;text-transform:uppercase;letter-spacing:0.05em}
.ingest__status--running{color:var(--accent-blue)}
.ingest__status--completed{color:var(--accent-green)}
.ingest__status--failed{color:var(--accent-red)}
.ingest__meta{display:flex;gap:var(--space-4);flex-wrap:wrap;margin-bottom:var(--space-3);
  font-size:var(--font-size-xs);color:var(--text-muted)}
.ingest__meta-item{display:flex;align-items:center;gap:var(--space-1)}
.ingest__meta-value{color:var(--text-primary);font-weight:500;font-family:var(--font-mono)}
.ingest__progress{height:6px;border-radius:3px;background:var(--bg-overlay);overflow:hidden;margin-bottom:var(--space-3)}
.ingest__progress-bar{height:100%;border-radius:3px;transition:width 0.4s ease}
.ingest__progress-bar--running{background:var(--accent-blue)}
.ingest__progress-bar--completed{background:var(--accent-green)}
.ingest__progress-bar--failed{background:var(--accent-red)}
.ingest__stages{display:grid;grid-template-columns:repeat(auto-fill,minmax(180px,1fr));gap:var(--space-2)}
.ingest__stage{display:flex;align-items:center;gap:var(--space-2);padding:var(--space-2);
  border-radius:var(--radius-sm);background:var(--bg-canvas);border:1px solid var(--border-muted);
  font-size:var(--font-size-xs)}
.ingest__stage-dot{width:8px;height:8px;border-radius:50%;flex-shrink:0}
.ingest__stage-dot--success{background:var(--accent-green)}
.ingest__stage-dot--running{background:var(--accent-blue);animation:ingest-pulse 1.2s ease-in-out infinite}
.ingest__stage-dot--failed{background:var(--accent-red)}
.ingest__stage-dot--pending{background:var(--text-muted)}
.ingest__stage-name{color:var(--text-secondary);flex:1}
.ingest__stage-count{color:var(--text-primary);font-family:var(--font-mono);font-weight:500}
.ingest__none{font-size:var(--font-size-sm);color:var(--text-muted);text-align:center;padding:var(--space-4)}
.ingest__live{display:inline-flex;align-items:center;gap:6px;font-size:var(--font-size-xs);color:var(--accent-blue);margin-left:auto}
.ingest__live-dot{width:8px;height:8px;border-radius:50%;background:var(--accent-blue);animation:ingest-pulse 1.2s ease-in-out infinite}
@keyframes ingest-pulse{0%,100%{opacity:1}50%{opacity:0.4}}
`;

function injectStyles() {
  if (document.getElementById('dash-styles')) return;
  const s = document.createElement('style');
  s.id = 'dash-styles';
  s.textContent = CSS;
  document.head.appendChild(s);
}

// ── Helpers ───────────────────────────────────────────────────────────────────
function svcClass(status) {
  if (status === 'healthy')                        return 'dash-svc dash-svc--healthy';
  if (status === 'unhealthy' || status === 'degraded') return 'dash-svc dash-svc--unhealthy';
  return 'dash-svc dash-svc--unknown';
}

function svcDot(status) {
  if (status === 'healthy')                        return '🟢';
  if (status === 'unhealthy' || status === 'degraded') return '🔴';
  return '🟡';
}

function renderServiceCards(health) {
  const deps = health.dependencies || {};
  return SERVICES.map(({ key, label }) => {
    const dep    = deps[key] || {};
    const status = dep.status || 'unknown';
    const sub    = dep.latency_ms != null ? `${dep.latency_ms}ms` : status;
    return `
      <div class="${svcClass(status)}">
        <div class="dash-svc__row">
          <span class="dash-svc__name">${label}</span>
          <span>${svcDot(status)}</span>
        </div>
        <div class="dash-svc__sub">${sub}</div>
      </div>`;
  }).join('');
}

function renderStatCards(counts, embedCount) {
  const items = [
    ...STAT_QUERIES.map(q => ({ label: q.label, color: q.color, value: counts[q.id] })),
    { label: 'Embeddings', color: 'var(--accent-purple)', value: embedCount },
  ];
  return items.map(({ label, color, value }) => `
    <div class="dash-stat">
      <div class="dash-stat__label" style="color:${color}">${label}</div>
      <div class="dash-stat__value">${value ?? '-'}</div>
    </div>`).join('');
}

// ── Ingestion progress helpers ────────────────────────────────────────────────
function statusDot(status) {
  if (status === 'success' || status === 'completed') return 'ingest__stage-dot--success';
  if (status === 'running')  return 'ingest__stage-dot--running';
  if (status === 'failed')   return 'ingest__stage-dot--failed';
  return 'ingest__stage-dot--pending';
}

function formatDuration(startedAt, completedAt) {
  const start = new Date(startedAt);
  const end   = completedAt ? new Date(completedAt) : new Date();
  const secs  = Math.floor((end - start) / 1000);
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  const rem  = secs % 60;
  return `${mins}m ${rem}s`;
}

function computeProgress(stages) {
  if (!stages || stages.length === 0) return 0;
  const done = stages.filter(s => s.status === 'success' || s.status === 'completed').length;
  return Math.round((done / stages.length) * 100);
}

function renderIngestionProgress(data) {
  if (!data) return '<div class="ingest__none">No ingestion runs found</div>';

  const status  = data.status || 'unknown';
  const pct     = status === 'completed' ? 100 : computeProgress(data.stages);
  const dur     = formatDuration(data.started_at, data.completed_at);

  const stagesHtml = (data.stages || []).map(s => `
    <div class="ingest__stage">
      <span class="ingest__stage-dot ${statusDot(s.status)}"></span>
      <span class="ingest__stage-name">${s.name}</span>
      <span class="ingest__stage-count">${s.items_processed.toLocaleString()}</span>
    </div>
  `).join('');

  return `
    <div class="ingest ingest--${status}">
      <div class="ingest__header">
        <span class="ingest__status ingest__status--${status}">${status}</span>
        ${status === 'running' ? '<span class="ingest__live"><span class="ingest__live-dot"></span>Live</span>' : ''}
      </div>
      <div class="ingest__meta">
        <div class="ingest__meta-item">Items extracted: <span class="ingest__meta-value">${(data.items_extracted ?? 0).toLocaleString()}</span></div>
        <div class="ingest__meta-item">Crates: <span class="ingest__meta-value">${data.crates_processed ?? 0}</span></div>
        <div class="ingest__meta-item">Duration: <span class="ingest__meta-value">${dur}</span></div>
      </div>
      <div class="ingest__progress">
        <div class="ingest__progress-bar ingest__progress-bar--${status}" style="width:${pct}%"></div>
      </div>
      ${stagesHtml ? `<div class="ingest__stages">${stagesHtml}</div>` : ''}
    </div>`;
}

// ── Fetch helpers ─────────────────────────────────────────────────────────────
let pane, timer, ingestionTimer;

async function fetchServices() {
  const el = pane.querySelector('#dash-services');
  try {
    const health = await apiClient.getHealth();
    el.innerHTML = renderServiceCards(health);
  } catch (err) {
    el.innerHTML = `<div class="dash-err">Service status unavailable: ${err.message}</div>`;
  }
}

async function fetchStats() {
  const el = pane.querySelector('#dash-stats');
  const counts = {};
  let embedCount = null;

  await Promise.allSettled(
    STAT_QUERIES.map(async ({ id, query }) => {
      try {
        const res  = await apiClient.queryGraph(query, {}, 1);
        const row  = res.results?.[0];
        counts[id] = row != null
          ? (typeof row === 'object' ? Object.values(row)[0] : row)
          : 0;
      } catch {
        counts[id] = null;
      }
    }),
  );

  try {
    const health = await apiClient.getHealth();
    embedCount = health.dependencies?.qdrant?.points_count ?? null;
  } catch { /* leave null */ }

  el.innerHTML = renderStatCards(counts, embedCount);
}

async function fetchIngestion() {
  const el = pane.querySelector('#dash-ingestion');
  if (!el) return;
  try {
    const data = await apiClient.getIngestionProgress();
    el.innerHTML = renderIngestionProgress(data);
    // Stop polling once ingestion is no longer running
    if (data && (data.status === 'completed' || data.status === 'failed')) {
      stopIngestionPolling();
    }
  } catch {
    el.innerHTML = renderIngestionProgress(null);
  }
}

function stopIngestionPolling() {
  if (ingestionTimer) {
    clearInterval(ingestionTimer);
    ingestionTimer = null;
  }
}

function startIngestionPolling() {
  if (!ingestionTimer) {
    ingestionTimer = setInterval(fetchIngestion, 3_000);
  }
}

async function refresh() {
  await Promise.all([fetchServices(), fetchStats(), fetchIngestion()]);
  const ts = pane.querySelector('#dash-ts');
  if (ts) ts.textContent = `Updated ${new Date().toLocaleTimeString()}`;
}

// ── Skeleton HTML ─────────────────────────────────────────────────────────────
function skeletonServices() {
  return SERVICES.map(s => `
    <div class="dash-svc dash-svc--unknown">
      <div class="dash-svc__row">
        <span class="dash-svc__name">${s.label}</span><span>🟡</span>
      </div>
      <div class="dash-svc__sub">loading…</div>
    </div>`).join('');
}

function skeletonStats() {
  return [...STAT_QUERIES.map(q => q.label), 'Embeddings'].map(l => `
    <div class="dash-stat">
      <div class="dash-stat__label">${l}</div>
      <div class="dash-stat__value">-</div>
    </div>`).join('');
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function init(paneEl) {
  injectStyles();
  pane = paneEl;

  pane.innerHTML = `
    <div class="dash-pane">
      <div class="dash-section">
        <div class="dash-section__hdr">
          <h2 class="dash-section__title">Ingestion Progress</h2>
        </div>
        <div id="dash-ingestion"><div class="ingest__none">Loading…</div></div>
      </div>
      <div class="dash-section">
        <div class="dash-section__hdr">
          <h2 class="dash-section__title">System Status</h2>
          <span id="dash-ts" class="dash-ts"></span>
          <button id="dash-refresh" class="dash-btn">Refresh</button>
        </div>
        <div id="dash-services" class="dash-services">${skeletonServices()}</div>
      </div>
      <div class="dash-section">
        <h2 class="dash-section__title" style="margin-bottom:var(--space-3)">Statistics</h2>
        <div id="dash-stats" class="dash-stats">${skeletonStats()}</div>
      </div>
    </div>`;

  pane.querySelector('#dash-refresh').addEventListener('click', () => {
    startIngestionPolling(); // restart polling in case a new run started
    refresh();
  });

  refresh();
  timer = setInterval(refresh, 30_000);
  startIngestionPolling();

  document.addEventListener('playground:tab-change', ({ detail: { tab } }) => {
    if (tab === 'dashboard') {
      if (!timer) timer = setInterval(refresh, 30_000);
      startIngestionPolling();
    } else {
      clearInterval(timer);
      timer = null;
      stopIngestionPolling();
    }
  });
}
