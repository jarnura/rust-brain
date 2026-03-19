/**
 * search.js — SearchPanel for the rust-brain playground
 * Renders into #pane-search; debounced semantic/aggregate search with kind filtering.
 */
import { apiClient } from '../lib/api-client.js';
import { state }     from '../lib/state.js';
import { init as initDetail } from './function-detail.js';

// ── Constants ──────────────────────────────────────────────────────────────

const KIND_COLORS = {
  fn:       'var(--kind-fn)',
  function: 'var(--kind-fn)',
  struct:   'var(--kind-struct)',
  enum:     'var(--kind-enum)',
  trait:    'var(--kind-trait)',
  mod:      'var(--kind-mod)',
  module:   'var(--kind-mod)',
  type:     'var(--kind-type)',
  impl:     'var(--kind-impl)',
  macro:    'var(--kind-macro)',
};

const FILTERS = [
  { label: 'All',       kind: null     },
  { label: 'Functions', kind: 'fn'     },
  { label: 'Structs',   kind: 'struct' },
  { label: 'Enums',     kind: 'enum'   },
  { label: 'Traits',    kind: 'trait'  },
];

// ── SearchPanel ────────────────────────────────────────────────────────────

class SearchPanel {
  /** @param {HTMLElement} pane */
  constructor(pane) {
    this.pane           = pane;
    this._timer         = null;
    this._activeFilter  = 'All';
    this._includeSource = false;
    this._lastResults   = [];
    this._render();
    this._attach();
  }

  // ── Markup ──────────────────────────────────────────────────────────────

  _render() {
    this.pane.innerHTML = `
      <div class="search-panel">
        <div class="search-panel__toolbar">
          <div class="search-panel__input-row">
            <span class="search-panel__icon" aria-hidden="true">⌕</span>
            <input id="sp-input" class="search-panel__input" type="search"
              placeholder="Search functions, structs, traits…"
              autocomplete="off" spellcheck="false">
            <button id="sp-search-btn" class="btn btn-primary" type="button">Search</button>
            <span id="sp-spinner" class="search-panel__spinner" hidden aria-label="Searching">◌</span>
          </div>
          <div class="search-panel__filter-row">
            <div class="search-panel__chips" id="sp-chips" role="group" aria-label="Filter by kind">
              ${FILTERS.map(f => `
                <button class="sp-chip${f.label === 'All' ? ' active' : ''}"
                        data-kind="${f.label}" type="button">${f.label}</button>
              `).join('')}
            </div>
            <label class="sp-toggle">
              <input id="sp-src-toggle" type="checkbox"> Include source
            </label>
          </div>
        </div>

        <div class="search-panel__meta" id="sp-meta" aria-live="polite"></div>

        <div class="search-panel__results" id="sp-results" role="list">
          <p class="search-panel__hint">Type to search the codebase…</p>
        </div>
      </div>
    `;
  }

  // ── Events ──────────────────────────────────────────────────────────────

  _attach() {
    this._input     = this.pane.querySelector('#sp-input');
    this._searchBtn = this.pane.querySelector('#sp-search-btn');
    this._spinner   = this.pane.querySelector('#sp-spinner');
    this._chips     = this.pane.querySelector('#sp-chips');
    this._meta      = this.pane.querySelector('#sp-meta');
    this._results   = this.pane.querySelector('#sp-results');
    this._srcToggle = this.pane.querySelector('#sp-src-toggle');

    this._searchBtn.addEventListener('click', () => this._doSearch());

    this._input.addEventListener('keydown', e => {
      if (e.key === 'Enter') { e.preventDefault(); this._doSearch(); }
      if (e.key === 'Escape') this._input.blur();
    });

    this._chips.addEventListener('click', e => {
      const chip = e.target.closest('.sp-chip');
      if (!chip) return;
      this._chips.querySelectorAll('.sp-chip').forEach(c => c.classList.remove('active'));
      chip.classList.add('active');
      this._activeFilter = chip.dataset.kind;
      this._applyFilter();
    });

    this._srcToggle.addEventListener('change', () => {
      this._includeSource = this._srcToggle.checked;
      // Re-fetch with source included if toggled on and we have a query
      if (this._input.value.trim()) this._doSearch();
      else this._applyFilter();
    });

    // Auto-focus when the Search tab becomes active
    document.addEventListener('playground:tab-change', ({ detail }) => {
      if (detail.tab === 'search') setTimeout(() => this._input.focus(), 50);
    });
  }

  // ── Search ──────────────────────────────────────────────────────────────

