let selectedWorkspaceId = null;
let globalStatsCache = {};

async function initWorkspaceSelector() {
    const selector = document.getElementById('workspaceSelector');
    if (!selector) return;

    try {
        const workspaces = await API.listWorkspaces();

        selector.innerHTML = '<option value="">Global (default)</option>';

        if (Array.isArray(workspaces) && workspaces.length > 0) {
            workspaces.forEach(ws => {
                const option = document.createElement('option');
                option.value = ws.id;
                option.textContent = ws.name || ws.id;
                selector.appendChild(option);
            });
        }
    } catch {
        selector.innerHTML = '<option value="">Global (default)</option>';
    }

    selector.addEventListener('change', handleWorkspaceChange);
}

function handleWorkspaceChange(event) {
    const workspaceId = event.target.value;

    if (!workspaceId) {
        selectedWorkspaceId = null;
        hideWorkspaceHealth();
        resetToGlobalStats();
    } else {
        selectedWorkspaceId = workspaceId;
        loadWorkspaceStats(workspaceId);
    }
}

async function loadWorkspaceStats(workspaceId) {
    try {
        const stats = await API.getWorkspaceStats(workspaceId);
        updateStatsCards(stats);
        renderWorkspaceHealth(stats);
        showWorkspaceHealth();
    } catch {
        showWorkspaceHealthError();
    }
}

function updateStatsCards(stats) {
    const nodeEl = document.getElementById('nodeCount');
    const edgeEl = document.getElementById('edgeCount');
    const embeddingEl = document.getElementById('embeddingCount');
    const itemEl = document.getElementById('itemCount');

    if (nodeEl) {
        nodeEl.textContent = formatNumber(stats.neo4j_nodes_count) ?? '-';
    }
    if (edgeEl) {
        edgeEl.textContent = formatNumber(stats.neo4j_edges_count) ?? '-';
    }
    if (embeddingEl) {
        embeddingEl.textContent = formatNumber(stats.qdrant_vectors_count) ?? '-';
    }
    if (itemEl) {
        itemEl.textContent = formatNumber(stats.pg_items_count) ?? '-';
    }
}

function cacheGlobalStats() {
    globalStatsCache = {
        nodeCount: document.getElementById('nodeCount')?.textContent,
        edgeCount: document.getElementById('edgeCount')?.textContent,
        embeddingCount: document.getElementById('embeddingCount')?.textContent,
        itemCount: document.getElementById('itemCount')?.textContent
    };
}

function resetToGlobalStats() {
    const nodeEl = document.getElementById('nodeCount');
    const edgeEl = document.getElementById('edgeCount');
    const embeddingEl = document.getElementById('embeddingCount');
    const itemEl = document.getElementById('itemCount');

    if (globalStatsCache.nodeCount && nodeEl) {
        nodeEl.textContent = globalStatsCache.nodeCount;
    }
    if (globalStatsCache.edgeCount && edgeEl) {
        edgeEl.textContent = globalStatsCache.edgeCount;
    }
    if (globalStatsCache.embeddingCount && embeddingEl) {
        embeddingEl.textContent = globalStatsCache.embeddingCount;
    }
    if (globalStatsCache.itemCount && itemEl) {
        itemEl.textContent = globalStatsCache.itemCount;
    }

    fetchStats();
}

