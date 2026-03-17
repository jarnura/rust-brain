/**
 * gaps.js — Coverage gaps and feature status panel
 * Exports: init(pane)
 * Static data snapshot; no live API dependency.
 */

// ── Styles ────────────────────────────────────────────────────────────────────
const CSS = `
.gaps-pane{padding:var(--space-4);overflow-y:auto;height:100%}
.gaps-summary{display:grid;grid-template-columns:repeat(4,1fr);gap:var(--space-3);margin-bottom:var(--space-6)}
@media(max-width:600px){.gaps-summary{grid-template-columns:repeat(2,1fr)}}
.gaps-sum-card{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);
  padding:var(--space-4);display:flex;align-items:center;gap:var(--space-3)}
.gaps-sum-icon{width:40px;height:40px;border-radius:var(--radius-md);display:flex;align-items:center;
  justify-content:center;font-size:18px;font-weight:700;flex-shrink:0}
.gaps-sum-label{font-size:var(--font-size-xs);color:var(--text-secondary);margin-bottom:2px}
.gaps-sum-value{font-size:22px;font-weight:600}
.gaps-grid{display:grid;grid-template-columns:1fr 1fr;gap:var(--space-6);margin-bottom:var(--space-6)}
@media(max-width:700px){.gaps-grid{grid-template-columns:1fr}}
.gaps-section-title{font-size:var(--font-size-md);font-weight:600;color:var(--text-primary);
  margin:0 0 var(--space-3)}
.gaps-panel{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);overflow:hidden}
.gaps-feature{padding:var(--space-3) var(--space-4);border-bottom:1px solid var(--border-muted)}
.gaps-feature:last-child{border-bottom:none}
.gaps-feature:hover{background:var(--bg-hover)}
.gaps-feature__row{display:flex;justify-content:space-between;align-items:center;gap:var(--space-2)}
.gaps-feature__left{display:flex;align-items:center;gap:var(--space-2)}
.gaps-status-icon{width:22px;height:22px;border-radius:var(--radius-sm);display:flex;align-items:center;
  justify-content:center;font-size:11px;font-weight:700;flex-shrink:0}
.gaps-feature__name{font-size:var(--font-size-sm);font-weight:500;color:var(--text-primary)}
.gaps-feature__cat{font-size:10px;color:var(--text-muted)}
.gaps-feature__note{font-size:var(--font-size-xs);color:var(--text-muted);margin-top:2px;padding-left:30px}
.gaps-badge{padding:1px 7px;border-radius:var(--radius-sm);font-size:10px;font-weight:600}
.gaps-badge--high{background:rgba(248,81,73,0.2);color:var(--accent-red)}
.gaps-badge--medium{background:rgba(210,153,34,0.2);color:var(--accent-yellow)}
.gaps-badge--low{background:rgba(88,166,255,0.2);color:var(--accent-blue)}
.gaps-issue{padding:var(--space-3) var(--space-4);border-bottom:1px solid var(--border-muted);border-left:3px solid transparent}
.gaps-issue:last-child{border-bottom:none}
.gaps-issue:hover{background:var(--bg-hover)}
.gaps-issue__row{display:flex;align-items:center;gap:var(--space-2);margin-bottom:2px}
.gaps-issue__id{font-size:10px;font-family:var(--font-mono);color:var(--text-muted)}
.gaps-issue__title{font-size:var(--font-size-sm);font-weight:500;color:var(--text-primary)}
.gaps-issue__desc{font-size:var(--font-size-xs);color:var(--text-secondary);margin-top:2px}
.gaps-recs{display:grid;grid-template-columns:repeat(3,1fr);gap:var(--space-3);margin-bottom:var(--space-6)}
@media(max-width:700px){.gaps-recs{grid-template-columns:1fr}}
.gaps-rec{background:var(--bg-elevated);border:1px solid var(--border-muted);border-top:2px solid;
  border-radius:var(--radius-md);padding:var(--space-4)}
.gaps-rec__row{display:flex;justify-content:space-between;align-items:flex-start;gap:var(--space-2);margin-bottom:var(--space-2)}
.gaps-rec__title{font-size:var(--font-size-sm);font-weight:600;color:var(--text-primary);flex:1}
.gaps-rec__desc{font-size:var(--font-size-xs);color:var(--text-secondary);margin-bottom:var(--space-2)}
.gaps-rec__effort{font-size:10px;color:var(--text-muted)}
.gaps-phases{background:var(--bg-elevated);border:1px solid var(--border-muted);border-radius:var(--radius-md);padding:var(--space-4)}
.gaps-phase{margin-bottom:var(--space-4)}
.gaps-phase:last-child{margin-bottom:0}
.gaps-phase__row{display:flex;justify-content:space-between;align-items:center;margin-bottom:var(--space-1)}
.gaps-phase__name{font-size:var(--font-size-sm);color:var(--text-primary)}
.gaps-phase__pct{font-size:var(--font-size-xs);color:var(--text-muted)}
.gaps-bar{height:6px;background:var(--bg-overlay);border-radius:3px;overflow:hidden}
.gaps-bar__fill{height:100%;border-radius:3px;transition:width 0.4s ease}
`;

