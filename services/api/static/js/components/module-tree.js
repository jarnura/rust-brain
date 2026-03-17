/**
 * module-tree.js — Module Tree panel component
 *
 * Crate name input → Load button → api.getModuleTree()
 * Renders a collapsible <details> tree of modules and their items.
 * Kind icons on each item.  Click item → window.playground.showDetail()
 * Expand All / Collapse All buttons.
 */

import { apiClient } from '../lib/api-client.js';

// ── Helpers ─────────────────────────────────────────────────────────────────

function escHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

const KIND_ICONS = {
  fn: 'ƒ', function: 'ƒ',
  struct: '⬡', enum: '∈',
  trait: '◈', type: 'T', typealias: 'T',
  mod: '⊟', module: '⊟',
  const: '∁', static: '∫',
  macro: '!', impl: '◇', use: '→',
};

function kindIcon(kind) {
  return KIND_ICONS[String(kind ?? '').toLowerCase()] ?? '·';
}

function buildDetailHtml(d) {
  if (!d) return '<p>No detail available.</p>';
  const code = d.source ?? d.body ?? d.code ?? '';
  return `
    <dl class="detail-meta">
      ${d.fqn   ? `<dt>FQN</dt><dd>${escHtml(d.fqn)}</dd>` : ''}
      ${d.kind  ? `<dt>Kind</dt><dd>${escHtml(d.kind)}</dd>` : ''}
      ${d.file  ? `<dt>File</dt><dd>${escHtml(d.file)}${d.line ? ':' + d.line : ''}</dd>` : ''}
      ${d.crate ? `<dt>Crate</dt><dd>${escHtml(d.crate)}</dd>` : ''}
    </dl>
    ${code ? `<pre><code class="language-rust">${escHtml(code)}</code></pre>` : ''}
  `;
}

// ── Tree renderers ───────────────────────────────────────────────────────────

function renderItems(items) {
  if (!items?.length) return '';
  return `<ul class="tree-items">${
    items.map(item => {
      const fqn  = item.fqn  ?? item.name ?? '(unknown)';
      const kind = item.kind ?? '';
      const name = item.name ?? fqn.split('::').pop();
      return `
        <li class="tree-item" data-fqn="${escHtml(fqn)}" data-kind="${escHtml(kind)}">
          <span class="kind-icon" title="${escHtml(kind)}">${kindIcon(kind)}</span>
          <span class="item-name">${escHtml(name)}</span>
          <span class="item-fqn dimmed">${escHtml(fqn)}</span>
        </li>`;
    }).join('')
  }</ul>`;
}

function renderNode(node, depth = 0) {
  const name     = node.name ?? node.path?.split('::').pop() ?? '(module)';
  const path     = node.path ?? node.fqn ?? '';
  const items    = node.items    ?? [];
  const children = node.children ?? node.modules ?? node.submodules ?? [];

  const badge = items.length
    ? `<span class="node-badge">${items.length}</span>`
    : '';

  return `
    <details class="tree-node" ${depth < 2 ? 'open' : ''}>
      <summary class="tree-node__summary">
        <span class="kind-icon">⊟</span>
        <span class="node-name">${escHtml(name)}</span>
        ${path ? `<span class="node-path dimmed">${escHtml(path)}</span>` : ''}
        ${badge}
      </summary>
      ${renderItems(items)}
      ${children.map(c => renderNode(c, depth + 1)).join('')}
    </details>`;
}

// ── init ─────────────────────────────────────────────────────────────────────

export function init(pane) {
  pane.innerHTML = `
    <div class="component-panel">
      <h2 class="panel-title">Module Tree</h2>
      <div class="search-bar">
        <input id="module-input" class="search-input" type="text"
               placeholder="Crate name — e.g. rust_brain" autocomplete="off" />
        <button id="module-load" class="btn btn-primary">Load</button>
        <button id="module-expand-all"   class="btn btn-ghost" hidden>Expand all</button>
        <button id="module-collapse-all" class="btn btn-ghost" hidden>Collapse all</button>
      </div>
      <div id="module-status" class="status-text"></div>
      <div id="module-tree"   class="tree-container"></div>
    </div>
  `;

  const input       = pane.querySelector('#module-input');
  const loadBtn     = pane.querySelector('#module-load');
  const expandBtn   = pane.querySelector('#module-expand-all');
  const collapseBtn = pane.querySelector('#module-collapse-all');
  const statusEl    = pane.querySelector('#module-status');
  const treeEl      = pane.querySelector('#module-tree');

  // ── Expand / Collapse all ─────────────────────────────────────────────────

  expandBtn.addEventListener('click', () => {
    treeEl.querySelectorAll('details').forEach(d => d.setAttribute('open', ''));
  });

  collapseBtn.addEventListener('click', () => {
    treeEl.querySelectorAll('details').forEach(d => d.removeAttribute('open'));
  });

  // ── Item click (event delegation — registered once) ───────────────────────

  treeEl.addEventListener('click', async e => {
    const item = e.target.closest('.tree-item');
    if (!item) return;
    const fqn = item.dataset.fqn;
    if (!fqn) return;
    item.classList.add('loading');
    try {
      const detail = await apiClient.getFunction(fqn);
      window.playground.showDetail(fqn, buildDetailHtml(detail));
    } catch (err) {
      window.playground.showDetail(fqn,
        `<p class="error-msg">${escHtml(err.message)}</p>`);
    } finally {
      item.classList.remove('loading');
    }
  });

  // ── Load ──────────────────────────────────────────────────────────────────

  async function load() {
    const crate = input.value.trim();
    if (!crate) return;

    loadBtn.disabled     = true;
    statusEl.textContent = 'Loading…';
    treeEl.innerHTML     = '';
    expandBtn.setAttribute('hidden', '');
    collapseBtn.setAttribute('hidden', '');

    try {
      const data = await apiClient.getModuleTree(crate);

      // Normalise: root node, array of roots, or raw data
      const roots = data.root   ? [data.root]
                  : data.tree   ? [data.tree]
                  : Array.isArray(data) ? data
                  : [data];

      treeEl.innerHTML = roots.map(n => renderNode(n)).join('');

      const nodeCount = treeEl.querySelectorAll('.tree-node').length;
      const itemCount = treeEl.querySelectorAll('.tree-item').length;
      statusEl.textContent = `${nodeCount} module(s), ${itemCount} item(s)`;

      expandBtn.removeAttribute('hidden');
      collapseBtn.removeAttribute('hidden');
    } catch (err) {
      statusEl.textContent = 'Error';
      treeEl.innerHTML = `<p class="error-msg">${escHtml(err.message)}</p>`;
    } finally {
      loadBtn.disabled = false;
    }
  }

  loadBtn.addEventListener('click', load);
  input.addEventListener('keydown', e => { if (e.key === 'Enter') load(); });
}
