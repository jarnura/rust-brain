/**
 * Rust Brain Benchmarker Dashboard
 * Visualizes eval suite results: aggregate scores, per-dimension breakdown,
 * case list, release comparison, and regression drill-down.
 *
 * Data source: GET /bench/runs  and  GET /bench/runs/:id
 * Falls back to mock data when the run manager API is unavailable.
 */

'use strict';

// ── Config ────────────────────────────────────────────────────────────────────

const _host = window.location.hostname;
const BENCH_API = `http://${_host}:8088`;

// ── Mock data (used when /bench/runs is unreachable) ──────────────────────────

const MOCK_RUNS = [
    {
        id: 'run-v050',
        suite_name: 'default',
        release_tag: 'v0.5.0',
        status: 'completed',
        total_cases: 12,
        completed_cases: 12,
        pass_count: 10,
        pass_rate: 0.833,
        mean_composite: 3.71,
        total_cost_usd: 0.42,
        started_at: '2026-04-01T10:00:00Z',
        completed_at: '2026-04-01T10:14:32Z',
    },
    {
        id: 'run-v040',
        suite_name: 'default',
        release_tag: 'v0.4.0',
        status: 'completed',
        total_cases: 12,
        completed_cases: 12,
        pass_count: 9,
        pass_rate: 0.75,
        mean_composite: 3.42,
        total_cost_usd: 0.39,
        started_at: '2026-03-18T09:00:00Z',
        completed_at: '2026-03-18T09:13:10Z',
    },
];

// per-dimension scores for a run
const MOCK_DIMENSIONS = {
    'run-v050': { correctness: 3.9, security: 3.8, completeness: 3.7, efficiency: 3.5, style: 3.6 },
    'run-v040': { correctness: 3.5, security: 3.6, completeness: 3.4, efficiency: 3.3, style: 3.4 },
};

const MOCK_CASES = {
    'run-v050': [
        { eval_case_id: 'add-workspace-endpoint',   composite: 4.1, pass: true,  cost_usd: 0.038, run1: 4.2, run2: 4.0, tags: ['workspace','api'] },
        { eval_case_id: 'fix-cypher-injection',      composite: 3.9, pass: true,  cost_usd: 0.035, run1: 4.0, run2: 3.8, tags: ['security','neo4j'] },
        { eval_case_id: 'add-sse-streaming',         composite: 3.7, pass: true,  cost_usd: 0.033, run1: 3.7, run2: 3.7, tags: ['api','streaming'] },
        { eval_case_id: 'refactor-ingestion-stages', composite: 3.5, pass: true,  cost_usd: 0.040, run1: 3.6, run2: 3.4, tags: ['ingestion'] },
        { eval_case_id: 'impl-rate-limiting',        composite: 3.8, pass: true,  cost_usd: 0.031, run1: 3.9, run2: 3.7, tags: ['api','security'] },
        { eval_case_id: 'add-trait-impl-endpoint',   composite: 3.6, pass: true,  cost_usd: 0.029, run1: 3.6, run2: 3.6, tags: ['api','graph'] },
        { eval_case_id: 'fix-neo4j-pool',            composite: 2.1, pass: false, cost_usd: 0.036, run1: 2.0, run2: 2.2, tags: ['neo4j','infra'] },
        { eval_case_id: 'add-batch-embed',           composite: 3.4, pass: true,  cost_usd: 0.034, run1: 3.5, run2: 3.3, tags: ['embedding'] },
        { eval_case_id: 'cleanup-dead-code',         composite: 3.2, pass: true,  cost_usd: 0.028, run1: 3.3, run2: 3.1, tags: ['refactor'] },
        { eval_case_id: 'add-metrics-endpoint',      composite: 3.9, pass: true,  cost_usd: 0.030, run1: 4.0, run2: 3.8, tags: ['api'] },
        { eval_case_id: 'fix-sqlx-migration',        composite: 2.3, pass: false, cost_usd: 0.037, run1: 2.4, run2: 2.2, tags: ['postgres'] },
        { eval_case_id: 'reverted-wrong-approach',   composite: 3.1, pass: true,  cost_usd: 0.031, run1: 3.1, run2: 3.1, tags: ['reject'] },
    ],
    'run-v040': [
        { eval_case_id: 'add-workspace-endpoint',   composite: 3.8, pass: true,  cost_usd: 0.036, run1: 3.9, run2: 3.7, tags: ['workspace','api'] },
        { eval_case_id: 'fix-cypher-injection',      composite: 2.1, pass: false, cost_usd: 0.034, run1: 2.0, run2: 2.2, tags: ['security','neo4j'] },
        { eval_case_id: 'add-sse-streaming',         composite: 3.5, pass: true,  cost_usd: 0.032, run1: 3.5, run2: 3.5, tags: ['api','streaming'] },
        { eval_case_id: 'refactor-ingestion-stages', composite: 3.3, pass: true,  cost_usd: 0.039, run1: 3.4, run2: 3.2, tags: ['ingestion'] },
        { eval_case_id: 'impl-rate-limiting',        composite: 3.2, pass: true,  cost_usd: 0.030, run1: 3.3, run2: 3.1, tags: ['api','security'] },
        { eval_case_id: 'add-trait-impl-endpoint',   composite: 3.4, pass: true,  cost_usd: 0.028, run1: 3.4, run2: 3.4, tags: ['api','graph'] },
        { eval_case_id: 'fix-neo4j-pool',            composite: 3.5, pass: true,  cost_usd: 0.035, run1: 3.6, run2: 3.4, tags: ['neo4j','infra'] },
        { eval_case_id: 'add-batch-embed',           composite: 3.2, pass: true,  cost_usd: 0.033, run1: 3.3, run2: 3.1, tags: ['embedding'] },
        { eval_case_id: 'cleanup-dead-code',         composite: 3.0, pass: true,  cost_usd: 0.027, run1: 3.1, run2: 2.9, tags: ['refactor'] },
        { eval_case_id: 'add-metrics-endpoint',      composite: 3.7, pass: true,  cost_usd: 0.029, run1: 3.8, run2: 3.6, tags: ['api'] },
        { eval_case_id: 'fix-sqlx-migration',        composite: 3.4, pass: true,  cost_usd: 0.036, run1: 3.5, run2: 3.3, tags: ['postgres'] },
        { eval_case_id: 'reverted-wrong-approach',   composite: 2.9, pass: false, cost_usd: 0.030, run1: 2.8, run2: 3.0, tags: ['reject'] },
    ],
};

