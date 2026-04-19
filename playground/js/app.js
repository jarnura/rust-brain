/**
 * Rust Brain Playground - Shared JavaScript
 * Common utilities and API helpers
 */

// Configuration - use current host for all API calls
const _host = window.location.hostname;
const CONFIG = {
    apiBase: `http://${_host}:8088`,
    qdrantBase: `http://${_host}:6333`,
    neo4jBase: `http://${_host}:7474`,
    ollamaBase: `http://${_host}:11434`,
    grafanaBase: `http://${_host}:3000`,
    timeout: 30000
};

/**
 * Fetch wrapper with error handling
 */
async function fetchAPI(endpoint, options = {}) {
    const url = endpoint.startsWith('http') ? endpoint : CONFIG.apiBase + endpoint;
    
    const defaultOptions = {
        headers: {
            'Content-Type': 'application/json',
            ...options.headers
        }
    };

    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), CONFIG.timeout);
    
    try {
        const response = await fetch(url, {
            ...defaultOptions,
            ...options,
            signal: controller.signal
        });

        clearTimeout(timeoutId);

        if (!response.ok) {
            const errorData = await response.json().catch(() => ({}));
            throw new APIError(
                errorData.error || `HTTP ${response.status}`,
                response.status,
                errorData.code || 'HTTP_ERROR'
            );
        }

        return await response.json();
    } catch (error) {
        clearTimeout(timeoutId);
        if (error.name === 'AbortError') {
            throw new APIError('Request timeout', 408, 'TIMEOUT');
        }
        if (error instanceof APIError) {
            throw error;
        }
        throw new APIError(error.message, 0, 'NETWORK_ERROR');
    }
}

/**
 * Custom API Error class
 */
class APIError extends Error {
    constructor(message, status, code) {
        super(message);
        this.name = 'APIError';
        this.status = status;
        this.code = code;
    }
}

/**
 * Show notification toast
 */
function showNotification(message, type = 'info') {
    // Remove existing notification
    const existing = document.querySelector('.notification-toast');
    if (existing) existing.remove();

    const icons = {
        success: '✓',
        error: '✗',
        warning: '⚠',
        info: 'ℹ'
    };

    const notification = document.createElement('div');
    notification.className = `notification-toast ${type} rounded-lg px-4 py-3 shadow-lg border flex items-center space-x-3`;
    notification.innerHTML = `
        <span class="text-lg">${icons[type] || icons.info}</span>
        <span class="text-sm text-dark-200">${escapeHtml(message)}</span>
    `;

    document.body.appendChild(notification);

    // Auto-remove after 3 seconds
    setTimeout(() => {
        notification.style.opacity = '0';
        notification.style.transform = 'translateX(20px)';
        setTimeout(() => notification.remove(), 300);
    }, 3000);
}

/**
 * Format timestamp for display
 */
function formatTimestamp(timestamp) {
    const date = new Date(timestamp);
    const now = new Date();
    const diff = now - date;

    // Less than 1 minute
    if (diff < 60000) {
        return 'just now';
    }
    // Less than 1 hour
    if (diff < 3600000) {
        const mins = Math.floor(diff / 60000);
        return `${mins} min${mins > 1 ? 's' : ''} ago`;
    }
    // Less than 24 hours
    if (diff < 86400000) {
        const hours = Math.floor(diff / 3600000);
        return `${hours} hour${hours > 1 ? 's' : ''} ago`;
    }
    // Less than 7 days
    if (diff < 604800000) {
        const days = Math.floor(diff / 86400000);
        return `${days} day${days > 1 ? 's' : ''} ago`;
    }
    // Otherwise show date
    return date.toLocaleDateString('en-US', {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit'
    });
}

/**
 * Format number with commas
 */
function formatNumber(num) {
    if (num === null || num === undefined) return '-';
    return num.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/**
 * Format bytes to human readable
 */
function formatBytes(bytes, decimals = 2) {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(decimals)) + ' ' + sizes[i];
}

/**
 * Format duration in milliseconds
 */
function formatDuration(ms) {
    if (ms < 1000) return `${ms}ms`;
    if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
    const mins = Math.floor(ms / 60000);
    const secs = Math.floor((ms % 60000) / 1000);
    return `${mins}m ${secs}s`;
}

/**
 * Escape HTML to prevent XSS
 */
function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

/**
 * Debounce function
 */
function debounce(func, wait) {
    let timeout;
    return function executedFunction(...args) {
        const later = () => {
            clearTimeout(timeout);
            func(...args);
        };
        clearTimeout(timeout);
        timeout = setTimeout(later, wait);
    };
}

/**
 * Throttle function
 */
function throttle(func, limit) {
    let inThrottle;
    return function(...args) {
        if (!inThrottle) {
            func.apply(this, args);
            inThrottle = true;
            setTimeout(() => inThrottle = false, limit);
        }
    };
}

/**
 * Update timestamp display
 */
