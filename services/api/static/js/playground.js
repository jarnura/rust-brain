/**
 * playground.js — rust-brain Playground entry point (ES module)
 *
 * Responsibilities:
 *  - Register highlight.js languages
 *  - Initialize tab switching (tab bar + sidebar nav)
 *  - Initialize collapsible sidebar
 *  - Initialize split-pane resizers
 *  - Initialize command palette (Cmd+K)
 *  - Keyboard shortcuts: Cmd+1-9 → tabs, Cmd+/ → sidebar, Escape
 *  - Window resize handler
 */

// ── highlight.js registration ──────────────────────────────────────────────
if (window.hljs) {
  hljs.configure({ ignoreUnescapedHTML: true });
}

// ── Constants ──────────────────────────────────────────────────────────────
const TABS = [
  { id: 'dashboard', label: 'Dashboard', icon: '⊞', key: '1' },
  { id: 'search',    label: 'Search',    icon: '⌕', key: '2' },
  { id: 'callgraph', label: 'Call Graph',icon: '⬡', key: '3' },
  { id: 'chat',      label: 'Chat',      icon: '◉', key: '4' },
  { id: 'cypher',    label: 'Cypher',    icon: '⌥', key: '5' },
  { id: 'types',     label: 'Types',     icon: 'T', key: '6' },
  { id: 'traits',    label: 'Traits',    icon: '◈', key: '7' },
  { id: 'modules',   label: 'Modules',   icon: '⊟', key: '8' },
  { id: 'audit',     label: 'Audit',     icon: '≡', key: '9' },
  { id: 'gaps',      label: 'Gaps',      icon: '△', key: '0' },
];

// ── DOM refs ───────────────────────────────────────────────────────────────
const appShell      = document.getElementById('app-shell');
const sidebarToggle = document.getElementById('sidebar-toggle');
const sidebarNav    = document.getElementById('sidebar-nav');
const tabBar        = document.getElementById('tab-bar');
const tabContent    = document.getElementById('tab-content');
const cmdPalette    = document.getElementById('cmd-palette');
const cmdInput      = document.getElementById('cmd-input');
const cmdList       = document.getElementById('cmd-list');
const detailClose   = document.getElementById('detail-close');
const resizerLeft   = document.getElementById('resizer-left');
const resizerRight  = document.getElementById('resizer-right');

let activeTab = 'dashboard';

// ── Tab switching ──────────────────────────────────────────────────────────
function switchTab(tabId) {
  if (activeTab === tabId) return;
  activeTab = tabId;

  // Tab bar buttons
  tabBar.querySelectorAll('.tab-bar__tab').forEach(btn => {
    const active = btn.dataset.tab === tabId;
    btn.classList.toggle('active', active);
    btn.setAttribute('aria-selected', String(active));
  });

  // Sidebar nav items
  sidebarNav.querySelectorAll('.sidebar__item').forEach(btn => {
    btn.classList.toggle('active', btn.dataset.tab === tabId);
  });

  // Panes
  tabContent.querySelectorAll('.tab-pane').forEach(pane => {
    pane.classList.toggle('active', pane.id === `pane-${tabId}`);
  });

  // Dispatch so components can lazy-init
  document.dispatchEvent(new CustomEvent('playground:tab-change', { detail: { tab: tabId } }));
}

// Attach tab-bar click
tabBar.addEventListener('click', e => {
  const btn = e.target.closest('.tab-bar__tab[data-tab]');
  if (btn) switchTab(btn.dataset.tab);
});

// Attach sidebar nav click
sidebarNav.addEventListener('click', e => {
  const btn = e.target.closest('.sidebar__item[data-tab]');
  if (btn) switchTab(btn.dataset.tab);
});

// ── Sidebar collapse ───────────────────────────────────────────────────────
function toggleSidebar() {
  appShell.classList.toggle('sidebar-collapsed');
  const collapsed = appShell.classList.contains('sidebar-collapsed');
  sidebarToggle.title = collapsed ? 'Expand sidebar (Cmd+/)' : 'Collapse sidebar (Cmd+/)';
  sidebarToggle.setAttribute('aria-expanded', String(!collapsed));
}

