/**
 * EventBus - Lightweight pub/sub event system
 */
class EventBus {
    constructor() {
        this._listeners = new Map();
    }

    /**
     * Subscribe to an event
     * @param {string} event
     * @param {Function} handler
     * @returns {Function} unsubscribe function
     */
    on(event, handler) {
        if (!this._listeners.has(event)) {
            this._listeners.set(event, new Set());
        }
        this._listeners.get(event).add(handler);
        return () => this.off(event, handler);
    }

    /**
     * Unsubscribe from an event
     * @param {string} event
     * @param {Function} handler
     */
    off(event, handler) {
        const handlers = this._listeners.get(event);
        if (handlers) {
            handlers.delete(handler);
            if (handlers.size === 0) {
                this._listeners.delete(event);
            }
        }
    }

    /**
     * Emit an event to all subscribers
     * @param {string} event
     * @param {*} data
     */
    emit(event, data) {
        const handlers = this._listeners.get(event);
        if (handlers) {
            for (const handler of handlers) {
                try {
                    handler(data);
                } catch (err) {
                    console.error(`[EventBus] Error in handler for "${event}":`, err);
                }
            }
        }
    }

    /**
     * Subscribe to an event once, auto-unsubscribes after first emit
     * @param {string} event
     * @param {Function} handler
     * @returns {Function} unsubscribe function
     */
    once(event, handler) {
        const wrapper = (data) => {
            handler(data);
            this.off(event, wrapper);
        };
        return this.on(event, wrapper);
    }
}

/** Singleton event bus shared across the app */
export const bus = new EventBus();
export default EventBus;
