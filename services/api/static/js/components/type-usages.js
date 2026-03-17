/**
 * type-usages.js — Type Usages panel component
 *
 * Text input for a type name → Find button → api.findTypeUsages()
 * Results list: FQN, kind icon, file:line
 * Click a result → fetch function detail → window.playground.showDetail()
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
  macro: '!', impl: '◇',
};

function kindIcon(kind) {
  return KIND_ICONS[String(kind).toLowerCase()] ?? '·';
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

// ── init ─────────────────────────────────────────────────────────────────────

export function init(pane) {
  pane.innerHTML = `
    <div class="component-panel">
      <h2 class="panel-title">Type Usages</h2>
      <div class="search-bar">
        <input id="type-input" class="search-input" type="text"
               placeholder="Type name — e.g. MyStruct, HashMap" autocomplete="off" />
        <input id="type-limit" class="search-limit" type="number"
               value="20" min="1" max="200" title="Result limit" />
        <button id="type-find" class="btn btn-primary">Find</button>
      </div>
      <div id="type-status" class="status-text"></div>
      <ul id="type-results" class="result-list"></ul>
    </div>
  `;

  const input     = pane.querySelector('#type-input');
  const limitEl   = pane.querySelector('#type-limit');
  const findBtn   = pane.querySelector('#type-find');
  const statusEl  = pane.querySelector('#type-status');
  const resultsEl = pane.querySelector('#type-results');

  async function find() {
    const typeName = input.value.trim();
    if (!typeName) return;

    findBtn.disabled     = true;
    statusEl.textContent = 'Searching…';
    resultsEl.innerHTML  = '';

    try {
      const data   = await apiClient.findTypeUsages(typeName, +limitEl.value || 20);
      const usages = data.usages ?? data.results ?? data.items
                     ?? (Array.isArray(data) ? data : []);

      statusEl.textContent = `${usages.length} usage(s) found`;

      if (usages.length === 0) {
        resultsEl.innerHTML = '<li class="empty-msg">No usages found.</li>';
        return;
      }

      resultsEl.innerHTML = usages.map((u, i) => {
        const fqn  = u.fqn  ?? u.name ?? '(unknown)';
        const kind = u.kind ?? '';
        const file = u.file ?? u.location?.file ?? '';
        const line = u.line ?? u.location?.line ?? '';
        const loc  = file ? `${file}${line ? ':' + line : ''}` : '';
        return `
          <li class="result-item" data-index="${i}" title="${escHtml(fqn)}">
            <span class="kind-icon" title="${escHtml(kind)}">${kindIcon(kind)}</span>
            <span class="result-fqn">${escHtml(fqn)}</span>
            ${loc ? `<span class="result-loc">${escHtml(loc)}</span>` : ''}
          </li>`;
      }).join('');

      resultsEl.querySelectorAll('.result-item').forEach((el, i) => {
        el.addEventListener('click', async () => {
          const u   = usages[i];
          const fqn = u.fqn ?? u.name;
          if (!fqn) return;
          el.classList.add('loading');
          try {
            const detail = await apiClient.getFunction(fqn);
            window.playground.showDetail(fqn, buildDetailHtml(detail));
          } catch (err) {
            window.playground.showDetail(fqn,
              `<p class="error-msg">${escHtml(err.message)}</p>`);
          } finally {
            el.classList.remove('loading');
          }
        });
      });
    } catch (err) {
      statusEl.textContent = 'Error';
      resultsEl.innerHTML  = `<li class="error-msg">${escHtml(err.message)}</li>`;
    } finally {
      findBtn.disabled = false;
    }
  }

  findBtn.addEventListener('click', find);
  input.addEventListener('keydown', e => { if (e.key === 'Enter') find(); });
}