function updateTimestamp() {
    const el = document.getElementById('lastUpdated');
    if (el) {
        el.textContent = `Last updated: ${new Date().toLocaleTimeString()}`;
    }
}

/**
 * Copy text to clipboard
 */
async function copyToClipboard(text) {
    try {
        await navigator.clipboard.writeText(text);
        return true;
    } catch (err) {
        // Fallback for older browsers
        const textarea = document.createElement('textarea');
        textarea.value = text;
        textarea.style.position = 'fixed';
        textarea.style.opacity = '0';
        document.body.appendChild(textarea);
        textarea.select();
        try {
            document.execCommand('copy');
            document.body.removeChild(textarea);
            return true;
        } catch (e) {
            document.body.removeChild(textarea);
            return false;
        }
    }
}

/**
 * Parse and safely evaluate JSON path
 */
function getNestedValue(obj, path, defaultValue = null) {
    const keys = path.split('.');
    let result = obj;
    for (const key of keys) {
        if (result === null || result === undefined) return defaultValue;
        result = result[key];
    }
    return result !== undefined ? result : defaultValue;
}

/**
 * Generate unique ID
 */
function generateId() {
    return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

/**
 * Local storage helpers with error handling
 */
const storage = {
    get(key, defaultValue = null) {
        try {
            const item = localStorage.getItem(key);
            return item ? JSON.parse(item) : defaultValue;
        } catch {
            return defaultValue;
        }
    },
    set(key, value) {
        try {
            localStorage.setItem(key, JSON.stringify(value));
            return true;
        } catch {
            return false;
        }
    },
    remove(key) {
        try {
            localStorage.removeItem(key);
            return true;
        } catch {
            return false;
        }
    }
};

/**
 * API endpoint helpers
 */
const API = {
    // Health check
    async health() {
        return fetchAPI('/health');
    },

    // Semantic search
    async searchSemantic(query, options = {}) {
        return fetchAPI('/tools/search_semantic', {
            method: 'POST',
            body: JSON.stringify({
                query,
                limit: options.limit || 10,
                score_threshold: options.scoreThreshold,
                crate_filter: options.crateFilter
            })
        });
    },

    // Get function details
    async getFunction(fqn) {
        return fetchAPI(`/tools/get_function?fqn=${encodeURIComponent(fqn)}`);
    },

    // Get callers
    async getCallers(fqn, depth = 1) {
        return fetchAPI(`/tools/get_callers?fqn=${encodeURIComponent(fqn)}&depth=${depth}`);
    },

    // Get trait implementations
    async getTraitImpls(traitName, limit = 10) {
        return fetchAPI(`/tools/get_trait_impls?trait_name=${encodeURIComponent(traitName)}&limit=${limit}`);
    },

    // Find type usages
    async findUsages(typeName, limit = 10) {
        return fetchAPI(`/tools/find_usages_of_type?type_name=${encodeURIComponent(typeName)}&limit=${limit}`);
    },

    // Get module tree
    async getModuleTree(crateName) {
        return fetchAPI(`/tools/get_module_tree?crate_name=${encodeURIComponent(crateName)}`);
    },

    // Query graph (Cypher)
    async queryGraph(query, parameters = {}, limit = 10) {
        return fetchAPI('/tools/query_graph', {
            method: 'POST',
            body: JSON.stringify({ query, parameters, limit })
        });
    },

    async listWorkspaces() {
        return fetchAPI('/workspaces');
    },

    async getWorkspaceStats(id) {
        return fetchAPI(`/workspaces/${encodeURIComponent(id)}/stats`);
    }
};

/**
 * Initialize common functionality
 */
function initializeCommon() {
    // Add keyboard shortcuts
    document.addEventListener('keydown', (e) => {
        // Ctrl/Cmd + K to focus search
        if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
            e.preventDefault();
            const searchInput = document.querySelector('#searchInput, input[type="search"], input[placeholder*="search" i]');
            if (searchInput) searchInput.focus();
        }
        // Escape to close modals
        if (e.key === 'Escape') {
            const modal = document.querySelector('.modal.active');
            if (modal) modal.classList.remove('active');
        }
    });

    // Update timestamps periodically
    setInterval(() => {
        document.querySelectorAll('[data-timestamp]').forEach(el => {
            el.textContent = formatTimestamp(el.dataset.timestamp);
        });
    }, 60000);
}

// Run initialization when DOM is ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initializeCommon);
} else {
    initializeCommon();
}

// Export for modules
if (typeof module !== 'undefined' && module.exports) {
    module.exports = {
        CONFIG,
        fetchAPI,
        APIError,
        showNotification,
        formatTimestamp,
        formatNumber,
        formatBytes,
        formatDuration,
        escapeHtml,
        debounce,
        throttle,
        updateTimestamp,
        copyToClipboard,
        getNestedValue,
        generateId,
        storage,
        API
    };
}
