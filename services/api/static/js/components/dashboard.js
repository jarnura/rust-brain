/**
 * dashboard.js — System status + statistics panel
 * RETRO-FUTURISTIC edition
 *
 * Exports: init(pane)
 * Auto-refreshes every 30s. Uses apiClient.getHealth() + queryGraph().
 */
import { apiClient } from '../lib/api-client.js';

const SERVICES = [
  { key: 'postgres', label: 'Postgres', icon: '◆' },
  { key: 'neo4j',    label: 'Neo4j',    icon: '⬡' },
  { key: 'qdrant',   label: 'Qdrant',   icon: '◈' },
  { key: 'ollama',   label: 'Ollama',   icon: '◉' },
  { key: 'opencode', label: 'OpenCode', icon: '⊞' },
  { key: 'mcp_sse',  label: 'MCP-SSE',  icon: '⌥' },
  { key: 'litellm',  label: 'LiteLLM',  icon: '△' },
];

const STAT_QUERIES = [
  { id: 'functions', label: 'Functions', color: 'var(--kind-fn)',     query: 'MATCH (n:Function) RETURN count(n) as c' },
  { id: 'structs',   label: 'Structs',   color: 'var(--kind-struct)', query: 'MATCH (n:Struct)   RETURN count(n) as c' },
  { id: 'traits',    label: 'Traits',    color: 'var(--kind-trait)',  query: 'MATCH (n:Trait)    RETURN count(n) as c' },
  { id: 'modules',   label: 'Modules',   color: 'var(--kind-mod)',    query: 'MATCH (n:Module)   RETURN count(n) as c' },
];