sidebarToggle.addEventListener('click', toggleSidebar);

// ── Detail panel close ─────────────────────────────────────────────────────
detailClose.addEventListener('click', () => {
  appShell.classList.add('detail-hidden');
});

// Expose so components can show the detail panel
window.playground = window.playground || {};
window.playground.showDetail = function showDetail(title, html) {
  document.getElementById('detail-title').textContent = title;
  document.getElementById('detail-body').innerHTML = html;
  appShell.classList.remove('detail-hidden');
  if (window.hljs) {
    document.getElementById('detail-body').querySelectorAll('pre code').forEach(block => {
      hljs.highlightElement(block);
    });
  }
};

// ── Split-pane resizers ────────────────────────────────────────────────────
function initResizer(resizerEl, columnIndex) {
  if (!resizerEl) return;
  let startX, startCols;

  resizerEl.addEventListener('mousedown', e => {
    e.preventDefault();
    startX    = e.clientX;
    startCols = window.getComputedStyle(appShell)
      .gridTemplateColumns.split(' ').map(parseFloat);
    resizerEl.classList.add('dragging');
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';

    function onMove(ev) {
      const delta = ev.clientX - startX;
      const cols  = [...startCols];
      const min   = 40;
      cols[columnIndex]     = Math.max(min, cols[columnIndex] + delta);
      cols[columnIndex + 2] = Math.max(min, cols[columnIndex + 2] - delta);
      appShell.style.gridTemplateColumns = cols.map((v, i) =>
        i === 1 || i === 3 ? '4px' : `${v}px`
      ).join(' ');
    }

    function onUp() {
      resizerEl.classList.remove('dragging');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup',   onUp);
    }

    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup',   onUp);
  });
}

initResizer(resizerLeft,  0); // moves column 0 (sidebar) ↔ column 2 (main)
initResizer(resizerRight, 2); // moves column 2 (main)    ↔ column 4 (detail)

// ── Command Palette ────────────────────────────────────────────────────────
function buildCmdItems() {
  return TABS.map(t => ({
    label: t.label,
    icon:  t.icon,
    kbd:   `⌘${t.key}`,
    action: () => switchTab(t.id),
  }));
}

let cmdFocusIndex = -1;

function openPalette() {
  cmdPalette.removeAttribute('hidden');
  cmdInput.value = '';
  cmdFocusIndex  = -1;
  renderCmdList('');
  cmdInput.focus();
}

function closePalette() {
  cmdPalette.setAttribute('hidden', '');
}

function renderCmdList(query) {
  const items = buildCmdItems().filter(
    item => !query || item.label.toLowerCase().includes(query.toLowerCase())
  );
  cmdFocusIndex = -1;
  cmdList.innerHTML = items.map((item, i) => `
    <li class="cmd-palette__item" data-index="${i}" role="option">
      <span class="cmd-palette__item-icon">${item.icon}</span>
      <span class="cmd-palette__item-label">${item.label}</span>
      ${item.kbd ? `<kbd class="cmd-palette__item-kbd">${item.kbd}</kbd>` : ''}
    </li>
  `).join('');

  cmdList.querySelectorAll('.cmd-palette__item').forEach((el, i) => {
    el.addEventListener('click', () => {
      items[i].action();
      closePalette();
    });
    el.addEventListener('mouseenter', () => {
      cmdList.querySelectorAll('.cmd-palette__item').forEach(x => x.classList.remove('focused'));
      el.classList.add('focused');
      cmdFocusIndex = i;
    });
  });
}

cmdInput.addEventListener('input', e => renderCmdList(e.target.value));

cmdInput.addEventListener('keydown', e => {
  const items = cmdList.querySelectorAll('.cmd-palette__item');
  if (e.key === 'ArrowDown') {
    e.preventDefault();
    cmdFocusIndex = (cmdFocusIndex + 1) % items.length;
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    cmdFocusIndex = (cmdFocusIndex - 1 + items.length) % items.length;
  } else if (e.key === 'Enter') {
    if (cmdFocusIndex >= 0) items[cmdFocusIndex].click();
    return;
  }
  items.forEach((el, i) => el.classList.toggle('focused', i === cmdFocusIndex));
  if (items[cmdFocusIndex]) items[cmdFocusIndex].scrollIntoView({ block: 'nearest' });
});

