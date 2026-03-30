/**
 * chat.js — Streaming chat component for rust-brain Playground
 *
 * Renders a chat panel with:
 *  - Header: session title + connection status indicator
 *  - Messages area: user (right, blue) / assistant (left, surface)
 *  - Tool call indicators inline (icon, name, status, collapsible result)
 *  - Textarea input (Shift+Enter newline, Enter send)
 *  - Send / Abort / Clear buttons
 *  - SSE streaming: chat:token, chat:tool_call, chat:complete, chat:error
 */

import { bus }       from '../lib/event-bus.js';
import { state }     from '../lib/state.js';
import { apiClient } from '../lib/api-client.js';
import { sseClient } from '../lib/sse-client.js';
import { renderMarkdown, escapeHtml } from '../lib/markdown.js';

// ── Helpers ────────────────────────────────────────────────────────────────

function generateId() {
    return `msg-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function formatTime(date) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

const MAX_MESSAGES = 500;

// ── ChatPanel ──────────────────────────────────────────────────────────────

class ChatPanel {
    /**
     * @param {{ container: HTMLElement, bus: EventBus, apiClient: ApiClient, sseClient: SseClient, state: AppState }} deps
     */
    constructor({ container, bus: eventBus, apiClient: api, sseClient: sse, state: appState }) {
        this._container = container;
        this._bus       = eventBus;
        this._api       = api;
        this._sse       = sse;
        this._state     = appState;

        this._messages       = [];
        this._streaming      = false;
        this._currentAssistantEl = null;
        this._currentTokens  = '';
        this._doneTimeout    = null;
        this._renderPending  = false;
        this._sessionId      = null;
        this._connected      = false;
        this._unsubs         = [];

        this._render();
        this._bindDom();
        this._bindEvents();
        this._initSession();
    }

    // ── DOM Construction ───────────────────────────────────────────────────

    _render() {
        this._container.innerHTML = `
            <div class="chat-container">
                <div class="chat-header">
                    <div class="chat-header__title">
                        <select class="chat-session-select" title="Select session">
                            <option value="">New Session</option>
                        </select>
                        <span class="chat-header__status status-dot status-unknown"></span>
                        <span class="chat-header__status-label">Disconnected</span>
                    </div>
                    <div class="chat-header__actions">
                        <button class="chat-header__btn chat-header__btn--new" title="New session">+ New</button>
                        <button class="chat-header__btn chat-header__btn--clear" title="Clear messages">Clear</button>
                    </div>
                </div>
                <div class="chat-messages" role="log" aria-live="polite"></div>
                <div class="chat-input-row">
                    <textarea class="chat-input"
                              placeholder="Ask about Rust code..."
                              rows="1"
                              aria-label="Chat message"></textarea>
                    <button class="chat-send" title="Send (Enter)">Send</button>
                    <button class="chat-abort" title="Abort generation" hidden>Abort</button>
                </div>
            </div>`;
    }

    _bindDom() {
        const root = this._container;
        this._els = {
            sessionSelect: root.querySelector('.chat-session-select'),
            statusDot:    root.querySelector('.chat-header__status'),
            statusLabel:  root.querySelector('.chat-header__status-label'),
            newBtn:       root.querySelector('.chat-header__btn--new'),
            clearBtn:     root.querySelector('.chat-header__btn--clear'),
            messagesArea: root.querySelector('.chat-messages'),
            input:        root.querySelector('.chat-input'),
            sendBtn:      root.querySelector('.chat-send'),
            abortBtn:     root.querySelector('.chat-abort'),
        };
    }

    // ── Event Binding ──────────────────────────────────────────────────────

    _bindEvents() {
        // Input: Shift+Enter = newline, Enter = send
        this._els.input.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this._sendMessage();
            }
        });

        // Auto-resize textarea
        this._els.input.addEventListener('input', () => this._autoResize());

        // Buttons
        this._els.sendBtn.addEventListener('click', () => this._sendMessage());
        this._els.abortBtn.addEventListener('click', () => this._abort());
        this._els.clearBtn.addEventListener('click', () => this._clearMessages());
        this._els.newBtn.addEventListener('click', () => this._createNewSession());

        // Session selector
        this._els.sessionSelect.addEventListener('change', (e) => {
            const sessionId = e.target.value;
            if (sessionId) {
                this._switchToSession(sessionId);
            } else {
                this._createNewSession();
            }
        });

        // SSE events
        this._unsubs.push(
            this._bus.on('chat:token',     (d) => this._onToken(d)),
            this._bus.on('chat:tool_call', (d) => this._onToolCall(d)),
            this._bus.on('chat:complete',  (d) => this._onComplete(d)),
            this._bus.on('chat:done',      ()  => this._onDone()),
            this._bus.on('chat:error',     (d) => this._onError(d)),
            this._bus.on('sse:connected',  ()  => this._setConnected(true)),
            this._bus.on('sse:reconnecting', () => this._setConnected(false)),
        );

        // State changes
        this._unsubs.push(
            this._bus.on('state:currentSession', ({ next }) => {
                if (next) this._onSessionChanged(next);
            }),
        );
    }

    // ── Session Management ─────────────────────────────────────────────────

    async _initSession() {
        // Load session list for dropdown
        await this._loadSessionList();

        const existing = this._state.getKey('currentSession');
        if (existing) {
            await this._onSessionChanged(existing);
            return;
        }

        // Try to reuse the most recent session
        const sessions = this._state.getKey('sessions');
        if (sessions && sessions.length > 0) {
            const recent = sessions[0];
            await this._switchToSession(recent.id);
            return;
        }

        // Create a new session
        await this._createNewSession();
    }

    async _loadSessionList() {
        try {
            const sessions = await this._api.listSessions();
            this._state.setSessions(sessions);
            this._updateSessionSelect();
        } catch (err) {
            console.error('Failed to load sessions:', err);
        }
    }

    _updateSessionSelect() {
        const sessions = this._state.getKey('sessions') || [];
        const currentId = this._sessionId;

        this._els.sessionSelect.innerHTML = `
            <option value="">+ New Session</option>
            ${sessions.map(s => `
                <option value="${s.id}" ${s.id === currentId ? 'selected' : ''}>
                    ${s.title || s.slug || s.id.slice(0, 8)}
                </option>
            `).join('')}
        `;
    }

    async _createNewSession() {
        // Prompt for session name
        const title = prompt('Enter session name:', '');
        if (title === null) return; // User cancelled
        
        const sessionTitle = title.trim() || `Session ${new Date().toLocaleDateString()}`;
        
        try {
            const session = await this._api.createSession({ title: sessionTitle });
            this._state.setCurrentSession(session);
            this._messages = [];
            this._els.messagesArea.innerHTML = '';
            await this._loadSessionList();
        } catch (err) {
            this._appendSystemMessage(`Failed to create session: ${err.message}`);
        }
    }

    async _switchToSession(sessionId) {
        try {
            // Save current messages before switching
            if (this._sessionId && this._messages && this._messages.length > 0) {
                this._state.setMessages(this._sessionId, this._messages);
            }

            // Load session details including messages
            const detail = await this._api.getSession(sessionId);
            this._state.setCurrentSession(detail.session);

            // Restore messages from API response
            this._messages = (detail.messages || []).map(msg => ({
                id: msg.id,
                role: msg.role,
                content: msg.parts?.filter(p => p.type === 'text').map(p => p.text).join('') || '',
                timestamp: msg.time?.created || Date.now(),
            }));

            // Also save to state for persistence
            this._state.setMessages(sessionId, this._messages);

            // Re-render messages
            this._renderAllMessages();
        } catch (err) {
            this._appendSystemMessage(`Failed to switch session: ${err.message}`);
        }
    }

    _renderAllMessages() {
        this._els.messagesArea.innerHTML = '';
        for (const msg of this._messages) {
            if (msg.role === 'user') {
                this._renderUserMessage(msg.content, msg.timestamp);
            } else if (msg.role === 'assistant') {
                this._renderAssistantMessage(msg.content, msg.timestamp);
            }
        }
        this._scrollToBottom();
    }

    _renderUserMessage(text, timestamp) {
        const el = document.createElement('div');
        el.className = 'chat-message chat-message--user';
        el.innerHTML = `
            <div class="chat-bubble">${escapeHtml(text)}</div>
            <div class="chat-meta">${formatTime(new Date(timestamp))}</div>`;
        this._els.messagesArea.appendChild(el);
    }

    _renderAssistantMessage(text, timestamp) {
        const el = document.createElement('div');
        el.className = 'chat-message chat-message--assistant';
        el.innerHTML = `
            <div class="chat-bubble">${renderMarkdown(text || '')}</div>
            <div class="chat-meta">${formatTime(new Date(timestamp))}</div>`;
        this._els.messagesArea.appendChild(el);
        this._highlightCode(el.querySelector('.chat-bubble'));
    }

    async _onSessionChanged(session) {
        const id = session.session_id || session.id;
        if (this._sessionId === id) return;

        // Save current messages before switching
        if (this._sessionId && this._messages && this._messages.length > 0) {
            this._state.setMessages(this._sessionId, this._messages);
        }

        this._sessionId = id;

        // Load messages from state or API
        let messages = this._state.getMessages(id);
        if (messages.length === 0) {
            // Try to load from API
            try {
                const detail = await this._api.getSession(id);
                messages = (detail.messages || []).map(msg => ({
                    id: msg.id,
                    role: msg.role,
                    content: msg.parts?.filter(p => p.type === 'text').map(p => p.text).join('') || '',
                    timestamp: msg.time?.created || Date.now(),
                }));
                this._state.setMessages(id, messages);
            } catch (err) {
                console.error('Failed to load messages:', err);
            }
        }

        this._messages = messages;
        this._renderAllMessages();
        this._updateSessionSelect();

        // Connect SSE stream for this session
        this._sse.connect(id);
        this._setConnected(true);
        this._bus.emit('chat:session_changed', { sessionId: id });
    }

    // ── Send Message ───────────────────────────────────────────────────────

    async _sendMessage() {
        const raw = this._els.input.value.trim();
        if (!raw || this._streaming) return;

        // Lock immediately to prevent double-submit
        this._setStreaming(true);

        this._els.input.value = '';
        this._autoResize();

        // Render user bubble (escaped)
        this._appendUserMessage(raw);

        // Prepare streaming state
        this._currentTokens = '';
        this._currentAssistantEl = this._createAssistantBubble();

        this._bus.emit('chat:message_sent', { sessionId: this._sessionId, message: raw });

        try {
            // Fire-and-forget: send message async, results arrive via SSE stream
            await this._api.sendChatAsync(this._sessionId, raw);
            // Streaming is now driven by SSE events (token → complete)
        } catch (err) {
            this._onError({ message: err.message });
        }
    }

    // ── Streaming Handlers ─────────────────────────────────────────────────

    _onToken({ token, text }) {
        this._currentTokens += (token || text || '');

        // Cancel pending finalization — more tokens are arriving
        if (this._doneTimeout) {
            clearTimeout(this._doneTimeout);
            this._doneTimeout = null;
        }

        // Auto-create assistant bubble if tokens arrive after a done event
        // (happens when agents spawn sub-agents: session goes idle → busy again)
        if (!this._currentAssistantEl) {
            this._currentAssistantEl = this._createAssistantBubble();
            this._setStreaming(true);
        }

        if (!this._renderPending) {
            this._renderPending = true;
            requestAnimationFrame(() => {
                this._renderPending = false;
                if (!this._currentAssistantEl) return;
                const bubble = this._currentAssistantEl.querySelector('.chat-bubble');
                bubble.innerHTML = renderMarkdown(this._currentTokens);
                this._highlightCode(bubble);
                this._scrollToBottom();
            });
        }
    }

    _onToolCall({ name, args, status, result }) {
        if (!this._currentAssistantEl) {
            this._currentAssistantEl = this._createAssistantBubble();
        }

        const parent = this._currentAssistantEl;
        const existingTool = parent.querySelector(`[data-tool-name="${CSS.escape(name)}"]`);

        if (existingTool) {
            this._updateToolCallEl(existingTool, { status, result });
        } else {
            const toolEl = this._createToolCallEl({ name, args, status });
            const bubble = parent.querySelector('.chat-bubble');
            bubble.insertAdjacentElement('afterend', toolEl);
        }
        this._scrollToBottom();
    }

    _onComplete({ message, response, source }) {
        // Update bubble with accumulated tokens or event content — but don't finalize yet.
        // Multi-step responses (tool call → text) emit multiple completes.
        const content = this._currentTokens || message || response || '';

        // Auto-create assistant bubble if complete arrives without one
        if (!this._currentAssistantEl && content) {
            this._currentAssistantEl = this._createAssistantBubble();
            this._setStreaming(true);
        }

        if (this._currentAssistantEl && content) {
            const bubble = this._currentAssistantEl.querySelector('.chat-bubble');
            bubble.innerHTML = renderMarkdown(content);
            this._highlightCode(bubble);
        }

        // Update source badge if provided
        if (source && this._currentAssistantEl) {
            this._setSourceBadge(this._currentAssistantEl, source);
        }

        // Finalize running tool indicators for this step
        this._currentAssistantEl?.querySelectorAll('.tool-call--running')
            .forEach(el => {
                el.classList.remove('tool-call--running');
                el.classList.add('tool-call--done');
            });

        this._scrollToBottom();
    }

    _onDone({ source } = {}) {
        // Update source badge on final done event
        if (source && this._currentAssistantEl) {
            this._setSourceBadge(this._currentAssistantEl, source);
        }

        // For multi-step agent conversations, the session may go idle briefly
        // between steps (agent → sub-agent → agent). Debounce finalization so
        // that if new tokens arrive within 1s, we continue streaming instead
        // of prematurely closing the bubble.
        if (this._doneTimeout) clearTimeout(this._doneTimeout);
        this._doneTimeout = setTimeout(() => {
            this._doneTimeout = null;
            // Only finalize if we haven't received new tokens since the done event
            if (!this._streaming) return; // already finalized
            this._finalizeStreaming();
        }, 1000);
    }

    _onError({ message, error }) {
        const text = message || error || 'Unknown error';

        if (this._currentAssistantEl) {
            const errEl = document.createElement('div');
            errEl.className = 'chat-error';
            errEl.textContent = text;
            this._currentAssistantEl.appendChild(errEl);
        } else {
            this._appendSystemMessage(text);
        }

        // Mark running tool calls as errored
        this._currentAssistantEl?.querySelectorAll('.tool-call--running')
            .forEach(el => {
                el.classList.remove('tool-call--running');
                el.classList.add('tool-call--error');
            });

        this._finalizeStreaming();
    }

    // ── Abort ──────────────────────────────────────────────────────────────

    async _abort() {
        if (!this._sessionId) return;
        try {
            await this._api.abortSession(this._sessionId);
        } catch {
            // best-effort
        }
        this._finalizeStreaming();
    }

    // ── Clear ──────────────────────────────────────────────────────────────

    _clearMessages() {
        if (this._streaming) {
            this._abort();
        }
        this._messages = [];
        this._els.messagesArea.innerHTML = '';
        this._currentAssistantEl = null;
        this._currentTokens = '';
    }

    // ── DOM Helpers ────────────────────────────────────────────────────────

    _appendUserMessage(text) {
        const id = generateId();
        const el = document.createElement('div');
        el.className = 'chat-message chat-message--user';
        el.id = id;
        el.innerHTML = `
            <div class="chat-bubble">${escapeHtml(text)}</div>
            <div class="chat-meta">${formatTime(new Date())}</div>`;

        this._els.messagesArea.appendChild(el);
        const msg = { id, role: 'user', content: text, timestamp: Date.now() };
        this._messages.push(msg);
        if (this._messages.length > MAX_MESSAGES) this._messages.shift();
        // Save to state for persistence
        if (this._sessionId) {
            this._state.addMessage(this._sessionId, msg);
        }
        this._scrollToBottom();
    }

    _createAssistantBubble(source) {
        const id = generateId();
        const el = document.createElement('div');
        el.className = 'chat-message chat-message--assistant';
        el.id = id;
        const sourceTag = source ? this._sourceTag(source) : '';
        el.innerHTML = `
            <div class="chat-bubble"><span class="spinner"></span></div>
            <div class="chat-meta">${sourceTag}${formatTime(new Date())}</div>`;

        this._els.messagesArea.appendChild(el);
        this._messages.push({ id, role: 'assistant', content: '', timestamp: Date.now() });
        if (this._messages.length > MAX_MESSAGES) this._messages.shift();
        this._scrollToBottom();
        return el;
    }

    _createToolCallEl({ name, args, status }) {
        const statusClass = status === 'error' ? 'tool-call--error'
            : status === 'done' ? 'tool-call--done'
            : 'tool-call--running';

        const el = document.createElement('div');
        el.className = `tool-call ${statusClass}`;
        el.dataset.toolName = name;

        const argsStr = args ? (typeof args === 'string' ? args : JSON.stringify(args)) : '';
        const truncatedArgs = argsStr.length > 80 ? argsStr.slice(0, 80) + '...' : argsStr;

        el.innerHTML = `
            <span class="tool-call__icon">${statusClass.includes('running') ? '<span class="spinner"></span>' : (statusClass.includes('error') ? '\u2717' : '\u2713')}</span>
            <span class="tool-call__name">${escapeHtml(name)}</span>
            <span class="tool-call__args" title="${escapeHtml(argsStr)}">${escapeHtml(truncatedArgs)}</span>
            <button class="tool-call__toggle" aria-expanded="false" hidden>details</button>
            <div class="tool-call__result" hidden></div>`;

        // Collapsible result toggle
        const toggleBtn = el.querySelector('.tool-call__toggle');
        const resultDiv = el.querySelector('.tool-call__result');
        toggleBtn.addEventListener('click', () => {
            const expanded = toggleBtn.getAttribute('aria-expanded') === 'true';
            toggleBtn.setAttribute('aria-expanded', String(!expanded));
            resultDiv.hidden = expanded;
        });

        return el;
    }

    _updateToolCallEl(el, { status, result }) {
        // Update status class
        el.classList.remove('tool-call--running', 'tool-call--done', 'tool-call--error');
        const statusClass = status === 'error' ? 'tool-call--error'
            : status === 'done' ? 'tool-call--done'
            : 'tool-call--running';
        el.classList.add(statusClass);

        // Update icon
        const icon = el.querySelector('.tool-call__icon');
        if (statusClass.includes('running')) {
            icon.innerHTML = '<span class="spinner"></span>';
        } else if (statusClass.includes('error')) {
            icon.textContent = '\u2717';
        } else {
            icon.textContent = '\u2713';
        }

        // Populate result
        if (result !== undefined && result !== null) {
            const toggleBtn = el.querySelector('.tool-call__toggle');
            const resultDiv = el.querySelector('.tool-call__result');
            toggleBtn.hidden = false;

            const resultStr = typeof result === 'string' ? result : JSON.stringify(result, null, 2);
            resultDiv.innerHTML = `<pre>${escapeHtml(resultStr)}</pre>`;
        }
    }

    _sourceTag(source) {
        const label = source === 'ollama' ? 'via Ollama' : 'via OpenCode';
        const cls = source === 'ollama' ? 'chat-source--ollama' : 'chat-source--opencode';
        return `<span class="chat-source ${cls}">${escapeHtml(label)}</span> `;
    }

    _setSourceBadge(messageEl, source) {
        const meta = messageEl.querySelector('.chat-meta');
        if (!meta) return;
        // Replace existing badge or prepend one
        const existing = meta.querySelector('.chat-source');
        if (existing) {
            existing.className = `chat-source ${source === 'ollama' ? 'chat-source--ollama' : 'chat-source--opencode'}`;
            existing.textContent = source === 'ollama' ? 'via Ollama' : 'via OpenCode';
        } else {
            meta.insertAdjacentHTML('afterbegin', this._sourceTag(source));
        }
    }

    _appendSystemMessage(text) {
        const el = document.createElement('div');
        el.className = 'chat-message chat-message--system';
        el.innerHTML = `<div class="chat-bubble chat-bubble--system">${escapeHtml(text)}</div>`;
        this._els.messagesArea.appendChild(el);
        this._scrollToBottom();
    }

    // ── State Helpers ──────────────────────────────────────────────────────

    _setStreaming(active) {
        this._streaming = active;
        this._els.sendBtn.hidden  = active;
        this._els.abortBtn.hidden = !active;
        this._els.input.disabled  = active;
        this._els.sendBtn.disabled = active;
    }

    _finalizeStreaming() {
        if (this._doneTimeout) {
            clearTimeout(this._doneTimeout);
            this._doneTimeout = null;
        }
        this._setStreaming(false);
        this._currentAssistantEl = null;
        this._currentTokens = '';
        this._els.input.focus();
    }

    _setConnected(connected) {
        this._connected = connected;
        const dot   = this._els.statusDot;
        const label = this._els.statusLabel;

        dot.classList.remove('status-healthy', 'status-unknown', 'status-degraded');

        if (connected) {
            dot.classList.add('status-healthy');
            label.textContent = 'Connected';
        } else {
            dot.classList.add('status-degraded');
            label.textContent = 'Reconnecting...';
        }
    }

    _scrollToBottom() {
        const area = this._els.messagesArea;
        requestAnimationFrame(() => {
            area.scrollTop = area.scrollHeight;
        });
    }

    _autoResize() {
        const el = this._els.input;
        el.style.height = 'auto';
        el.style.height = Math.min(el.scrollHeight, 120) + 'px';
    }

    _highlightCode(container) {
        if (typeof hljs !== 'undefined') {
            container.querySelectorAll('pre code').forEach(block => {
                hljs.highlightElement(block);
            });
        }
    }

    // ── Cleanup ────────────────────────────────────────────────────────────

    destroy() {
        // Save messages before destroying
        if (this._sessionId && this._messages && this._messages.length > 0) {
            this._state.setMessages(this._sessionId, this._messages);
        }
        for (const unsub of this._unsubs) unsub();
        this._unsubs = [];
        this._sse.disconnect();
        // Don't clear container - just hide it
        // this._container.innerHTML = '';
    }
}

// ── Module init (called by playground.js lazy-loader) ──────────────────────

let panel = null;

export function init(container) {
    if (panel) {
        // Panel exists - restore it to the new container
        // Re-render and re-bind to ensure UI is fresh
        panel.destroy();
        panel._container = container;
        panel._unsubs = [];
        panel._render();
        panel._bindDom();
        panel._bindEvents();
        panel._renderAllMessages();
        return;
    }
    panel = new ChatPanel({
        container,
        bus,
        apiClient,
        sseClient,
        state,
    });
}

export { ChatPanel };
export default ChatPanel;
