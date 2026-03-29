/**
 * ApiClient - Typed fetch wrapper for all /tools/* endpoints
 * Mirrors patterns from playground/js/app.js
 */

class APIError extends Error {
    constructor(message, status, code) {
        super(message);
        this.name = 'APIError';
        this.status = status;
        this.code = code;
    }
}

class ApiClient {
    constructor(baseUrl) {
        this.baseUrl = baseUrl || window.location.origin;
        this.defaultTimeout = 30_000;
        this.chatTimeout = 600_000;  // 10 minutes for long LLM responses
    }

    // ------------------------------------------------------------------ core

    async _fetch(path, options = {}, timeout) {
        const url = path.startsWith('http') ? path : this.baseUrl + path;
        const ms = timeout ?? this.defaultTimeout;

        const controller = new AbortController();
        const tid = setTimeout(() => controller.abort(), ms);

        try {
            const response = await fetch(url, {
                headers: { 'Content-Type': 'application/json', ...options.headers },
                ...options,
                signal: controller.signal,
            });

            clearTimeout(tid);

            if (!response.ok) {
                const body = await response.json().catch(() => ({}));
                throw new APIError(
                    body.error || `HTTP ${response.status}`,
                    response.status,
                    body.code || 'HTTP_ERROR',
                );
            }

            if (response.status === 204) return {};
            return await response.json();
        } catch (err) {
            clearTimeout(tid);
            if (err.name === 'AbortError') throw new APIError('Request timeout', 408, 'TIMEOUT');
            if (err instanceof APIError) throw err;
            throw new APIError(err.message, 0, 'NETWORK_ERROR');
        }
    }

    _get(path, params = {}, timeout) {
        const qs = new URLSearchParams(
            Object.fromEntries(
                Object.entries(params)
                    .filter(([, v]) => v !== undefined && v !== null)
                    .map(([k, v]) => [k, String(v)]),
            ),
        ).toString();
        return this._fetch(qs ? `${path}?${qs}` : path, {}, timeout);
    }

    _post(path, body, timeout) {
        return this._fetch(path, { method: 'POST', body: JSON.stringify(body) }, timeout);
    }

    // ------------------------------------------------------------------ health

    getHealth() {
        return this._get('/health');
    }

    // ------------------------------------------------------------------ ingestion

    getIngestionProgress() {
        return this._get('/api/ingestion/progress');
    }

    // ------------------------------------------------------------------ search

    /**
     * @param {string} query
     * @param {{ limit?: number, scoreThreshold?: number, crateFilter?: string }} opts
     */
    searchSemantic(query, opts = {}) {
        return this._post('/tools/search_semantic', {
            query,
            limit: opts.limit ?? 10,
            score_threshold: opts.scoreThreshold,
            crate_filter: opts.crateFilter,
        });
    }

    /**
     * Aggregate search across all stores
     * @param {string} query
     * @param {{ limit?: number }} opts
     */
    aggregateSearch(query, opts = {}) {
        return this._post('/tools/aggregate_search', {
            query,
            limit: opts.limit ?? 10,
            include_source: opts.includeSource ?? false,
            include_graph: false,
        });
    }

    // ------------------------------------------------------------------ graph / code

    /** @param {string} fqn */
    getFunction(fqn) {
        return this._get('/tools/get_function', { fqn });
    }

    /**
     * @param {string} fqn
     * @param {number} depth
     */
    getCallers(fqn, depth = 1) {
        return this._get('/tools/get_callers', { fqn, depth });
    }

    /**
     * @param {string} traitName
     * @param {number} limit
     */
    getTraitImpls(traitName, limit = 10) {
        return this._get('/tools/get_trait_impls', { trait_name: traitName, limit });
    }

    /**
     * @param {string} typeName
     * @param {number} limit
     */
    findTypeUsages(typeName, limit = 10) {
        return this._get('/tools/find_usages_of_type', { type_name: typeName, limit });
    }

    /** @param {string} crateName */
    getModuleTree(crateName) {
        return this._get('/tools/get_module_tree', { crate_name: crateName });
    }

    /**
     * @param {string} query  Cypher query
     * @param {object} parameters
     * @param {number} limit
     */
    queryGraph(query, parameters = {}, limit = 10) {
        return this._post('/tools/query_graph', { query, parameters, limit });
    }

    // ------------------------------------------------------------------ typecheck (PostgreSQL)

    /**
     * Find call sites with a specific type argument (turbofish)
     * @param {string} typeName - Type name to search for in concrete_type_args
     * @param {object} opts - Optional filters
     * @param {string} opts.calleeName - Filter by callee function name
     * @param {number} opts.limit - Max results (default: 20)
     */
    findCallsWithType(typeName, opts = {}) {
        return this._get('/tools/find_calls_with_type', {
            type_name: typeName,
            callee_name: opts.calleeName,
            limit: opts.limit ?? 20,
        });
    }

    /**
     * Find all trait implementations for a specific type
     * @param {string} typeName - Type name to search for in self_type
     * @param {number} limit - Max results (default: 20)
     */
    findTraitImplsForType(typeName, limit = 20) {
        return this._get('/tools/find_trait_impls_for_type', {
            type_name: typeName,
            limit,
        });
    }

    // ------------------------------------------------------------------ chat

    /**
     * Blocking chat (returns full response)
     * @param {string} sessionId
     * @param {string} message
     * @param {object} opts
     */
    sendChat(sessionId, message, opts = {}) {
        return this._post(
            '/tools/chat',
            { session_id: sessionId, message, ...opts },
            this.chatTimeout,
        );
    }

    /**
     * Async chat (returns job ID, poll or stream for result)
     * @param {string} sessionId
     * @param {string} message
     * @param {object} opts
     */
    sendChatAsync(sessionId, message, opts = {}) {
        return this._post(
            '/tools/chat/send',
            { session_id: sessionId, message, ...opts },
            this.chatTimeout,
        );
    }

    // ------------------------------------------------------------------ sessions

    createSession(opts = {}) {
        return this._post('/tools/chat/sessions', opts);
    }

    listSessions() {
        return this._get('/tools/chat/sessions');
    }

    /** @param {string} id */
    getSession(id) {
        return this._get(`/tools/chat/sessions/${encodeURIComponent(id)}`);
    }

    /** @param {string} id */
    deleteSession(id) {
        return this._fetch(`/tools/chat/sessions/${encodeURIComponent(id)}`, { method: 'DELETE' });
    }

    /**
     * @param {string} id
     * @param {object} opts
     */
    forkSession(id, opts = {}) {
        return this._post(`/tools/chat/sessions/${encodeURIComponent(id)}/fork`, opts);
    }

    /** @param {string} id */
    abortSession(id) {
        return this._post(`/tools/chat/sessions/${encodeURIComponent(id)}/abort`, {});
    }
}

export { APIError };
export const apiClient = new ApiClient();
export default ApiClient;