// Close on backdrop click
cmdPalette.querySelector('.cmd-palette__backdrop').addEventListener('click', closePalette);

// ── Keyboard shortcuts ─────────────────────────────────────────────────────
document.addEventListener('keydown', e => {
  const meta = e.metaKey || e.ctrlKey;

  // Escape: close palette or detail
  if (e.key === 'Escape') {
    if (!cmdPalette.hasAttribute('hidden')) { closePalette(); return; }
    if (!appShell.classList.contains('detail-hidden')) {
      appShell.classList.add('detail-hidden'); return;
    }
    return;
  }

  // Cmd+K: command palette
  if (meta && e.key === 'k') {
    e.preventDefault();
    cmdPalette.hasAttribute('hidden') ? openPalette() : closePalette();
    return;
  }

  // Cmd+/: toggle sidebar
  if (meta && e.key === '/') {
    e.preventDefault();
    toggleSidebar();
    return;
  }

  // Cmd+1…9,0: switch tabs
  if (meta && /^[0-9]$/.test(e.key)) {
    e.preventDefault();
    const tab = TABS.find(t => t.key === e.key);
    if (tab) switchTab(tab.id);
    return;
  }
});

// ── Window resize ──────────────────────────────────────────────────────────
let resizeTimer;
window.addEventListener('resize', () => {
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    document.dispatchEvent(new CustomEvent('playground:resize'));
  }, 150);
});

// ── Initialise ─────────────────────────────────────────────────────────────
// ES modules are deferred, so DOMContentLoaded may have already fired
function initPlayground() {
  switchTab('dashboard');

  // Enhanced command palette (eager — not tab-scoped)
  import('./components/command-palette.js')
    .then(m => m.init())
    .catch(() => {}); // falls back to basic palette already in place

  // Lazy-load component modules
  const lazyComponents = {
    dashboard: () => import('./components/dashboard.js'),
    search:    () => import('./components/search.js'),
    callgraph: () => import('./components/call-graph.js'),
    chat:      () => import('./components/chat.js'),
    cypher:    () => import('./components/cypher.js'),
    types:     () => import('./components/type-usages.js'),
    traits:    () => import('./components/trait-impls.js'),
    modules:   () => import('./components/module-tree.js'),
    audit:     () => import('./components/audit.js'),
    gaps:      () => import('./components/gaps.js'),
  };

  document.addEventListener('playground:tab-change', async ({ detail: { tab } }) => {
    if (lazyComponents[tab]) {
      const loader = lazyComponents[tab];
      delete lazyComponents[tab]; // load once
      try {
        const mod = await loader();
        if (mod.init) mod.init(document.getElementById(`pane-${tab}`));
      } catch {
        // component not yet implemented — silent
      }
    }
  });

  // Trigger dashboard init on first load
  document.dispatchEvent(new CustomEvent('playground:tab-change', { detail: { tab: 'dashboard' } }));

  updateConnectionStatus();
  setInterval(updateConnectionStatus, 30000);
}

async function updateConnectionStatus() {
  const statusDot = document.getElementById('sidebar-status');
  const statusLabel = document.getElementById('sidebar-status-label');
  
  try {
    const resp = await fetch('/health');
    const data = await resp.json();
    
    if (data.status === 'healthy') {
      statusDot.className = 'status-dot status-healthy';
      statusLabel.textContent = 'Connected';
    } else {
      statusDot.className = 'status-dot status-degraded';
      statusLabel.textContent = 'Degraded';
    }
  } catch {
    statusDot.className = 'status-dot status-unhealthy';
    statusLabel.textContent = 'Disconnected';
  }
}

// Run immediately if DOM ready, otherwise wait
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', initPlayground);
} else {
  initPlayground();
}
