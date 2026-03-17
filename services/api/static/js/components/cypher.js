/**
 * cypher.js — Cypher query panel component
 *
 * Textarea + Execute button → api.queryGraph()
 * Results rendered as a JSON table.
 * Query history persisted to localStorage (last 20).
 * Cmd/Ctrl+Enter runs the query.
 */

import { apiClient } from '../lib/api-client.js';

const HISTORY_KEY = 'cypher_history';
const MAX_HISTORY = 20;

// ── localStorage helpers ────────────────────────────────────────────────────

function loadHistory() {
  try { return JSON.parse(localStorage.getItem(HISTORY_KEY) || '[]'); }
  catch { return []; }
}

function saveHistory(queries) {
  localStorage.setItem(HISTORY_KEY, JSON.stringify(queries.slice(0, MAX_HISTORY)));
}

function addToHistory(query) {
  const hist = loadHistory().filter(q => q !== query);
  saveHistory([query, ...hist]);
}

// ── Rendering helpers ───────────────────────────────────────────────────────

function escHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

function renderTable(rows) {
  if (!rows || rows.length === 0) {
    return '<p class="empty-msg">No results.</p>';
  }
  const cols  = [...new Set(rows.flatMap(r => Object.keys(r)))];
  const thead = `<tr>${cols.map(c => `<th>${escHtml(c)}</th>`).join('')}</tr>`;
  const tbody = rows.map(r =>
    `<tr>${cols.map(c => {
      const v = r[c] ?? '';
      return `<td>${escHtml(typeof v === 'object' ? JSON.stringify(v) : String(v))}</td>`;
    }).join('')}</tr>`
  ).join('');
  return `<table class="result-table"><thead>${thead}</thead><tbody>${tbody}</tbody></table>`;
}

// ── init ────────────────────────────────────────────────────────────────────

export function init(pane) {
  pane.innerHTML = `
    <div class="component-panel">
      <h2 class="panel-title">Cypher Query</h2>
      <div class="cypher-editor">
        <textarea id="cypher-input" class="cypher-textarea" rows="6"
          placeholder="MATCH (f:Function)-[:CALLS]->(g:Function) RETURN f.fqn, g.fqn LIMIT 10"
          spellcheck="false"></textarea>
        <div class="cypher-toolbar">
          <button id="cypher-run" class="btn btn-primary">▶ Execute</button>
          <button id="cypher-history-toggle" class="btn btn-ghost">History ▾</button>
          <span id="cypher-limit-label">Limit</span>
          <input id="cypher-limit" class="search-limit" type="number" value="10" min="1" max="1000" title="Row limit">
          <span id="cypher-status" class="status-text"></span>
        </div>
      </div>
      <div id="cypher-history-panel" class="history-panel" hidden></div>
      <div id="cypher-results" class="result-area"></div>
    </div>
  `;

  const textarea    = pane.querySelector('#cypher-input');
  const runBtn      = pane.querySelector('#cypher-run');
  const limitInput  = pane.querySelector('#cypher-limit');
  const statusEl    = pane.querySelector('#cypher-status');
  const historyBtn  = pane.querySelector('#cypher-history-toggle');
  const historyPane = pane.querySelector('#cypher-history-panel');
  const resultsEl   = pane.querySelector('#cypher-results');

  // ── History panel ──────────────────────────────────────────────────────────

  function renderHistoryPanel() {
    const hist = loadHistory();
    if (hist.length === 0) {
      historyPane.innerHTML = '<p class="empty-msg">No history yet.</p>';
      return;
    }
    historyPane.innerHTML = hist.map((q, i) =>
      `<div class="history-item" data-index="${i}">
         <code class="history-query">${escHtml(q)}</code>
       </div>`
    ).join('');
    historyPane.querySelectorAll('.history-item').forEach(el => {
      el.addEventListener('click', () => {
        textarea.value = loadHistory()[+el.dataset.index];
        historyPane.setAttribute('hidden', '');
        historyBtn.textContent = 'History ▾';
        textarea.focus();
      });
    });
  }

  historyBtn.addEventListener('click', () => {
    const hidden = historyPane.hasAttribute('hidden');
    if (hidden) {
      renderHistoryPanel();
      historyPane.removeAttribute('hidden');
      historyBtn.textContent = 'History ▴';
    } else {
      historyPane.setAttribute('hidden', '');
      historyBtn.textContent = 'History ▾';
    }
  });

  // ── Execute ────────────────────────────────────────────────────────────────

  async function runQuery() {
    const query = textarea.value.trim();
    if (!query) return;

    runBtn.disabled  = true;
    statusEl.textContent = 'Running…';
    resultsEl.innerHTML  = '';

    try {
      addToHistory(query);
      const limit = +limitInput.value || 10;
      const data  = await apiClient.queryGraph(query, {}, limit);

      // Normalise response shape
      const rows = data.rows ?? data.results ?? data.data ?? (Array.isArray(data) ? data : [data]);
      resultsEl.innerHTML  = renderTable(rows);
      statusEl.textContent = `${rows.length} row(s)`;
    } catch (err) {
      resultsEl.innerHTML  = `<p class="error-msg">${escHtml(err.message)}</p>`;
      statusEl.textContent = 'Error';
    } finally {
      runBtn.disabled = false;
    }
  }

  runBtn.addEventListener('click', runQuery);

  textarea.addEventListener('keydown', e => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      runQuery();
    }
  });
}
