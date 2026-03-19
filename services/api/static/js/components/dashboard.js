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

// ── Fetch helpers ─────────────────────────────────────────────────────────────
let pane, timer;

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

async function refresh() {
  await Promise.all([fetchServices(), fetchStats()]);
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

  pane.querySelector('#dash-refresh').addEventListener('click', refresh);

  refresh();
  timer = setInterval(refresh, 30_000);

  document.addEventListener('playground:tab-change', ({ detail: { tab } }) => {
    if (tab === 'dashboard') {
      if (!timer) timer = setInterval(refresh, 30_000);
    } else {
      clearInterval(timer);
      timer = null;
    }
  });
}