function renderWorkspaceHealth(stats) {
    const container = document.getElementById('workspaceHealthSection');
    if (!container) return;

    const statusBadgeClass = getStatusBadgeClass(stats.status);
    const consistencyBadgeClass = getConsistencyBadgeClass(stats.consistency?.status);

    const multiLabelHealthy = (stats.isolation?.multi_label_nodes ?? 0) === 0;
    const crossEdgeHealthy = (stats.isolation?.cross_workspace_edges ?? 0) === 0;
    const labelMismatchHealthy = (stats.isolation?.label_mismatches ?? 0) === 0;

    const durationDisplay = stats.index_duration_seconds
        ? formatDuration(stats.index_duration_seconds * 1000)
        : null;

    container.innerHTML = `
        <div class="bg-dark-800 rounded-lg p-6">
            <div class="flex items-center justify-between mb-6">
                <h3 class="text-lg font-semibold text-dark-100">Workspace Health</h3>
                <div class="flex items-center space-x-3">
                    <span class="px-3 py-1 text-xs font-medium rounded-full ${statusBadgeClass}">
                        ${escapeHtml(stats.status || 'Unknown')}
                    </span>
                    <span class="px-3 py-1 text-xs font-medium rounded-full ${consistencyBadgeClass}">
                        ${escapeHtml(stats.consistency?.status || 'Unknown')}
                    </span>
                </div>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-3 gap-4 mb-4">
                ${renderIsolationCard(
                    'Multi-label Nodes',
                    stats.isolation?.multi_label_nodes ?? 0,
                    multiLabelHealthy,
                    'Nodes with multiple labels'
                )}
                ${renderIsolationCard(
                    'Cross-workspace Edges',
                    stats.isolation?.cross_workspace_edges ?? 0,
                    crossEdgeHealthy,
                    'Edges connecting different workspaces'
                )}
                ${renderIsolationCard(
                    'Label Mismatches',
                    stats.isolation?.label_mismatches ?? 0,
                    labelMismatchHealthy,
                    'Inconsistent node labels'
                )}
            </div>

            ${durationDisplay ? `
                <div class="mt-4 pt-4 border-t border-dark-700">
                    <p class="text-sm text-dark-400">
                        <span class="text-dark-300">Indexing duration:</span> ${escapeHtml(durationDisplay)}
                    </p>
                </div>
            ` : ''}
        </div>
    `;
}

function renderIsolationCard(title, count, isHealthy, description) {
    const healthyClass = 'bg-green-900/30 border border-green-700';
    const unhealthyClass = 'bg-red-900/30 border border-red-700';
    const iconColor = isHealthy ? 'text-green-500' : 'text-red-500';
    const statusIcon = isHealthy
        ? '<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7"/></svg>'
        : '<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>';

    return `
        <div class="rounded-lg p-4 ${isHealthy ? healthyClass : unhealthyClass}">
            <div class="flex items-center justify-between mb-2">
                <span class="text-sm font-medium text-dark-200">${escapeHtml(title)}</span>
                <span class="${iconColor}">${statusIcon}</span>
            </div>
            <p class="text-2xl font-semibold text-white">${formatNumber(count) ?? 0}</p>
            <p class="text-xs text-dark-400 mt-1">${escapeHtml(description)}</p>
        </div>
    `;
}

function getStatusBadgeClass(status) {
    switch (status) {
        case 'ready':
            return 'bg-green-900/30 text-green-300 border border-green-700';
        case 'indexing':
            return 'bg-blue-900/30 text-blue-300 border border-blue-700';
        case 'error':
            return 'bg-red-900/30 text-red-300 border border-red-700';
        default:
            return 'bg-dark-700 text-dark-300 border border-dark-600';
    }
}

function getConsistencyBadgeClass(status) {
    switch (status) {
        case 'consistent':
            return 'bg-green-900/30 text-green-300 border border-green-700';
        case 'inconsistent':
            return 'bg-red-900/30 text-red-300 border border-red-700';
        default:
            return 'bg-dark-700 text-dark-300 border border-dark-600';
    }
}

function showWorkspaceHealth() {
    const container = document.getElementById('workspaceHealthSection');
    if (container) {
        container.classList.remove('hidden');
    }
}

function hideWorkspaceHealth() {
    const container = document.getElementById('workspaceHealthSection');
    if (container) {
        container.classList.add('hidden');
    }
}

function showWorkspaceHealthError() {
    const container = document.getElementById('workspaceHealthSection');
    if (!container) return;

    container.innerHTML = `
        <div class="bg-dark-800 rounded-lg p-6">
            <h3 class="text-lg font-semibold text-dark-100 mb-4">Workspace Health</h3>
            <div class="bg-red-900/30 border border-red-700 rounded-lg p-4">
                <p class="text-red-400">Failed to load workspace stats. Please try refreshing.</p>
            </div>
        </div>
    `;
    container.classList.remove('hidden');
}

function getSelectedWorkspaceId() {
    return selectedWorkspaceId;
}
