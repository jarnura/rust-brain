/**
 * function-detail.js — FunctionDetailPanel for the right-pane detail view.
 * Listens for state:functionDetail, fetches data, and renders into #detail-body.
 * Emits: detail:show_in_graph, detail:ask_ai
 */
import { bus }       from '../lib/event-bus.js';
import { state }     from '../lib/state.js';
import { apiClient } from '../lib/api-client.js';

// ── Constants ──────────────────────────────────────────────────────────────

const TABS = ['Signature', 'Docs', 'Source', 'Callers', 'Callees'];

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

// ── FunctionDetailPanel ────────────────────────────────────────────────────

class FunctionDetailPanel {
  constructor() {
    this._titleEl  = document.getElementById('detail-title');
    this._bodyEl   = document.getElementById('detail-body');
    this._shell    = document.getElementById('app-shell');
    this._activeTab = 'Signature';
    this._data      = null;
    this._fqn       = null;

    bus.on('state:functionDetail', ({ next: fqn }) => {
      if (fqn) this._load(fqn);
    });

    // "Show in Call Graph" button → switch to callgraph tab and load the FQN
    bus.on('detail:show_in_graph', ({ fqn }) => {
      // Switch to the callgraph tab (triggers lazy-init if first time)
      const tabBtn = document.querySelector('.sidebar__item[data-tab="callgraph"]')
                  || document.querySelector('.tab-bar__tab[data-tab="callgraph"]');
      if (tabBtn) tabBtn.click();
      // Wait for component to initialize, then set FQN and load
      const tryLoad = (attempts) => {
        const input = document.getElementById('cg-fqn');
        if (input) {
          input.value = fqn;
          document.getElementById('cg-load')?.click();
        } else if (attempts > 0) {
          setTimeout(() => tryLoad(attempts - 1), 200);
        }
      };
      setTimeout(() => tryLoad(5), 300);
    });

    // "Ask AI" button → switch to chat tab and pre-fill a question
    bus.on('detail:ask_ai', ({ fqn, fn: fnData }) => {
      const tabBtn = document.querySelector('.sidebar__item[data-tab="chat"]')
                  || document.querySelector('.tab-bar__tab[data-tab="chat"]');
      if (tabBtn) tabBtn.click();
      const tryFill = (attempts) => {
        const input = document.querySelector('.chat-input');
        if (input) {
          const sig = fnData?.signature || '';
          input.value = `Explain the function \`${fqn}\`${sig ? ': `' + sig.trim() + '`' : ''}`;
          input.focus();
          input.dispatchEvent(new Event('input'));
        } else if (attempts > 0) {
          setTimeout(() => tryFill(attempts - 1), 200);
        }
      };
      setTimeout(() => tryFill(5), 300);
    });
  }

  // ── Load ────────────────────────────────────────────────────────────────

  async _load(fqn) {
    this._fqn       = fqn;
    this._activeTab = 'Signature';
    this._titleEl.textContent = fqn.split('::').pop() || fqn;
    this._bodyEl.innerHTML    = '<p class="detail-panel__loading">Loading…</p>';
    this._shell.classList.remove('detail-hidden');

    try {
      const fnData = await apiClient.getFunction(fqn).catch(() => null);
      this._data = { fn: fnData || {}, callers: fnData?.callers || [] };
      this._render();
    } catch (err) {
      this._bodyEl.innerHTML = `<p class="detail-panel__error">Failed to load: ${this._esc(err.message)}</p>`;
    }
  }

  // ── Render ──────────────────────────────────────────────────────────────

  _render() {
    const fn      = this._data.fn;
    const callers = this._data.callers;

    const parts       = (this._fqn || '').split('::');
    const breadcrumb  = parts.map((seg, i) => {
      const partial = parts.slice(0, i + 1).join('::');
      return i < parts.length - 1
        ? `<span class="fd-bc__seg fd-bc__seg--link" data-fqn="${this._esc(partial)}">${this._esc(seg)}</span>`
        : `<span class="fd-bc__seg fd-bc__seg--active">${this._esc(seg)}</span>`;
    }).join('<span class="fd-bc__sep">::</span>');

    const kind      = (fn.kind || 'fn').toLowerCase();
    const vis       = fn.visibility || 'pub';
    const file      = fn.file || fn.source_file || '';
    const line      = fn.line ?? fn.line_number ?? '';
    const color     = KIND_COLORS[kind] || 'var(--text-secondary)';
    const locLabel  = file ? `${this._shortPath(file)}${line !== '' ? ':' + line : ''}` : '';

    this._bodyEl.innerHTML = `
      <div class="fd">
        <div class="fd-bc" role="navigation" aria-label="Breadcrumb">${breadcrumb}</div>

        <div class="fd__meta">
          <span class="fd__kind" style="color:${color};border-color:${color}">${kind}</span>
          <span class="fd__vis">${this._esc(vis)}</span>
          ${locLabel ? `<span class="fd__loc" title="${this._esc(file)}${line !== '' ? ':' + line : ''}">${this._esc(locLabel)}</span>` : ''}
        </div>

        <div class="fd__tab-bar" role="tablist">
          ${TABS.map(t => `
            <button class="fd__tab${t === this._activeTab ? ' active' : ''}"
                    role="tab" data-fdtab="${t}" aria-selected="${t === this._activeTab}">${t}</button>
          `).join('')}
        </div>

        <div class="fd__tab-content" id="fd-content">
          ${this._tabContent(this._activeTab, fn, callers)}
        </div>

        <div class="fd__actions">
          <button class="fd__btn fd__btn--graph" id="fd-show-graph" type="button">Show in Call Graph</button>
          <button class="fd__btn fd__btn--ai"    id="fd-ask-ai"     type="button">Ask AI</button>
        </div>
      </div>
    `;

    this._attachEvents(fn, callers);
    this._highlight();
  }