  async _doSearch() {
    const query = this._input.value.trim();
    if (!query) {
      this._meta.textContent = '';
      this._results.innerHTML = '<p class="search-panel__hint">Type to search the codebase…</p>';
      this._lastResults = [];
      return;
    }

    this._spinner.removeAttribute('hidden');
    const t0 = performance.now();

    try {
      const data    = await apiClient.aggregateSearch(query, { limit: 20, includeSource: this._includeSource });
      const elapsed = (performance.now() - t0).toFixed(0);
      this._lastResults = Array.isArray(data?.results) ? data.results
                        : Array.isArray(data)           ? data
                        : [];
      const n = this._lastResults.length;
      this._meta.textContent = `${n} result${n !== 1 ? 's' : ''} — ${elapsed} ms`;
      this._applyFilter();
    } catch (err) {
      this._meta.textContent = '';
      this._results.innerHTML = `<p class="search-panel__error">Error: ${this._esc(err.message)}</p>`;
    } finally {
      this._spinner.setAttribute('hidden', '');
    }
  }

  // ── Filter + render ─────────────────────────────────────────────────────

  _applyFilter() {
    const filter   = FILTERS.find(f => f.label === this._activeFilter) ?? FILTERS[0];
    const KIND_ALIASES = { function: 'fn', module: 'mod', typedef: 'type', typealias: 'type' };
    const filtered = filter.kind
      ? this._lastResults.filter(r => {
          const raw = (r.kind || '').toLowerCase();
          const k = KIND_ALIASES[raw] || raw;
          return k === filter.kind;
        })
      : this._lastResults;

    if (filtered.length === 0) {
      this._results.innerHTML = '<p class="search-panel__hint">No results match the current filter.</p>';
      return;
    }

    this._results.innerHTML = filtered.map(r => this._row(r)).join('');

    if (window.hljs) {
      this._results.querySelectorAll('pre code').forEach(b => hljs.highlightElement(b));
    }

    this._results.querySelectorAll('.sp-result').forEach((el, i) => {
      const activate = () => {
        const fqn = filtered[i].fqn || filtered[i].qualified_name || filtered[i].name || '';
        state.set({ functionDetail: fqn });
        this._results.querySelectorAll('.sp-result').forEach(x => x.classList.remove('selected'));
        el.classList.add('selected');
      };
      el.addEventListener('click',   activate);
      el.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') activate(); });
    });
  }

  // ── Result row HTML ─────────────────────────────────────────────────────

  _row(r) {
    const kind     = (r.kind || 'fn').toLowerCase();
    const name     = this._esc(r.name || r.fqn || '(unknown)');
    const fqn      = this._esc(r.fqn  || r.qualified_name || r.name || '');
    const file     = r.file_path || r.file || r.source_file || '';
    const line     = r.start_line ?? r.line ?? r.line_number ?? '';
    const score    = typeof r.score      === 'number' ? r.score
                   : typeof r.similarity === 'number' ? r.similarity
                   : 0;
    const scorePct = Math.round(Math.min(1, Math.max(0, score)) * 100);
    const color    = KIND_COLORS[kind] || 'var(--text-secondary)';
    const srcText  = r.body_source || r.snippet || r.source_snippet || '';
    const snippet  = this._includeSource && srcText
      ? `<pre class="sp-result__snippet"><code class="language-rust">${this._esc(srcText)}</code></pre>`
      : '';

    return `
      <div class="sp-result" role="listitem button" tabindex="0" aria-label="${name}">
        <div class="sp-result__header">
          <span class="sp-result__kind" style="color:${color};border-color:${color}">${kind}</span>
          <span class="sp-result__name">${name}</span>
          <div class="sp-result__score" title="score ${score.toFixed(3)}">
            <div class="sp-result__score-fill" style="width:${scorePct}%"></div>
          </div>
        </div>
        <div class="sp-result__fqn">${fqn}</div>
        ${file ? `<div class="sp-result__loc">${this._esc(this._shortPath(file))}${line !== '' ? ':' + line : ''}</div>` : ''}
        ${snippet}
      </div>`;
  }

  // ── Helpers ─────────────────────────────────────────────────────────────

  _shortPath(f) {
    const parts = f.replace(/\\/g, '/').split('/');
    return parts.length > 3 ? '…/' + parts.slice(-2).join('/') : f;
  }

  _esc(s) {
    return String(s)
      .replace(/&/g, '&amp;').replace(/</g, '&lt;')
      .replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }
}

// ── Module entry point ──────────────────────────────────────────────────────

/** Called by playground.js when the Search tab is first activated */
export function init(pane) {
  initDetail();          // wire up the right-pane function-detail listener
  new SearchPanel(pane);
}
