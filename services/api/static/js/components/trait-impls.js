/**
 * trait-impls.js — Trait Implementations panel component
 *
 * Text input for a trait name → Find button → api.getTraitImpls()
 * Results list: impl FQN, implementing type, file:line
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
      <h2 class="panel-title">Trait Implementations</h2>
      <div class="search-bar">
        <input id="trait-input" class="search-input" type="text"
               placeholder="Trait name — e.g. Display, Iterator, Serialize" autocomplete="off" />
        <input id="trait-limit" class="search-limit" type="number"
               value="20" min="1" max="200" title="Result limit" />
        <button id="trait-find" class="btn btn-primary">Find</button>
      </div>
      <div id="trait-status" class="status-text"></div>
      <ul id="trait-results" class="result-list"></ul>
    </div>
  `;

  const input     = pane.querySelector('#trait-input');
  const limitEl   = pane.querySelector('#trait-limit');
  const findBtn   = pane.querySelector('#trait-find');
  const statusEl  = pane.querySelector('#trait-status');
  const resultsEl = pane.querySelector('#trait-results');

  async function find() {
    const traitName = input.value.trim();
    if (!traitName) return;

    findBtn.disabled     = true;
    statusEl.textContent = 'Searching…';
    resultsEl.innerHTML  = '';

    try {
      const data  = await apiClient.getTraitImpls(traitName, +limitEl.value || 20);
      const impls = data.impls ?? data.results ?? data.items
                    ?? (Array.isArray(data) ? data : []);

      statusEl.textContent = `${impls.length} impl(s) found`;

      if (impls.length === 0) {
        resultsEl.innerHTML = '<li class="empty-msg">No implementations found.</li>';
        return;
      }

      resultsEl.innerHTML = impls.map((impl, i) => {
        const implFqn  = impl.impl_fqn  ?? impl.fqn  ?? impl.name ?? '(unknown)';
        const typeName = impl.type_name ?? impl.type ?? impl.struct_name ?? '';
        const file     = impl.file ?? impl.location?.file ?? '';
        const line     = impl.line ?? impl.location?.line ?? '';
        const loc      = file ? `${file}${line ? ':' + line : ''}` : '';
        const display  = typeName || implFqn;
        return `
          <li class="result-item" data-index="${i}" title="${escHtml(implFqn)}">
            <span class="impl-type">${escHtml(display)}</span>
            ${typeName && typeName !== implFqn
              ? `<span class="impl-fqn dimmed">${escHtml(implFqn)}</span>`
              : ''}
            ${loc ? `<span class="result-loc">${escHtml(loc)}</span>` : ''}
          </li>`;
      }).join('');

      resultsEl.querySelectorAll('.result-item').forEach((el, i) => {
        el.addEventListener('click', async () => {
          const impl = impls[i];
          const fqn  = impl.impl_fqn ?? impl.fqn ?? impl.name;
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