// ── State ─────────────────────────────────────────────────────────────────────

const state = {
    usingMockData: false,
    runs: [],
    currentRunId: null,
    compareRunId: null,
    currentRun: null,       // full run detail (includes cases + dimensions)
    compareRun: null,
    activeTab: 'overview',
    dimChart: null,         // Chart.js instance
};

// ── API helpers ───────────────────────────────────────────────────────────────

async function apiFetch(path) {
    const res = await fetch(BENCH_API + path, {
        headers: { 'Content-Type': 'application/json' },
        signal: AbortSignal.timeout(8000),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    return res.json();
}

async function loadRuns() {
    try {
        const runs = await apiFetch('/bench/runs');
        state.usingMockData = false;
        return runs;
    } catch {
        state.usingMockData = true;
        return MOCK_RUNS;
    }
}

async function loadRunDetail(runId) {
    if (state.usingMockData) {
        const run = MOCK_RUNS.find(r => r.id === runId);
        return {
            ...run,
            dimensions: MOCK_DIMENSIONS[runId] || {},
            cases: MOCK_CASES[runId] || [],
        };
    }
    try {
        return await apiFetch(`/bench/runs/${runId}`);
    } catch {
        state.usingMockData = true;
        const run = MOCK_RUNS.find(r => r.id === runId);
        return {
            ...run,
            dimensions: MOCK_DIMENSIONS[runId] || {},
            cases: MOCK_CASES[runId] || [],
        };
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

function escapeHtml(str) {
    if (str === null || str === undefined) return '';
    return String(str)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
}

function fmtDate(iso) {
    if (!iso) return '—';
    return new Date(iso).toLocaleString('en-US', {
        month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit',
    });
}

function fmtPct(v) {
    if (v === null || v === undefined) return '—';
    return (v * 100).toFixed(1) + '%';
}

function fmtScore(v) {
    if (v === null || v === undefined) return '—';
    return Number(v).toFixed(2);
}

function fmtCost(v) {
    if (v === null || v === undefined) return '—';
    return '$' + Number(v).toFixed(4);
}

function passChip(pass) {
    return pass
        ? '<span class="chip-pass">PASS</span>'
        : '<span class="chip-fail">FAIL</span>';
}

function variance(r) {
    if (r.run1 === undefined) return '—';
    return Math.abs(r.run1 - r.run2).toFixed(2);
}

// ── Mock data notice ──────────────────────────────────────────────────────────

function renderMockNotice() {
    const el = document.getElementById('mockNotice');
    el.classList.toggle('hidden', !state.usingMockData);
}

// ── Run selector ──────────────────────────────────────────────────────────────

function populateRunSelectors() {
    const primary = document.getElementById('runSelector');
    const compare = document.getElementById('compareSelector');

    const makeOption = (run) =>
        `<option value="${escapeHtml(run.id)}">${escapeHtml(run.release_tag || run.id)} — ${fmtDate(run.completed_at)}</option>`;

    primary.innerHTML = state.runs.map(makeOption).join('');
    compare.innerHTML = '<option value="">— choose baseline —</option>' +
        state.runs.map(makeOption).join('');

    // Default: newest = current, second newest = compare
    if (state.runs.length > 0) {
        state.currentRunId = state.runs[0].id;
        primary.value = state.currentRunId;
    }
    if (state.runs.length > 1) {
        state.compareRunId = state.runs[1].id;
        compare.value = state.compareRunId;
    }
}

// ── Tab navigation ────────────────────────────────────────────────────────────

function activateTab(tab) {
    state.activeTab = tab;
    document.querySelectorAll('.bench-tab').forEach(btn => {
        btn.classList.toggle('tab-active', btn.dataset.tab === tab);
    });
    document.querySelectorAll('.tab-panel').forEach(panel => {
        panel.classList.toggle('hidden', panel.id !== 'panel-' + tab);
    });

    if (tab === 'dimension' && state.currentRun) renderDimChart();
    if (tab === 'compare' && state.currentRun && state.compareRun) renderCompare();
}

// ── Overview panel ────────────────────────────────────────────────────────────

function renderOverview(run) {
    const el = document.getElementById('overviewStats');
    if (!run) { el.innerHTML = '<p class="text-dark-400">No run selected.</p>'; return; }

    el.innerHTML = `
        <div class="stat-card">
            <p class="stat-label">Pass Rate</p>
            <p class="stat-value text-green-400">${fmtPct(run.pass_rate)}</p>
            <p class="stat-sub">${run.pass_count} / ${run.total_cases} cases</p>
        </div>
        <div class="stat-card">
            <p class="stat-label">Mean Composite</p>
            <p class="stat-value text-indigo-400">${fmtScore(run.mean_composite)}</p>
            <p class="stat-sub">out of 5.0</p>
        </div>
        <div class="stat-card">
            <p class="stat-label">Total Cost</p>
            <p class="stat-value text-yellow-400">${fmtCost(run.total_cost_usd)}</p>
            <p class="stat-sub">${run.total_cases} cases × 2 runs</p>
        </div>
        <div class="stat-card">
            <p class="stat-label">Run Date</p>
            <p class="stat-value text-dark-200 text-xl">${fmtDate(run.completed_at || run.started_at)}</p>
            <p class="stat-sub">Suite: ${escapeHtml(run.suite_name)}</p>
        </div>
    `;

    // Progress bar
    const pct = run.total_cases > 0 ? (run.pass_count / run.total_cases) * 100 : 0;
    const bar = document.getElementById('passRateBar');
    bar.style.width = pct.toFixed(1) + '%';
    bar.className = 'pass-rate-fill ' + (pct >= 80 ? 'fill-green' : pct >= 60 ? 'fill-yellow' : 'fill-red');
    document.getElementById('passRateLabel').textContent =
        `${fmtPct(run.pass_rate)} pass rate (${run.release_tag || run.id})`;
}

// ── Per-dimension chart ───────────────────────────────────────────────────────

function renderDimChart() {
    const run = state.currentRun;
    if (!run || !run.dimensions) return;

    const dims = run.dimensions;
    const labels = Object.keys(dims).map(d => d.charAt(0).toUpperCase() + d.slice(1));
    const values = Object.values(dims);

    const canvas = document.getElementById('dimChart');
    const ctx = canvas.getContext('2d');

    if (state.dimChart) state.dimChart.destroy();

    state.dimChart = new Chart(ctx, {
        type: 'bar',
        data: {
            labels,
            datasets: [{
                label: run.release_tag || run.id,
                data: values,
                backgroundColor: 'rgba(99, 102, 241, 0.7)',
                borderColor: 'rgba(99, 102, 241, 1)',
                borderWidth: 1,
                borderRadius: 4,
            }],
        },
        options: {
            responsive: true,
            maintainAspectRatio: false,
            scales: {
                y: {
                    min: 0, max: 5,
                    ticks: { color: '#a1a1aa', stepSize: 1 },
                    grid: { color: '#2a2a2a' },
                },
                x: {
                    ticks: { color: '#a1a1aa' },
                    grid: { display: false },
                },
            },
            plugins: {
                legend: { display: false },
                tooltip: {
                    callbacks: {
                        label: ctx => ` ${ctx.parsed.y.toFixed(2)} / 5.0`,
                    },
                },
            },
        },
    });
}

// ── Case list panel ───────────────────────────────────────────────────────────

function renderCases(run) {
    const tbody = document.getElementById('casesTbody');
    if (!run || !run.cases || run.cases.length === 0) {
        tbody.innerHTML = '<tr><td colspan="6" class="text-center text-dark-400 py-8">No cases.</td></tr>';
        return;
    }

    tbody.innerHTML = run.cases.map(c => `
        <tr class="case-row hover:bg-dark-700 cursor-pointer" data-case-id="${escapeHtml(c.eval_case_id)}" onclick="openDrillDown('${escapeHtml(c.eval_case_id)}')">
            <td class="px-4 py-3 font-mono text-sm text-dark-200">${escapeHtml(c.eval_case_id)}</td>
            <td class="px-4 py-3 text-center font-semibold ${c.composite >= 3.5 ? 'text-green-400' : c.composite >= 2.5 ? 'text-yellow-400' : 'text-red-400'}">${fmtScore(c.composite)}</td>
            <td class="px-4 py-3 text-center">${passChip(c.pass)}</td>
            <td class="px-4 py-3 text-right text-dark-300 font-mono text-sm">${fmtCost(c.cost_usd)}</td>
            <td class="px-4 py-3 text-right text-dark-400 font-mono text-sm">${variance(c)}</td>
            <td class="px-4 py-3">${(c.tags || []).map(t => `<span class="tag">${escapeHtml(t)}</span>`).join('')}</td>
        </tr>
    `).join('');
}

// ── Release comparison panel ──────────────────────────────────────────────────

function renderCompare() {
    const curr = state.currentRun;
    const base = state.compareRun;
    const el = document.getElementById('compareContent');

    if (!curr || !base) {
        el.innerHTML = '<p class="text-dark-400">Select both a current run and a baseline to compare.</p>';
        return;
    }

    // Build case map for baseline
    const baseMap = {};
    (base.cases || []).forEach(c => { baseMap[c.eval_case_id] = c; });

    const rows = (curr.cases || []).map(c => {
        const b = baseMap[c.eval_case_id];
        if (!b) return null;

        const delta = c.composite - b.composite;
        const wasPass = b.pass;
        const nowPass = c.pass;

        let rowClass = '';
        let status = '';
        if (wasPass && !nowPass) { rowClass = 'compare-regression'; status = '<span class="badge-regression">REGRESSION</span>'; }
        else if (!wasPass && nowPass) { rowClass = 'compare-improvement'; status = '<span class="badge-improvement">IMPROVED</span>'; }
        else { status = '<span class="badge-unchanged">—</span>'; }

        const deltaStr = (delta >= 0 ? '+' : '') + delta.toFixed(2);
        const deltaClass = delta > 0.05 ? 'text-green-400' : delta < -0.05 ? 'text-red-400' : 'text-dark-400';

        return `
            <tr class="${rowClass} hover:bg-dark-700 cursor-pointer" onclick="openDrillDown('${escapeHtml(c.eval_case_id)}')">
                <td class="px-4 py-3 font-mono text-sm text-dark-200">${escapeHtml(c.eval_case_id)}</td>
                <td class="px-4 py-3 text-center">${passChip(b.pass)}</td>
                <td class="px-4 py-3 text-center">${passChip(c.pass)}</td>
                <td class="px-4 py-3 text-right font-mono text-sm text-dark-300">${fmtScore(b.composite)}</td>
                <td class="px-4 py-3 text-right font-mono text-sm text-dark-300">${fmtScore(c.composite)}</td>
                <td class="px-4 py-3 text-right font-mono text-sm font-semibold ${deltaClass}">${deltaStr}</td>
                <td class="px-4 py-3 text-center">${status}</td>
            </tr>
        `;
    }).filter(Boolean).join('');

    // Summary
    const regressions = (curr.cases || []).filter(c => {
        const b = baseMap[c.eval_case_id];
        return b && b.pass && !c.pass;
    });
    const improvements = (curr.cases || []).filter(c => {
        const b = baseMap[c.eval_case_id];
        return b && !b.pass && c.pass;
    });

    el.innerHTML = `
        <div class="flex gap-4 mb-6 flex-wrap">
            <div class="compare-summary-card border-red-800">
                <p class="text-sm text-dark-400">Regressions</p>
                <p class="text-2xl font-bold text-red-400">${regressions.length}</p>
            </div>
            <div class="compare-summary-card border-green-800">
                <p class="text-sm text-dark-400">Improvements</p>
                <p class="text-2xl font-bold text-green-400">${improvements.length}</p>
            </div>
            <div class="compare-summary-card border-dark-600">
                <p class="text-sm text-dark-400">Pass Rate</p>
                <p class="text-lg font-semibold">
                    <span class="text-dark-400">${fmtPct(base.pass_rate)}</span>
                    <span class="text-dark-500 mx-2">→</span>
                    <span class="${curr.pass_rate >= base.pass_rate ? 'text-green-400' : 'text-red-400'}">${fmtPct(curr.pass_rate)}</span>
                </p>
            </div>
            <div class="compare-summary-card border-dark-600">
                <p class="text-sm text-dark-400">Mean Composite</p>
                <p class="text-lg font-semibold">
                    <span class="text-dark-400">${fmtScore(base.mean_composite)}</span>
                    <span class="text-dark-500 mx-2">→</span>
                    <span class="${curr.mean_composite >= base.mean_composite ? 'text-green-400' : 'text-red-400'}">${fmtScore(curr.mean_composite)}</span>
                </p>
            </div>
        </div>
        <div class="overflow-x-auto">
            <table class="bench-table w-full">
                <thead>
                    <tr>
                        <th class="text-left px-4 py-2">Case</th>
                        <th class="text-center px-4 py-2">Baseline</th>
                        <th class="text-center px-4 py-2">Current</th>
                        <th class="text-right px-4 py-2">Score (base)</th>
                        <th class="text-right px-4 py-2">Score (curr)</th>
                        <th class="text-right px-4 py-2">Delta</th>
                        <th class="text-center px-4 py-2">Status</th>
                    </tr>
                </thead>
                <tbody>${rows}</tbody>
            </table>
        </div>
    `;
}

// ── Regression drill-down modal ───────────────────────────────────────────────

function openDrillDown(caseId) {
    const run = state.currentRun;
    if (!run) return;

    const c = (run.cases || []).find(x => x.eval_case_id === caseId);
    if (!c) return;

    const base = state.compareRun
        ? (state.compareRun.cases || []).find(x => x.eval_case_id === caseId)
        : null;

    const modal = document.getElementById('drillDownModal');
    const content = document.getElementById('drillDownContent');

    const baseSection = base ? `
        <div class="drill-section">
            <h4 class="drill-section-title">Baseline (${escapeHtml(state.compareRun.release_tag || state.compareRun.id)})</h4>
            <div class="grid grid-cols-2 gap-3">
                <div class="drill-stat"><p class="stat-label">Composite</p><p class="stat-val">${fmtScore(base.composite)}</p></div>
                <div class="drill-stat"><p class="stat-label">Result</p><p>${passChip(base.pass)}</p></div>
                <div class="drill-stat"><p class="stat-label">Run 1</p><p class="stat-val">${fmtScore(base.run1)}</p></div>
                <div class="drill-stat"><p class="stat-label">Run 2</p><p class="stat-val">${fmtScore(base.run2)}</p></div>
            </div>
        </div>
    ` : '';

    const delta = base ? (c.composite - base.composite) : null;
    const deltaHtml = delta !== null
        ? `<span class="ml-2 ${delta >= 0 ? 'text-green-400' : 'text-red-400'}">${delta >= 0 ? '+' : ''}${delta.toFixed(2)}</span>`
        : '';

    content.innerHTML = `
        <div class="flex justify-between items-start mb-6">
            <div>
                <h3 class="text-lg font-semibold text-white font-mono">${escapeHtml(caseId)}</h3>
                <div class="flex gap-2 mt-1">${(c.tags || []).map(t => `<span class="tag">${escapeHtml(t)}</span>`).join('')}</div>
            </div>
            <div class="text-right">
                <p class="text-2xl font-bold ${c.composite >= 3.5 ? 'text-green-400' : c.composite >= 2.5 ? 'text-yellow-400' : 'text-red-400'}">${fmtScore(c.composite)}${deltaHtml}</p>
                <p class="text-sm text-dark-400">composite score</p>
            </div>
        </div>

        <div class="drill-section">
            <h4 class="drill-section-title">Current Run (${escapeHtml(run.release_tag || run.id)})</h4>
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
                <div class="drill-stat"><p class="stat-label">Result</p><p>${passChip(c.pass)}</p></div>
                <div class="drill-stat"><p class="stat-label">Cost</p><p class="stat-val">${fmtCost(c.cost_usd)}</p></div>
                <div class="drill-stat"><p class="stat-label">Run 1</p><p class="stat-val">${fmtScore(c.run1)}</p></div>
                <div class="drill-stat"><p class="stat-label">Run 2</p><p class="stat-val">${fmtScore(c.run2)}</p></div>
            </div>
            <p class="text-xs text-dark-500 mt-2">Variance: ${variance(c)}</p>
        </div>

        ${baseSection}

        ${base && base.pass && !c.pass ? `
        <div class="p-4 bg-red-950 border border-red-800 rounded-lg">
            <p class="text-red-400 font-semibold text-sm">REGRESSION DETECTED</p>
            <p class="text-dark-300 text-sm mt-1">This case passed in the baseline but failed in the current run. Score dropped by ${Math.abs(delta).toFixed(2)} points.</p>
        </div>` : ''}

        ${base && !base.pass && c.pass ? `
        <div class="p-4 bg-green-950 border border-green-800 rounded-lg">
            <p class="text-green-400 font-semibold text-sm">IMPROVEMENT</p>
            <p class="text-dark-300 text-sm mt-1">This case failed in the baseline but passed in the current run. Score improved by ${Math.abs(delta).toFixed(2)} points.</p>
        </div>` : ''}
    `;

    modal.classList.remove('hidden');
}

function closeDrillDown() {
    document.getElementById('drillDownModal').classList.add('hidden');
}

// Close on backdrop click
document.getElementById('drillDownModal').addEventListener('click', function(e) {
    if (e.target === this) closeDrillDown();
});

// ── CSV Export ────────────────────────────────────────────────────────────────

function exportCSV() {
    const run = state.currentRun;
    if (!run || !run.cases) { alert('No run data to export.'); return; }

    const header = ['case_id', 'composite', 'pass', 'cost_usd', 'run1', 'run2', 'variance', 'tags'];
    const rows = run.cases.map(c => [
        c.eval_case_id,
        c.composite,
        c.pass ? 'true' : 'false',
        c.cost_usd,
        c.run1 ?? '',
        c.run2 ?? '',
        variance(c),
        (c.tags || []).join(';'),
    ]);

    const csv = [header, ...rows].map(r => r.map(v => `"${String(v).replace(/"/g, '""')}"`).join(',')).join('\n');
    const blob = new Blob([csv], { type: 'text/csv' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `bench-${run.release_tag || run.id}-${new Date().toISOString().slice(0, 10)}.csv`;
    a.click();
    URL.revokeObjectURL(url);
}

// ── Main load ─────────────────────────────────────────────────────────────────

async function loadCurrentRun() {
    if (!state.currentRunId) return;
    state.currentRun = await loadRunDetail(state.currentRunId);
    renderOverview(state.currentRun);
    renderCases(state.currentRun);
    if (state.activeTab === 'dimension') renderDimChart();
    if (state.activeTab === 'compare') renderCompare();
}

async function loadCompareRun() {
    if (!state.compareRunId) { state.compareRun = null; renderCompare(); return; }
    state.compareRun = await loadRunDetail(state.compareRunId);
    if (state.activeTab === 'compare') renderCompare();
}

async function onRunChange() {
    state.currentRunId = document.getElementById('runSelector').value;
    await loadCurrentRun();
}

async function onCompareChange() {
    state.compareRunId = document.getElementById('compareSelector').value || null;
    await loadCompareRun();
}

async function init() {
    state.runs = await loadRuns();
    renderMockNotice();
    populateRunSelectors();

    if (state.currentRunId) {
        const [curr, comp] = await Promise.all([
            loadRunDetail(state.currentRunId),
            state.compareRunId ? loadRunDetail(state.compareRunId) : Promise.resolve(null),
        ]);
        state.currentRun = curr;
        state.compareRun = comp;
    }

    renderOverview(state.currentRun);
    renderCases(state.currentRun);
    activateTab('overview');
}

// Bootstrap after DOM ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
} else {
    init();
}