  // ── Tab content ─────────────────────────────────────────────────────────

  _tabContent(tab, fn, callers) {
    switch (tab) {
      case 'Signature': return this._signature(fn);
      case 'Docs':      return this._docs(fn);
      case 'Source':    return this._source(fn);
      case 'Callers':   return this._callers(callers);
      case 'Callees':   return this._callees(fn);
      default:          return '';
    }
  }

  _signature(fn) {
    const sig = fn.signature || fn.fn_signature || fn.definition || '';
    if (!sig) return '<p class="fd__empty">No signature available.</p>';
    return `<pre class="fd__code"><code class="language-rust">${this._esc(sig)}</code></pre>`;
  }

  _docs(fn) {
    const docs = fn.docstring || fn.docs || fn.doc_comment || fn.documentation || '';
    if (!docs) return '<p class="fd__empty">No documentation available.</p>';
    if (window.marked) {
      return `<div class="fd__docs fd__markdown">${window.marked.parse(docs)}</div>`;
    }
    return `<pre class="fd__docs">${this._esc(docs)}</pre>`;
  }

  _source(fn) {
    const src = fn.body_source || fn.source || fn.source_code || fn.body || '';
    if (!src) return '<p class="fd__empty">Source not available.</p>';
    const start      = Number(fn.start_line ?? fn.line ?? fn.line_number ?? 1);
    const lineCount  = src.split('\n').length;
    const gutterText = Array.from({ length: lineCount }, (_, i) => start + i).join('\n');
    return `
      <div class="fd__source-wrap">
        <pre class="fd__code fd__code--source"><span class="fd__gutter" aria-hidden="true">${gutterText}</span><code class="language-rust">${this._esc(src)}</code></pre>
      </div>`;
  }

  _callers(callers) {
    const list = Array.isArray(callers) ? callers : [];
    if (list.length === 0) return '<p class="fd__empty">No callers found.</p>';
    return `<ul class="fd__ref-list">${list.map(c => this._refItem(c)).join('')}</ul>`;
  }

  _callees(fn) {
    const list = Array.isArray(fn?.callees) ? fn.callees
               : Array.isArray(fn?.calls)   ? fn.calls
               : [];
    if (list.length === 0) return '<p class="fd__empty">No callees found.</p>';
    return `<ul class="fd__ref-list">${list.map(c => this._refItem(c)).join('')}</ul>`;
  }

  _refItem(c) {
    const fqn  = typeof c === 'string' ? c : (c.fqn || c.caller_fqn || c.callee_fqn || c.qualified_name || c.name || String(c));
    const file = typeof c === 'object' ? (c.file || c.source_file || '') : '';
    const line = typeof c === 'object' ? (c.line ?? c.line_number ?? '') : '';
    const name = fqn.split('::').pop() || fqn;
    return `
      <li class="fd__ref-item" data-fqn="${this._esc(fqn)}" role="button" tabindex="0">
        <span class="fd__ref-name">${this._esc(name)}</span>
        <span class="fd__ref-fqn">${this._esc(fqn)}</span>
        ${file ? `<span class="fd__ref-loc">${this._esc(this._shortPath(file))}${line !== '' ? ':' + line : ''}</span>` : ''}
      </li>`;
  }

  // ── Events ──────────────────────────────────────────────────────────────

  _attachEvents(fn, callers) {
    const tabBar  = this._bodyEl.querySelector('.fd__tab-bar');
    const content = this._bodyEl.querySelector('#fd-content');

    tabBar.addEventListener('click', e => {
      const btn = e.target.closest('.fd__tab[data-fdtab]');
      if (!btn) return;
      const tab = btn.dataset.fdtab;
      this._activeTab = tab;
      tabBar.querySelectorAll('.fd__tab').forEach(b => {
        const active = b.dataset.fdtab === tab;
        b.classList.toggle('active', active);
        b.setAttribute('aria-selected', String(active));
      });
      content.innerHTML = this._tabContent(tab, fn, callers);
      this._highlight();
      this._attachRefEvents();
    });

    // Breadcrumb navigation
    this._bodyEl.querySelector('.fd-bc').addEventListener('click', e => {
      const seg = e.target.closest('.fd-bc__seg--link[data-fqn]');
      if (seg) state.set({ functionDetail: seg.dataset.fqn });
    });

    this._bodyEl.querySelector('#fd-show-graph').addEventListener('click', () => {
      bus.emit('detail:show_in_graph', { fqn: this._fqn });
    });

    this._bodyEl.querySelector('#fd-ask-ai').addEventListener('click', () => {
      bus.emit('detail:ask_ai', { fqn: this._fqn, fn });
    });

    this._attachRefEvents();
  }

  _attachRefEvents() {
    this._bodyEl.querySelectorAll('.fd__ref-item[data-fqn]').forEach(el => {
      const go = () => state.set({ functionDetail: el.dataset.fqn });
      el.addEventListener('click',   go);
      el.addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') go(); });
    });
  }

  // ── Helpers ─────────────────────────────────────────────────────────────

  _highlight() {
    if (!window.hljs) return;
    this._bodyEl.querySelectorAll('pre code').forEach(b => hljs.highlightElement(b));
  }

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

// ── Singleton ───────────────────────────────────────────────────────────────

let _panel = null;

/** Initialise once; safe to call multiple times. */
export function init() {
  if (!_panel) _panel = new FunctionDetailPanel();
  return _panel;
}

export default FunctionDetailPanel;
