/**
 * Sidebar — session list with collapse toggle, context menu, and new session button.
 *
 * Depends on:
 *   - apiClient  (window.apiClient or imported)
 *   - state      (AppState singleton)
 *   - bus        (EventBus singleton)
 */

import { apiClient } from '../lib/api-client.js';
import { state } from '../lib/state.js';
import { bus } from '../lib/event-bus.js';

// ── Helpers ────────────────────────────────────────────────────────────────

function relativeTime(isoString) {
    if (!isoString) return '';
    const diff = Date.now() - new Date(isoString).getTime();
    const s = Math.floor(diff / 1000);
    if (s < 60) return 'just now';
    const m = Math.floor(s / 60);
    if (m < 60) return `${m}m ago`;
    const h = Math.floor(m / 60);
    if (h < 24) return `${h}h ago`;
    const d = Math.floor(h / 24);
    return `${d}d ago`;
}

function truncate(str, max = 32) {
    if (!str) return 'Untitled';
    return str.length > max ? str.slice(0, max - 1) + '…' : str;
}

function sortByRecent(sessions) {
    return [...sessions].sort((a, b) => {
        const ta = new Date(a.updated_at || a.created_at || 0).getTime();
        const tb = new Date(b.updated_at || b.created_at || 0).getTime();
        return tb - ta;
    });
}

// ── Sidebar ────────────────────────────────────────────────────────────────

class Sidebar {
    /**
     * @param {HTMLElement} container  — element that will host the sidebar
     */
    constructor(container) {
        this._container = container;
        this._sessions = [];
        this._collapsed = false;
        this._contextMenu = null;
        this._unsubs = [];

        this._render();
        this._bindGlobalEvents();
    }

    // ── Public API ──────────────────────────────────────────────────────────

    async mount() {
        await this._loadSessions();
    }

    destroy() {
        for (const unsub of this._unsubs) unsub();
        this._unsubs = [];
        this._destroyContextMenu();
        this._container.innerHTML = '';
    }

    // ── Rendering ───────────────────────────────────────────────────────────

    _render() {
        this._container.innerHTML = '';
        this._container.className = `sidebar${this._collapsed ? ' sidebar--collapsed' : ''}`;

        // Header
        const header = document.createElement('div');
        header.className = 'sidebar__header';
        header.innerHTML = `
            <span class="sidebar__title">Sessions</span>
            <button class="sidebar__new-btn" title="New session" aria-label="New session">+</button>
            <button class="sidebar__collapse-btn" title="${this._collapsed ? 'Expand' : 'Collapse'}" aria-label="Toggle sidebar">
                ${this._collapsed ? '›' : '‹'}
            </button>
        `;
        this._container.appendChild(header);

        header.querySelector('.sidebar__new-btn').addEventListener('click', () => this._onNew());
        header.querySelector('.sidebar__collapse-btn').addEventListener('click', () => this._onToggleCollapse());

        // Session list
        const list = document.createElement('ul');
        list.className = 'sidebar__list';
        list.setAttribute('role', 'listbox');
        list.setAttribute('aria-label', 'Sessions');
        this._listEl = list;
        this._container.appendChild(list);

        this._renderSessions();
    }

    _renderSessions() {
        const list = this._listEl;
        list.innerHTML = '';

        const currentId = state.getKey('currentSession')?.id;
        const sorted = sortByRecent(this._sessions);

        if (sorted.length === 0 && !this._collapsed) {
            const empty = document.createElement('li');
            empty.className = 'sidebar__empty';
            empty.textContent = 'No sessions yet';
            list.appendChild(empty);
            return;
        }

        for (const session of sorted) {
            const item = this._buildSessionItem(session, session.id === currentId);
            list.appendChild(item);
        }
    }

    _buildSessionItem(session, isActive) {
        const li = document.createElement('li');
        li.className = `sidebar__item${isActive ? ' sidebar__item--active' : ''}`;
        li.setAttribute('role', 'option');
        li.setAttribute('aria-selected', String(isActive));
        li.dataset.sessionId = session.id;

        const title = truncate(session.title || session.id);
        const msgCount = session.message_count ?? session.messages?.length ?? 0;
        const timestamp = relativeTime(session.updated_at || session.created_at);

        li.innerHTML = `
            <div class="sidebar__item-title">${escapeHtml(title)}</div>
            <div class="sidebar__item-meta">
                <span class="sidebar__item-count">${msgCount} msg${msgCount !== 1 ? 's' : ''}</span>
                <span class="sidebar__item-time">${timestamp}</span>
            </div>
        `;

        li.addEventListener('click', () => this._onSelectSession(session));
        li.addEventListener('contextmenu', (e) => this._onContextMenu(e, session));

        return li;
    }

