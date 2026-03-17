/**
 * command-palette.js — Enhanced Command Palette component
 *
 * Replaces the basic tab-navigation palette in playground.js with a
 * richer, grouped overlay:
 *
 *   • Navigation  — Go to Search, Chat, Cypher, Types …
 *   • Recent Functions — from localStorage (populated by other components)
 *   • Actions — New Session, Clear History, Toggle Sidebar, Close Detail
 *
 * Features:
 *   • Fuzzy matching with character-level highlighting
 *   • Arrow-key / Enter / Escape keyboard navigation
 *   • Mouse hover focus sync
 *   • Replaces the existing #cmd-input listeners (cloneNode trick) so both
 *     playground.js and this module don't fight over the same element
 *
 * Usage:
 *   import { init, addRecentFunction } from './components/command-palette.js';
 *   init();                          // call once at startup
 *   addRecentFunction('crate::foo'); // called by other components on detail open
 */

// ── Constants ────────────────────────────────────────────────────────────────

const RECENT_KEY = 'cmd_recent_funcs';
const MAX_RECENT = 10;

const NAVIGATION = [
  { label: 'Go to Dashboard',  icon: '⊞', tab: 'dashboard' },
  { label: 'Go to Search',     icon: '⌕', tab: 'search'    },
  { label: 'Go to Call Graph', icon: '⬡', tab: 'callgraph' },
  { label: 'Go to Chat',       icon: '◉', tab: 'chat'      },
  { label: 'Go to Cypher',     icon: '⌥', tab: 'cypher'    },
  { label: 'Go to Types',      icon: 'T', tab: 'types'     },
  { label: 'Go to Traits',     icon: '◈', tab: 'traits'    },
  { label: 'Go to Modules',    icon: '⊟', tab: 'modules'   },
  { label: 'Go to Audit',      icon: '≡', tab: 'audit'     },
  { label: 'Go to Gaps',       icon: '△', tab: 'gaps'      },
];

// ── Recent functions helpers ─────────────────────────────────────────────────

function loadRecents() {
  try { return JSON.parse(localStorage.getItem(RECENT_KEY) || '[]'); }
  catch { return []; }
}

