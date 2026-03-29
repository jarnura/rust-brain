/**
 * call-sites.js — Call Sites panel component (turbofish analysis)
 *
 * Text input for a type name → Find button → api.findCallsWithType()
 * Results list: caller FQN, callee FQN, type args, file:line
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
      <h2 class="panel-title">Call Sites (Turbofish)</h2>
      <p class="panel-hint">Find monomorphized calls with specific type arguments (e.g., <code>parse::&lt;String&gt;()</code>)</p>
      <div class="search-bar">
        <input id="type-input" class="search-input" type="text"
               placeholder="Type name — e.g. String, PaymentRequest" autocomplete="off" />
        <input id="callee-input" class="search-input" type="text"
               placeholder="Callee (optional) — e.g. parse, collect" autocomplete="off" />
        <input id="limit-input" class="search-limit" type="number"
               value="20" min="1" max="100" title="Result limit" />
        <button id="find-btn" class="btn btn-primary">Find</button>
      </div>
      <div id="status" class="status-text"></div>
      <ul id="results" class="result-list"></ul>
    </div>
  `;

  const typeInput   = pane.querySelector('#type-input');
  const calleeInput = pane.querySelector('#callee-input');
  const limitInput  = pane.querySelector('#limit-input');
  const findBtn     = pane.querySelector('#find-btn');
  const statusEl    = pane.querySelector('#status');
  const resultsEl   = pane.querySelector('#results');

  async function find() {
    const typeName = typeInput.value.trim();
    if (!typeName) return;

    const calleeName = calleeInput.value.trim() || undefined;
    const limit = +limitInput.value || 20;

    findBtn.disabled     = true;
    statusEl.textContent = 'Searching…';
    resultsEl.innerHTML  = '';

    try {
      const data = await apiClient.findCallsWithType(typeName, { calleeName, limit });
      const calls = data.calls ?? data.results ?? data.items
                    ?? (Array.isArray(data) ? data : []);

      statusEl.textContent = `${calls.length} call site(s) found for "${typeName}"`;

      if (calls.length === 0) {
        resultsEl.innerHTML = '<li class="empty-msg">No call sites found. Try a different type or remove the callee filter.</li>';
        return;
      }

      resultsEl.innerHTML = calls.map((call, i) => {
        const callerFqn = call.caller_fqn ?? call.caller ?? '(unknown)';
        const calleeFqn = call.callee_fqn ?? call.callee ?? '(unknown)';
        const typeArgs  = call.concrete_type_args ?? call.type_args ?? [];
        const file      = call.file_path ?? call.file ?? '';
        const line      = call.line_number ?? call.line ?? '';
        const loc       = file ? `${file}:${line}` : '';
        const mono      = call.is_monomorphized ? '✓' : '✗';
        const quality   = call.quality ?? 'unknown';
        
        return `
          <li class="result-item call-site-item" data-index="${i}">
            <div class="call-site-header">
              <span class="caller-fqn" title="Caller">${escHtml(callerFqn)}</span>
              <span class="arrow">→</span>
              <span class="callee-fqn" title="Callee">${escHtml(calleeFqn)}</span>
            </div>
            <div class="call-site-meta">
              <span class="type-args" title="Type Arguments">&lt;${typeArgs.map(escHtml).join(', ')}&gt;</span>
              <span class="mono-badge ${call.is_monomorphized ? 'mono-yes' : 'mono-no'}" title="Monomorphized">mono: ${mono}</span>
              <span class="quality-badge">${escHtml(quality)}</span>
            </div>
            ${loc ? `<div class="call-site-loc">${escHtml(loc)}</div>` : ''}
          </li>`;
      }).join('');

      resultsEl.querySelectorAll('.result-item').forEach((el, i) => {
        el.addEventListener('click', async () => {
          const call = calls[i];
          const fqn = call.caller_fqn ?? call.caller;
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
  typeInput.addEventListener('keydown', e => { if (e.key === 'Enter') find(); });
  calleeInput.addEventListener('keydown', e => { if (e.key === 'Enter') find(); });
}
