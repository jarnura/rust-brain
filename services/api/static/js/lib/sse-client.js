/**
 * SseClient - Managed EventSource connection with auto-reconnect
 * Emits: chat:token, chat:complete, chat:tool_call, chat:error,
 *        sse:connected, sse:reconnecting
 */
import { bus } from './event-bus.js';

const MIN_BACKOFF = 1_000;
const MAX_BACKOFF = 30_000;

class SseClient {
    constructor() {
        this._source = null;
        this._url = null;
        this._backoff = MIN_BACKOFF;
        this._reconnectTimer = null;
        this._stopped = true;
    }

    /**
     * Connect to the SSE stream for a given session
     * @param {string} sessionId
     * @param {string} baseUrl  optional override (defaults to current origin)
     */
    connect(sessionId, baseUrl) {
        this._stopped = false;
        const host = baseUrl || `http://${window.location.hostname}:8088`;
        this._url = `${host}/tools/chat/stream?session_id=${encodeURIComponent(sessionId)}`;
        this._open();
    }

    /** Permanently disconnect and clear any pending reconnects */
    disconnect() {
        this._stopped = true;
        clearTimeout(this._reconnectTimer);
        this._close();
    }

    // ----------------------------------------------------------------- private

    _open() {
        this._close();

        const source = new EventSource(this._url);
        this._source = source;

        source.addEventListener('open', () => {
            this._backoff = MIN_BACKOFF;
            bus.emit('sse:connected', { url: this._url });
        });

        source.addEventListener('token', (e) => {
            bus.emit('chat:token', this._parse(e.data));
        });

        source.addEventListener('complete', (e) => {
            bus.emit('chat:complete', this._parse(e.data));
        });

        source.addEventListener('tool_call', (e) => {
            bus.emit('chat:tool_call', this._parse(e.data));
        });

        source.addEventListener('error', (e) => {
            const payload = e.data ? this._parse(e.data) : { message: 'Stream error' };
            bus.emit('chat:error', payload);
        });

        source.onerror = () => {
            this._close();
            if (!this._stopped) {
                this._scheduleReconnect();
            }
        };
    }

    _close() {
        if (this._source) {
            this._source.onopen = null;
            this._source.onerror = null;
            this._source.close();
            this._source = null;
        }
    }

    _scheduleReconnect() {
        const delay = this._backoff;
        this._backoff = Math.min(this._backoff * 2, MAX_BACKOFF);

        bus.emit('sse:reconnecting', { delay });

        this._reconnectTimer = setTimeout(() => {
            if (!this._stopped) this._open();
        }, delay);
    }

    _parse(raw) {
        try {
            return JSON.parse(raw);
        } catch {
            return { raw };
        }
    }
}

export const sseClient = new SseClient();
export default SseClient;
