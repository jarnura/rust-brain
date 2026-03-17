/**
 * AppState - Immutable application state with event emission
 */
import { bus } from './event-bus.js';

const INITIAL_STATE = {
    currentSession: null,
    sessions: [],
    activePanel: 'dashboard',
    searchResults: null,
    functionDetail: null,
    callGraph: null,
    connected: false,
};

class AppState {
    constructor(initialState = INITIAL_STATE) {
        this._state = Object.freeze({ ...initialState });
    }

    /**
     * Get current state snapshot (immutable)
     * @returns {Readonly<object>}
     */
    get() {
        return this._state;
    }

    /**
     * Get a single key from state
     * @param {string} key
     * @returns {*}
     */
    getKey(key) {
        return this._state[key];
    }

    /**
     * Update one or more keys immutably.
     * Emits `state:changed` with { prev, next, changed } and
     * `state:<key>` for each changed key.
     * @param {Partial<object>} updates
     */
    set(updates) {
        const prev = this._state;
        const next = Object.freeze({ ...prev, ...updates });
        this._state = next;

        const changed = Object.keys(updates).filter(k => prev[k] !== next[k]);
        if (changed.length === 0) return;

        bus.emit('state:changed', { prev, next, changed });

        for (const key of changed) {
            bus.emit(`state:${key}`, { prev: prev[key], next: next[key] });
        }
    }

    /**
     * Reset state to initial values
     */
    reset() {
        this.set({ ...INITIAL_STATE });
    }

    // --- Convenience setters ---

    setCurrentSession(session) {
        this.set({ currentSession: session });
    }

    setSessions(sessions) {
        this.set({ sessions: [...sessions] });
    }

    setActivePanel(panel) {
        this.set({ activePanel: panel });
    }

    setSearchResults(results) {
        this.set({ searchResults: results });
    }

    setFunctionDetail(detail) {
        this.set({ functionDetail: detail });
    }

    setCallGraph(graph) {
        this.set({ callGraph: graph });
    }

    setConnected(connected) {
        this.set({ connected });
    }
}

/** Singleton app state */
export const state = new AppState();
export default AppState;