    // ── Session actions ─────────────────────────────────────────────────────

    async _loadSessions() {
        try {
            const result = await apiClient.listSessions();
            this._sessions = Array.isArray(result) ? result : (result?.sessions ?? []);
            state.setSessions(this._sessions);
            this._renderSessions();
        } catch (err) {
            console.error('[Sidebar] Failed to load sessions:', err);
        }
    }

    async _onNew() {
        try {
            const session = await apiClient.createSession();
            this._sessions = [session, ...this._sessions];
            state.setSessions(this._sessions);
            this._onSelectSession(session);
        } catch (err) {
            console.error('[Sidebar] Failed to create session:', err);
        }
    }

    _onSelectSession(session) {
        state.set({ currentSession: session });
        bus.emit('session:changed', { session });
        this._renderSessions();
    }

    async _forkSession(session) {
        try {
            const forked = await apiClient.forkSession(session.id);
            this._sessions = [forked, ...this._sessions];
            state.setSessions(this._sessions);
            this._onSelectSession(forked);
        } catch (err) {
            console.error('[Sidebar] Fork failed:', err);
        }
    }

    async _deleteSession(session) {
        try {
            await apiClient.deleteSession(session.id);
            this._sessions = this._sessions.filter(s => s.id !== session.id);
            state.setSessions(this._sessions);

            const current = state.getKey('currentSession');
            if (current?.id === session.id) {
                const next = sortByRecent(this._sessions)[0] ?? null;
                state.set({ currentSession: next });
                bus.emit('session:changed', { session: next });
            }

            this._renderSessions();
        } catch (err) {
            console.error('[Sidebar] Delete failed:', err);
        }
    }

    // ── Collapse ─────────────────────────────────────────────────────────────

    _onToggleCollapse() {
        this._collapsed = !this._collapsed;
        this._render();
        bus.emit('sidebar:collapsed', { collapsed: this._collapsed });
    }

    // ── Context menu ─────────────────────────────────────────────────────────

    _onContextMenu(e, session) {
        e.preventDefault();
        this._destroyContextMenu();

        const menu = document.createElement('ul');
        menu.className = 'sidebar__context-menu';
        menu.style.cssText = `position:fixed;top:${e.clientY}px;left:${e.clientX}px;z-index:9999`;
        menu.setAttribute('role', 'menu');

        const fork = document.createElement('li');
        fork.textContent = 'Fork';
        fork.setAttribute('role', 'menuitem');
        fork.addEventListener('click', () => { this._destroyContextMenu(); this._forkSession(session); });

        const del = document.createElement('li');
        del.textContent = 'Delete';
        del.className = 'sidebar__context-menu-item--danger';
        del.setAttribute('role', 'menuitem');
        del.addEventListener('click', () => { this._destroyContextMenu(); this._deleteSession(session); });

        menu.appendChild(fork);
        menu.appendChild(del);
        document.body.appendChild(menu);
        this._contextMenu = menu;
    }

    _destroyContextMenu() {
        if (this._contextMenu) {
            this._contextMenu.remove();
            this._contextMenu = null;
        }
    }

    // ── Global event bindings ────────────────────────────────────────────────

    _bindGlobalEvents() {
        const dismissMenu = (e) => {
            if (this._contextMenu && !this._contextMenu.contains(e.target)) {
                this._destroyContextMenu();
            }
        };
        document.addEventListener('click', dismissMenu);
        document.addEventListener('keydown', (e) => { if (e.key === 'Escape') this._destroyContextMenu(); });

        // Refresh list when sessions change externally
        const unsub = bus.on('session:refresh', () => this._loadSessions());
        this._unsubs.push(unsub, () => document.removeEventListener('click', dismissMenu));
    }
}

// ── XSS guard ──────────────────────────────────────────────────────────────

function escapeHtml(str) {
    return str.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

export { Sidebar };
export default Sidebar;