// ── Styles ────────────────────────────────────────────────────────────────────
const CSS = `
/* ── Dashboard: Retro-futuristic ── */
.dash-pane {
  padding: var(--space-5);
  overflow-y: auto;
  height: 100%;
}

/* ── Asymmetric hero section ── */
.dash-hero {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--space-4);
  margin-bottom: var(--space-6);
  opacity: 0;
  animation: retro-boot 0.5s ease-out 0.1s forwards;
}

@media (max-width: 700px) {
  .dash-hero { grid-template-columns: 1fr; }
}

.dash-hero__left {
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: var(--space-2);
}

.dash-hero__greeting {
  font-family: var(--font-sans);
  font-size: var(--font-size-3xl);
  font-weight: 700;
  line-height: var(--line-height-heading);
  letter-spacing: var(--letter-spacing-heading);
  color: var(--text-primary);
  background: linear-gradient(135deg, #00f0ff 0%, #8855ff 100%);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}

.dash-hero__tagline {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  color: var(--text-muted);
  letter-spacing: 0.1em;
  text-transform: uppercase;
}

.dash-hero__right {
  display: flex;
  align-items: center;
  justify-content: flex-end;
}

/* ── Diagonal status indicator ── */
.dash-status-badge {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  padding: var(--space-3) var(--space-5);
  background: var(--bg-elevated);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  position: relative;
  overflow: hidden;
}

.dash-status-badge::before {
  content: '';
  position: absolute;
  top: 0;
  left: 0;
  width: 3px;
  height: 100%;
  background: linear-gradient(180deg, var(--accent-green), var(--accent-blue));
}

.dash-status-badge__dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dash-status-badge__dot--healthy {
  background: var(--accent-green);
  box-shadow: 0 0 10px var(--accent-green);
  animation: glow-pulse 2s ease-in-out infinite;
}

.dash-status-badge__dot--degraded {
  background: var(--accent-yellow);
  box-shadow: 0 0 8px var(--accent-yellow);
}

.dash-status-badge__dot--unhealthy {
  background: var(--accent-red);
  box-shadow: 0 0 8px var(--accent-red);
}

.dash-status-badge__text {
  font-family: var(--font-mono);
  font-size: var(--font-size-sm);
  font-weight: 600;
  letter-spacing: 0.06em;
  text-transform: uppercase;
}

/* ── Section headers ── */
.dash-section {
  margin-bottom: var(--space-6);
  opacity: 0;
  animation: retro-boot 0.5s ease-out forwards;
}

.dash-section:nth-child(2) { animation-delay: 0.15s; }
.dash-section:nth-child(3) { animation-delay: 0.25s; }
.dash-section:nth-child(4) { animation-delay: 0.35s; }

.dash-section__hdr {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-4);
  padding-bottom: var(--space-2);
  border-bottom: 1px solid var(--border-muted);
}

.dash-section__title {
  font-family: var(--font-mono);
  font-size: var(--font-size-xs);
  font-weight: 600;
  color: var(--text-muted);
  letter-spacing: 0.12em;
  text-transform: uppercase;
  flex: 1;
  margin: 0;
}

.dash-ts {
  font-size: var(--font-size-xs);
  color: var(--text-muted);
  font-family: var(--font-mono);
}

.dash-btn {
  padding: 4px 12px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-default);
  background: var(--bg-elevated);
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  font-family: var(--font-mono);
  cursor: pointer;
  letter-spacing: 0.04em;
  transition: all 0.2s ease;
}

.dash-btn:hover {
  background: var(--bg-card);
  color: var(--accent-blue);
  border-color: var(--accent-blue);
  box-shadow: 0 0 8px rgba(0, 240, 255, 0.15);
}

/* ── Service cards — Asymmetric grid ── */
.dash-services {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: var(--space-3);
}

@media (max-width: 900px) { .dash-services { grid-template-columns: repeat(3, 1fr); } }
@media (max-width: 600px) { .dash-services { grid-template-columns: repeat(2, 1fr); } }

.dash-svc {
  border-radius: var(--radius-md);
  padding: var(--space-3) var(--space-4);
  border: 1px solid var(--border-muted);
  position: relative;
  overflow: hidden;
  transition: all 0.2s ease;
  /* Staggered entrance */
  opacity: 0;
  animation: retro-boot 0.4s ease-out forwards;
}

.dash-svc:nth-child(1) { animation-delay: 0.2s; }
.dash-svc:nth-child(2) { animation-delay: 0.26s; }
.dash-svc:nth-child(3) { animation-delay: 0.32s; }
.dash-svc:nth-child(4) { animation-delay: 0.38s; }
.dash-svc:nth-child(5) { animation-delay: 0.44s; }
.dash-svc:nth-child(6) { animation-delay: 0.50s; }
.dash-svc:nth-child(7) { animation-delay: 0.56s; }

/* Diagonal corner accent */
.dash-svc::before {
  content: '';
  position: absolute;
  top: 0;
  right: 0;
  width: 24px;
  height: 24px;
  background: linear-gradient(135deg, transparent 50%, rgba(0, 240, 255, 0.06) 50%);
  transition: all 0.2s ease;
}

.dash-svc:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
}

.dash-svc--healthy {
  background: rgba(0, 255, 136, 0.04);
  border-color: rgba(0, 255, 136, 0.2);
}
.dash-svc--healthy::before {
  background: linear-gradient(135deg, transparent 50%, rgba(0, 255, 136, 0.08) 50%);
}

.dash-svc--unhealthy {
  background: rgba(255, 51, 102, 0.04);
  border-color: rgba(255, 51, 102, 0.2);
}
.dash-svc--unhealthy::before {
  background: linear-gradient(135deg, transparent 50%, rgba(255, 51, 102, 0.08) 50%);
}

.dash-svc--unknown {
  background: var(--bg-elevated);
  border-color: var(--border-muted);
}

.dash-svc__row {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 4px;
}

.dash-svc__icon {
  font-size: var(--font-size-md);
  color: var(--text-muted);
  font-family: var(--font-mono);
  margin-right: var(--space-2);
}

.dash-svc__name {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
  font-family: var(--font-mono);
  letter-spacing: 0.02em;
}

.dash-svc__status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dash-svc__status-dot--healthy {
  background: var(--accent-green);
  box-shadow: 0 0 6px var(--accent-green);
}

.dash-svc__status-dot--unhealthy {
  background: var(--accent-red);
  box-shadow: 0 0 6px var(--accent-red);
}

.dash-svc__status-dot--unknown {
  background: var(--text-muted);
}

.dash-svc__sub {
  font-size: var(--font-size-xs);
  color: var(--text-muted);
  font-family: var(--font-mono);
}

/* ── Statistics — bold number display ── */
.dash-stats {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  gap: var(--space-3);
}

@media (max-width: 800px) { .dash-stats { grid-template-columns: repeat(3, 1fr); } }
@media (max-width: 500px) { .dash-stats { grid-template-columns: repeat(2, 1fr); } }

.dash-stat {
  background: var(--bg-elevated);
  border: 1px solid var(--border-muted);
  border-radius: var(--radius-md);
  padding: var(--space-4);
  position: relative;
  overflow: hidden;
  transition: all 0.2s ease;
  /* Staggered */
  opacity: 0;
  animation: retro-boot 0.4s ease-out forwards;
}

.dash-stat:nth-child(1) { animation-delay: 0.3s; }
.dash-stat:nth-child(2) { animation-delay: 0.36s; }
.dash-stat:nth-child(3) { animation-delay: 0.42s; }
.dash-stat:nth-child(4) { animation-delay: 0.48s; }
.dash-stat:nth-child(5) { animation-delay: 0.54s; }

.dash-stat:hover {
  border-color: var(--accent-blue);
  box-shadow: 0 0 12px rgba(0, 240, 255, 0.08);
}

/* Bottom glow line */
.dash-stat::after {
  content: '';
  position: absolute;
  bottom: 0;
  left: 20%;
  right: 20%;
  height: 1px;
  background: linear-gradient(90deg, transparent, var(--accent-blue), transparent);
  opacity: 0;
  transition: opacity 0.2s ease;
}

.dash-stat:hover::after { opacity: 0.5; }

.dash-stat__label {
  font-size: var(--font-size-xs);
  font-family: var(--font-mono);
  letter-spacing: 0.08em;
  text-transform: uppercase;
  margin-bottom: var(--space-2);
}

.dash-stat__value {
  font-size: 28px;
  font-weight: 700;
  color: var(--text-primary);
  font-family: var(--font-mono);
  line-height: 1;
  letter-spacing: -0.02em;
}

.dash-err {
  grid-column: 1 / -1;
  padding: var(--space-3);
  border-radius: var(--radius-md);
  background: rgba(255, 51, 102, 0.06);
  border: 1px solid rgba(255, 51, 102, 0.15);
  color: var(--accent-red);
  font-size: var(--font-size-sm);
  font-family: var(--font-mono);
}

/* ── Ingestion progress ── */
.ingest {
  border-radius: var(--radius-md);
  padding: var(--space-4);
  border: 1px solid var(--border-muted);
  background: var(--bg-elevated);
  position: relative;
  overflow: hidden;
}

/* Diagonal corner cut */
.ingest::before {
  content: '';
  position: absolute;
  top: 0;
  right: 0;
  width: 40px;
  height: 40px;
  background: linear-gradient(135deg, transparent 50%, rgba(0, 240, 255, 0.04) 50%);
}

.ingest--running {
  border-color: rgba(0, 240, 255, 0.3);
  background: rgba(0, 240, 255, 0.03);
}
.ingest--completed {
  border-color: rgba(0, 255, 136, 0.3);
  background: rgba(0, 255, 136, 0.03);
}
.ingest--failed {
  border-color: rgba(255, 51, 102, 0.3);
  background: rgba(255, 51, 102, 0.03);
}

.ingest__header {
  display: flex;
  align-items: center;
  gap: var(--space-3);
  margin-bottom: var(--space-3);
}

.ingest__status {
  font-size: var(--font-size-sm);
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  font-family: var(--font-mono);
}
.ingest__status--running   { color: var(--accent-blue); text-shadow: 0 0 8px rgba(0, 240, 255, 0.3); }
.ingest__status--completed { color: var(--accent-green); }
.ingest__status--failed    { color: var(--accent-red); }

.ingest__meta {
  display: flex;
  gap: var(--space-4);
  flex-wrap: wrap;
  margin-bottom: var(--space-3);
  font-size: var(--font-size-xs);
  color: var(--text-muted);
  font-family: var(--font-mono);
}
.ingest__meta-item { display: flex; align-items: center; gap: var(--space-1); }
.ingest__meta-value { color: var(--text-primary); font-weight: 600; }

.ingest__progress {
  height: 4px;
  border-radius: 2px;
  background: var(--bg-card);
  overflow: hidden;
  margin-bottom: var(--space-3);
}

.ingest__progress-bar {
  height: 100%;
  border-radius: 2px;
  transition: width 0.4s ease;
}
.ingest__progress-bar--running   { background: linear-gradient(90deg, var(--accent-blue), var(--accent-violet)); box-shadow: 0 0 8px rgba(0, 240, 255, 0.3); }
.ingest__progress-bar--completed { background: var(--accent-green); }
.ingest__progress-bar--failed    { background: var(--accent-red); }

.ingest__stages {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
  gap: var(--space-2);
}

.ingest__stage {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  padding: var(--space-2);
  border-radius: var(--radius-sm);
  background: var(--bg-card);
  border: 1px solid var(--border-muted);
  font-size: var(--font-size-xs);
  font-family: var(--font-mono);
}

.ingest__stage-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.ingest__stage-dot--success { background: var(--accent-green); box-shadow: 0 0 4px var(--accent-green); }
.ingest__stage-dot--running { background: var(--accent-blue);  box-shadow: 0 0 4px var(--accent-blue);  animation: ingest-pulse 1.2s ease-in-out infinite; }
.ingest__stage-dot--failed  { background: var(--accent-red);   box-shadow: 0 0 4px var(--accent-red); }
.ingest__stage-dot--pending { background: var(--text-muted); }

.ingest__stage-name  { color: var(--text-secondary); flex: 1; }
.ingest__stage-count { color: var(--text-primary); font-weight: 600; }

.ingest__none {
  font-size: var(--font-size-sm);
  color: var(--text-muted);
  text-align: center;
  padding: var(--space-4);
  font-family: var(--font-mono);
  letter-spacing: 0.04em;
}

.ingest__live {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-xs);
  color: var(--accent-blue);
  margin-left: auto;
  font-family: var(--font-mono);
  letter-spacing: 0.06em;
}

.ingest__live-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--accent-blue);
  box-shadow: 0 0 6px var(--accent-blue);
  animation: ingest-pulse 1.2s ease-in-out infinite;
}

@keyframes ingest-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
/* glow-pulse is defined in playground.css — do not duplicate here */

/* ── Snapshot badge ── */
.snap-badge {
  margin-top: var(--space-3);
  padding: var(--space-2) var(--space-3);
  background: rgba(0, 240, 255, 0.04);
  border: 1px solid rgba(0, 240, 255, 0.15);
  border-radius: var(--radius-md);
  font-family: var(--font-mono);
}
.snap-badge__row {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  margin-bottom: 2px;
}
.snap-badge__label {
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.1em;
  color: var(--accent-blue);
  background: rgba(0, 240, 255, 0.1);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
}
.snap-badge__version {
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  font-weight: 600;
}
.snap-badge__detail {
  font-size: 10px;
  color: var(--text-muted);
  letter-spacing: 0.02em;
}
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

function svcDotClass(status) {
  if (status === 'healthy')                        return 'dash-svc__status-dot dash-svc__status-dot--healthy';
  if (status === 'unhealthy' || status === 'degraded') return 'dash-svc__status-dot dash-svc__status-dot--unhealthy';
  return 'dash-svc__status-dot dash-svc__status-dot--unknown';
}

function overallStatus(health) {
  const deps = health.dependencies || {};
  const statuses = Object.values(deps).map(d => d.status);
  if (statuses.every(s => s === 'healthy')) return 'healthy';
  if (statuses.some(s => s === 'unhealthy')) return 'unhealthy';
  return 'degraded';
}

function renderServiceCards(health) {
  const deps = health.dependencies || {};
  return SERVICES.map(({ key, label, icon }) => {
    const dep    = deps[key] || {};
    const status = dep.status || 'unknown';
    const sub    = dep.latency_ms != null ? `${dep.latency_ms}ms` : status;
    return `
      <div class="${svcClass(status)}">
        <div class="dash-svc__row">
          <span><span class="dash-svc__icon">${icon}</span><span class="dash-svc__name">${label}</span></span>
          <span class="${svcDotClass(status)}"></span>
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
      <div class="dash-stat__value">${value != null ? Number(value).toLocaleString() : '—'}</div>
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
  if (!data) return '<div class="ingest__none">// no ingestion runs found</div>';

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
        ${status === 'running' ? '<span class="ingest__live"><span class="ingest__live-dot"></span>LIVE</span>' : ''}
      </div>
      <div class="ingest__meta">
        <div class="ingest__meta-item">extracted: <span class="ingest__meta-value">${(data.items_extracted ?? 0).toLocaleString()}</span></div>
        <div class="ingest__meta-item">crates: <span class="ingest__meta-value">${data.crates_processed ?? 0}</span></div>
        <div class="ingest__meta-item">duration: <span class="ingest__meta-value">${dur}</span></div>
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
  const badgeEl = pane.querySelector('#dash-status-badge');
  try {
    const health = await apiClient.getHealth();
    el.innerHTML = renderServiceCards(health);

    // Update hero badge
    if (badgeEl) {
      const overall = overallStatus(health);
      const labels = { healthy: 'ALL SYSTEMS NOMINAL', degraded: 'SYSTEMS DEGRADED', unhealthy: 'SYSTEMS CRITICAL' };
      badgeEl.innerHTML = `
        <span class="dash-status-badge__dot dash-status-badge__dot--${overall}"></span>
        <span class="dash-status-badge__text">${labels[overall] || 'UNKNOWN'}</span>
      `;
    }
  } catch (err) {
    el.innerHTML = `<div class="dash-err">// service status unavailable: ${err.message}</div>`;
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

async function fetchSnapshot() {
  const el = pane.querySelector('#dash-snapshot');
  if (!el) return;
  try {
    const resp = await fetch('/api/snapshot');
    const data = await resp.json();
    if (data.loaded && data.manifest) {
      const m = data.manifest;
      const source = m.source?.name || 'unknown';
      const commit = (m.source?.commit || '').slice(0, 7);
      const items = m.stats?.total_items?.toLocaleString() || '?';
      const model = m.embedding?.model || 'unknown';
      el.innerHTML = `
        <div class="snap-badge">
          <div class="snap-badge__row">
            <span class="snap-badge__label">SNAPSHOT</span>
            <span class="snap-badge__version">v${m.version || '?'}</span>
          </div>
          <div class="snap-badge__detail">${source}${commit ? '@' + commit : ''} // ${items} items // ${model}</div>
        </div>`;
    } else {
      el.innerHTML = '';
    }
  } catch {
    el.innerHTML = '';
  }
}

async function refresh() {
  await Promise.all([fetchServices(), fetchStats(), fetchIngestion(), fetchSnapshot()]);
  const ts = pane.querySelector('#dash-ts');
  if (ts) ts.textContent = `// ${new Date().toLocaleTimeString()}`;
}

// ── Skeleton HTML ─────────────────────────────────────────────────────────────
function skeletonServices() {
  return SERVICES.map(s => `
    <div class="dash-svc dash-svc--unknown">
      <div class="dash-svc__row">
        <span><span class="dash-svc__icon">${s.icon}</span><span class="dash-svc__name">${s.label}</span></span>
        <span class="dash-svc__status-dot dash-svc__status-dot--unknown"></span>
      </div>
      <div class="dash-svc__sub">loading…</div>
    </div>`).join('');
}

function skeletonStats() {
  return [...STAT_QUERIES.map(q => q.label), 'Embeddings'].map(l => `
    <div class="dash-stat">
      <div class="dash-stat__label">${l}</div>
      <div class="dash-stat__value">—</div>
    </div>`).join('');
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function init(paneEl) {
  injectStyles();
  pane = paneEl;

  pane.innerHTML = `
    <div class="dash-pane">
      <!-- Hero: Asymmetric layout -->
      <div class="dash-hero">
        <div class="dash-hero__left">
          <div class="dash-hero__greeting">System<br>Overview</div>
          <div class="dash-hero__tagline">// rust-brain code intelligence</div>
          <div id="dash-snapshot"></div>
        </div>
        <div class="dash-hero__right">
          <div class="dash-status-badge" id="dash-status-badge">
            <span class="dash-status-badge__dot dash-status-badge__dot--degraded"></span>
            <span class="dash-status-badge__text">INITIALIZING…</span>
          </div>
        </div>
      </div>

      <!-- Ingestion -->
      <div class="dash-section">
        <div class="dash-section__hdr">
          <h2 class="dash-section__title">// Ingestion Progress</h2>
        </div>
        <div id="dash-ingestion"><div class="ingest__none">// loading…</div></div>
      </div>

      <!-- Services -->
      <div class="dash-section">
        <div class="dash-section__hdr">
          <h2 class="dash-section__title">// System Status</h2>
          <span id="dash-ts" class="dash-ts"></span>
          <button id="dash-refresh" class="dash-btn">REFRESH</button>
        </div>
        <div id="dash-services" class="dash-services">${skeletonServices()}</div>
      </div>

      <!-- Statistics -->
      <div class="dash-section">
        <div class="dash-section__hdr">
          <h2 class="dash-section__title">// Graph Statistics</h2>
        </div>
        <div id="dash-stats" class="dash-stats">${skeletonStats()}</div>
      </div>
    </div>`;

  pane.querySelector('#dash-refresh').addEventListener('click', () => {
    startIngestionPolling();
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