/** Call this from other components whenever a function detail is shown. */
export function addRecentFunction(fqn) {
  if (!fqn) return;
  const recents = loadRecents().filter(r => r !== fqn);
  localStorage.setItem(RECENT_KEY, JSON.stringify([fqn, ...recents].slice(0, MAX_RECENT)));
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function escHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

/** Returns true if every char in query appears in text (in order). */
function fuzzyMatch(text, query) {
  if (!query) return true;
  const t = text.toLowerCase();
  const q = query.toLowerCase();
  let pos = 0;
  for (let i = 0; i < q.length; i++) {
    pos = t.indexOf(q[i], pos);
    if (pos === -1) return false;
    pos++;
  }
  return true;
}

/** Wraps matched characters in <mark> for visual highlighting. */
function highlightMatch(text, query) {
  if (!query) return escHtml(text);
  const lower = text.toLowerCase();
  const q     = query.toLowerCase();
  const out   = [];
  let pos = 0, prev = 0;
  for (let i = 0; i < q.length; i++) {
    const idx = lower.indexOf(q[i], pos);
    if (idx === -1) break;
    out.push(escHtml(text.slice(prev, idx)));
    out.push(`<mark>${escHtml(text[idx])}</mark>`);
    prev = idx + 1;
    pos  = prev;
  }
  out.push(escHtml(text.slice(prev)));
  return out.join('');
}

function switchTab(tabId) {
  document.querySelector(`.tab-bar__tab[data-tab="${tabId}"]`)?.click();
}

// ── init ─────────────────────────────────────────────────────────────────────

export function init() {
  const overlay = document.getElementById('cmd-palette');
  const listEl  = document.getElementById('cmd-list');
  const origInput = document.getElementById('cmd-input');
  if (!overlay || !listEl || !origInput) return;

  // Clone to clear event listeners registered by playground.js
  const input = origInput.cloneNode(true);
  origInput.parentNode.replaceChild(input, origInput);

  const ACTIONS = [
    { label: 'New Session',    icon: '＋',
      action: () => document.dispatchEvent(new CustomEvent('playground:new-session')) },
    { label: 'Clear History',  icon: '⌫',
      action: () => {
        localStorage.removeItem('cypher_history');
        localStorage.removeItem(RECENT_KEY);
      }
    },
    { label: 'Close Detail',   icon: '✕',
      action: () => document.getElementById('app-shell')?.classList.add('detail-hidden') },
    { label: 'Toggle Sidebar', icon: '‹',
      action: () => document.getElementById('sidebar-toggle')?.click() },
  ];

  // Flat list of all rendered items (rebuilt on each render)
  let items    = [];
  let focusIdx = -1;

  // ── Build grouped items ───────────────────────────────────────────────────

  function buildSections(query) {
    const sections = [];

    const navMatches = NAVIGATION.filter(n => fuzzyMatch(n.label, query));
    if (navMatches.length) {
      sections.push({
        header: 'Navigation',
        entries: navMatches.map(n => ({
          label:  n.label,
          icon:   n.icon,
          action: () => switchTab(n.tab),
        })),
      });
    }

    const recents = loadRecents()
      .filter(r => fuzzyMatch(r, query))
      .map(fqn => ({
        label:  fqn,
        icon:   'ƒ',
        action: () => window.playground?.showDetail?.(fqn, '<p>Loading…</p>'),
      }));
    if (recents.length) {
      sections.push({ header: 'Recent Functions', entries: recents });
    }

    const actionMatches = ACTIONS.filter(a => fuzzyMatch(a.label, query));
    if (actionMatches.length) {
      sections.push({ header: 'Actions', entries: actionMatches });
    }

    return sections;
  }

  // ── Render list ───────────────────────────────────────────────────────────

  function render(query) {
    const sections = buildSections(query);
    items    = sections.flatMap(s => s.entries);
    focusIdx = -1;

    if (items.length === 0) {
      listEl.innerHTML = '<li class="cmd-palette__empty">No results.</li>';
      return;
    }

    listEl.innerHTML = sections.map(sec => {
      const rows = sec.entries.map(entry => {
        const gi = items.indexOf(entry);
        return `
          <li class="cmd-palette__item" data-index="${gi}" role="option">
            <span class="cmd-palette__item-icon">${escHtml(entry.icon)}</span>
            <span class="cmd-palette__item-label">${highlightMatch(entry.label, query)}</span>
          </li>`;
      }).join('');
      return `
        <li class="cmd-palette__section-header" aria-hidden="true">${escHtml(sec.header)}</li>
        ${rows}`;
    }).join('');

    listEl.querySelectorAll('.cmd-palette__item').forEach(el => {
      el.addEventListener('click', () => {
        const i = +el.dataset.index;
        if (items[i]) { items[i].action(); close(); }
      });
      el.addEventListener('mouseenter', () => {
        listEl.querySelectorAll('.cmd-palette__item')
          .forEach(x => x.classList.remove('focused'));
        el.classList.add('focused');
        focusIdx = +el.dataset.index;
      });
    });
  }

  // ── Open / Close ──────────────────────────────────────────────────────────

  function open() {
    overlay.removeAttribute('hidden');
    input.value = '';
    focusIdx    = -1;
    render('');
    input.focus();
  }

  function close() {
    overlay.setAttribute('hidden', '');
    focusIdx = -1;
  }

  // ── Input events ──────────────────────────────────────────────────────────

  input.addEventListener('input', e => render(e.target.value));

  input.addEventListener('keydown', e => {
    const els   = listEl.querySelectorAll('.cmd-palette__item');
    const count = items.length;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusIdx = count ? (focusIdx + 1) % count : -1;
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusIdx = count ? (focusIdx - 1 + count) % count : -1;
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (focusIdx >= 0 && items[focusIdx]) { items[focusIdx].action(); close(); }
      return;
    } else if (e.key === 'Escape') {
      e.stopPropagation();
      close();
      return;
    } else {
      return;
    }

    els.forEach(el => el.classList.toggle('focused', +el.dataset.index === focusIdx));
    listEl.querySelector('.cmd-palette__item.focused')
      ?.scrollIntoView({ block: 'nearest' });
  });

  // Backdrop click
  overlay.querySelector('.cmd-palette__backdrop')?.addEventListener('click', close);

  // ── Keyboard shortcut (overrides playground.js listener) ─────────────────

  document.addEventListener('keydown', e => {
    const meta = e.metaKey || e.ctrlKey;
    if (meta && e.key === 'k') {
      e.preventDefault();
      overlay.hasAttribute('hidden') ? open() : close();
    }
  });

  // ── Expose on window.playground ──────────────────────────────────────────

  window.playground = window.playground || {};
  window.playground.openPalette  = open;
  window.playground.closePalette = close;
}