function injectStyles() {
  if (document.getElementById('gaps-styles')) return;
  const s = document.createElement('style');
  s.id = 'gaps-styles';
  s.textContent = CSS;
  document.head.appendChild(s);
}

// ── Static data ───────────────────────────────────────────────────────────────
const FEATURES = [
  { name: 'Docker Infrastructure',  category: 'Infrastructure', status: 'working', note: 'All services running healthy' },
  { name: 'Postgres Database',       category: 'Storage',        status: 'working', note: 'Tables created, connections working' },
  { name: 'Neo4j Graph Database',    category: 'Storage',        status: 'working', note: 'Constraints and indexes created' },
  { name: 'Qdrant Vector Store',     category: 'Storage',        status: 'working', note: 'Collections initialized' },
  { name: 'Ollama LLM Integration',  category: 'AI/ML',          status: 'working', note: 'nomic-embed-text + codellama:7b loaded' },
  { name: 'Tool API Layer',          category: 'API',            status: 'working', note: '9 endpoints implemented, builds successfully' },
  { name: 'Semantic Search',         category: 'Query',          status: 'working', note: 'Embedding + vector search functional' },
  { name: 'Call Graph Queries',      category: 'Query',          status: 'working', note: 'Neo4j traversal working' },
  { name: 'Trait Resolution',        category: 'Query',          status: 'working', note: 'IMPLEMENTS relationship queries' },
  { name: 'Tree-sitter Parser',      category: 'Ingestion',      status: 'partial', note: 'Code generated, needs API fixes' },
  { name: 'Syn Parser',              category: 'Ingestion',      status: 'partial', note: 'Code generated, needs API fixes' },
  { name: 'Type Resolver',           category: 'Ingestion',      status: 'partial', note: 'sqlx 0.8 API compatibility issues' },
  { name: 'Graph Writer',            category: 'Ingestion',      status: 'partial', note: 'neo4rs 0.8 API changes' },
  { name: 'Incremental Ingestion',   category: 'Ingestion',      status: 'broken',  note: 'Not yet implemented' },
  { name: 'Multi-repo Support',      category: 'Advanced',       status: 'broken',  note: 'Schema supports, ingestion pipeline pending' },
];

const ISSUES = [
  { id: 'INGEST-001', sev: 'high',   title: 'sqlx 0.8 try_get API changed',       desc: 'sqlx 0.8 changed try_get signature. Update to new API or downgrade to 0.7.' },
  { id: 'INGEST-002', sev: 'high',   title: 'neo4rs execute method visibility',   desc: 'neo4rs 0.8 changed execute visibility. Use run() method or downgrade.' },
  { id: 'INGEST-003', sev: 'medium', title: 'uuid new_v5 signature changed',      desc: 'Uuid::new_v5 signature changed in newer versions. Update to new API.' },
  { id: 'DOC-001',    sev: 'medium', title: 'API documentation missing',          desc: 'Create docs/api-spec.md with OpenAPI specification.' },
  { id: 'TEST-001',   sev: 'medium', title: 'Integration tests pending',          desc: 'Run integration tests once ingestion is fixed.' },
];

const RECS = [
  { title: 'Fix API Compatibility',     desc: 'Update ingestion service for sqlx 0.8, neo4rs 0.8, and uuid.',  priority: 'high',   effort: '2-4h' },
  { title: 'Add API Documentation',     desc: 'Create OpenAPI specification for Tool API endpoints.',          priority: 'medium', effort: '1-2h' },
  { title: 'Incremental Ingestion',     desc: 'Add support for ingesting only changed files.',                  priority: 'medium', effort: '4-8h' },
  { title: 'Integration Tests',         desc: 'Create end-to-end tests for the ingestion pipeline.',           priority: 'medium', effort: '2-4h' },
  { title: 'Monomorphization Tracking', desc: 'Add concrete type tracking at call sites.',                     priority: 'low',    effort: '8-16h' },
  { title: 'Multi-repo Ingestion',      desc: 'Extend ingestion to support multiple repositories.',            priority: 'low',    effort: '4-8h' },
];

const PHASES = [
  { name: 'Phase 0: Prerequisites',             status: 'passed',  progress: 100 },
  { name: 'Phase 1: Docker Compose — Core',     status: 'passed',  progress: 100 },
  { name: 'Phase 2: Ingestion Pipeline',        status: 'partial', progress: 60  },
  { name: 'Phase 3: Tool API Layer',            status: 'passed',  progress: 100 },
  { name: 'Phase 4: Integration Testing & Docs',status: 'partial', progress: 50  },
];

// ── Config maps ───────────────────────────────────────────────────────────────
const STATUS_CFG = {
  working: { bg: 'rgba(63,185,80,0.15)',  fg: 'var(--accent-green)',  badge: 'rgba(63,185,80,0.2)',  icon: '✓' },
  partial: { bg: 'rgba(210,153,34,0.15)', fg: 'var(--accent-yellow)', badge: 'rgba(210,153,34,0.2)', icon: '⚠' },
  broken:  { bg: 'rgba(248,81,73,0.15)',  fg: 'var(--accent-red)',    badge: 'rgba(248,81,73,0.2)',  icon: '✗' },
};

const SEV_BORDER = {
  high:   'var(--accent-red)',
  medium: 'var(--accent-yellow)',
  low:    'var(--accent-blue)',
};

const PHASE_COLOR = {
  passed:  'var(--accent-green)',
  partial: 'var(--accent-yellow)',
  pending: 'var(--bg-overlay)',
};

// ── Render helpers ────────────────────────────────────────────────────────────
function renderFeatures() {
  return FEATURES.map(f => {
    const c = STATUS_CFG[f.status];
    return `
      <div class="gaps-feature">
        <div class="gaps-feature__row">
          <div class="gaps-feature__left">
            <div class="gaps-status-icon" style="background:${c.bg};color:${c.fg}">${c.icon}</div>
            <div>
              <div class="gaps-feature__name">${f.name}</div>
              <div class="gaps-feature__cat">${f.category}</div>
            </div>
          </div>
          <span class="gaps-badge" style="background:${c.badge};color:${c.fg}">${f.status}</span>
        </div>
        <div class="gaps-feature__note">${f.note}</div>
      </div>`;
  }).join('');
}

function renderIssues() {
  return ISSUES.map(i => `
    <div class="gaps-issue" style="border-left-color:${SEV_BORDER[i.sev]}">
      <div class="gaps-issue__row">
        <span class="gaps-issue__id">${i.id}</span>
        <span class="gaps-badge gaps-badge--${i.sev}">${i.sev}</span>
      </div>
      <div class="gaps-issue__title">${i.title}</div>
      <div class="gaps-issue__desc">${i.desc}</div>
    </div>`).join('');
}

function renderRecs() {
  return RECS.map(r => `
    <div class="gaps-rec" style="border-top-color:${SEV_BORDER[r.priority]}">
      <div class="gaps-rec__row">
        <span class="gaps-rec__title">${r.title}</span>
        <span class="gaps-badge gaps-badge--${r.priority}">${r.priority}</span>
      </div>
      <div class="gaps-rec__desc">${r.desc}</div>
      <div class="gaps-rec__effort">Est. effort: ${r.effort}</div>
    </div>`).join('');
}

function renderPhases() {
  return PHASES.map(p => `
    <div class="gaps-phase">
      <div class="gaps-phase__row">
        <span class="gaps-phase__name">${p.name}</span>
        <span class="gaps-phase__pct">${p.progress}%</span>
      </div>
      <div class="gaps-bar">
        <div class="gaps-bar__fill" style="width:${p.progress}%;background:${PHASE_COLOR[p.status]}"></div>
      </div>
    </div>`).join('');
}

// ── Init ──────────────────────────────────────────────────────────────────────
export function init(pane) {
  injectStyles();

  const working    = FEATURES.filter(f => f.status === 'working').length;
  const partial    = FEATURES.filter(f => f.status === 'partial').length;
  const broken     = FEATURES.filter(f => f.status === 'broken').length;
  const completion = Math.round((working + partial * 0.5) / FEATURES.length * 100);

  pane.innerHTML = `
    <div class="gaps-pane">
      <div class="gaps-summary">
        <div class="gaps-sum-card">
          <div class="gaps-sum-icon" style="background:rgba(63,185,80,0.15);color:var(--accent-green)">✓</div>
          <div><div class="gaps-sum-label">Working</div><div class="gaps-sum-value" style="color:var(--accent-green)">${working}</div></div>
        </div>
        <div class="gaps-sum-card">
          <div class="gaps-sum-icon" style="background:rgba(210,153,34,0.15);color:var(--accent-yellow)">⚠</div>
          <div><div class="gaps-sum-label">Partial</div><div class="gaps-sum-value" style="color:var(--accent-yellow)">${partial}</div></div>
        </div>
        <div class="gaps-sum-card">
          <div class="gaps-sum-icon" style="background:rgba(248,81,73,0.15);color:var(--accent-red)">✗</div>
          <div><div class="gaps-sum-label">Broken</div><div class="gaps-sum-value" style="color:var(--accent-red)">${broken}</div></div>
        </div>
        <div class="gaps-sum-card">
          <div class="gaps-sum-icon" style="background:rgba(88,166,255,0.15);color:var(--accent-blue)">%</div>
          <div><div class="gaps-sum-label">Completion</div><div class="gaps-sum-value" style="color:var(--accent-blue)">${completion}%</div></div>
        </div>
      </div>

      <div class="gaps-grid">
        <div>
          <h2 class="gaps-section-title">Feature Checklist</h2>
          <div class="gaps-panel">${renderFeatures()}</div>
        </div>
        <div>
          <h2 class="gaps-section-title">Known Issues</h2>
          <div class="gaps-panel">${renderIssues()}</div>
        </div>
      </div>

      <h2 class="gaps-section-title">Recommendations</h2>
      <div class="gaps-recs">${renderRecs()}</div>

      <h2 class="gaps-section-title">Phase Progress</h2>
      <div class="gaps-phases">${renderPhases()}</div>
    </div>`;
}
