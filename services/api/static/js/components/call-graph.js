/**
 * CallGraphPanel - Interactive D3 force-directed call graph visualization
 *
 * Listens: state:functionDetail
 * Emits:   graph:node_selected
 */
import { bus } from '../lib/event-bus.js';
import { state } from '../lib/state.js';
import { apiClient } from '../lib/api-client.js';

const d3Import = import('https://cdn.jsdelivr.net/npm/d3@7/+esm');

const MAX_NODES = 200;
const COLORS = { current: '#3b82f6', caller: '#22c55e', callee: '#f97316' };
const NODE_RADIUS = 8;

function truncate(name, max = 20) {
    const short = name.includes('::') ? name.split('::').pop() : name;
    return short.length > max ? short.slice(0, max - 1) + '\u2026' : short;
}

function transformApiData(fqn, data) {
    const nodesMap = new Map();
    const links = [];

    nodesMap.set(fqn, { id: fqn, role: 'current' });

    if (data.callers) {
        for (const c of data.callers) {
            const id = c.fqn || c.name || c;
            if (!nodesMap.has(id)) nodesMap.set(id, { id, role: 'caller' });
            links.push({ source: id, target: fqn });
        }
    }

    if (data.callees) {
        for (const c of data.callees) {
            const id = c.fqn || c.name || c;
            if (!nodesMap.has(id)) nodesMap.set(id, { id, role: 'callee' });
            links.push({ source: fqn, target: id });
        }
    }

    const nodes = [...nodesMap.values()].slice(0, MAX_NODES);
    const nodeIds = new Set(nodes.map(n => n.id));
    const filteredLinks = links.filter(l => nodeIds.has(l.source) && nodeIds.has(l.target));

    return { nodes, links: filteredLinks };
}

class CallGraphPanel {
    constructor(container) {
        this._container = container;
        this._d3 = null;
        this._simulation = null;
        this._graphData = { nodes: [], links: [] };
        this._tooltip = null;
        this._svg = null;
        this._g = null;

        this._buildDOM();
        this._bindEvents();
    }

    _buildDOM() {
        this._container.innerHTML = `
            <div class="callgraph-controls" style="display:flex;gap:8px;align-items:center;padding:8px 12px;border-bottom:1px solid var(--border,#333);">
                <input id="cg-fqn" type="text" placeholder="Fully qualified name\u2026"
                       style="flex:1;padding:6px 10px;background:var(--bg-input,#1e1e2e);color:var(--fg,#cdd6f4);border:1px solid var(--border,#45475a);border-radius:4px;font-family:inherit;font-size:13px;" />
                <label style="display:flex;align-items:center;gap:4px;font-size:12px;color:var(--fg-muted,#a6adc8);">
                    Depth
                    <input id="cg-depth" type="range" min="1" max="10" value="2" style="width:80px;" />
                    <span id="cg-depth-val" style="min-width:16px;text-align:center;">2</span>
                </label>
                <button id="cg-load" style="padding:6px 14px;background:var(--accent,#89b4fa);color:#1e1e2e;border:none;border-radius:4px;cursor:pointer;font-weight:600;font-size:13px;">Load</button>
            </div>
            <div class="callgraph-canvas" style="position:relative;flex:1;overflow:hidden;">
                <svg id="cg-svg" width="100%" height="100%" style="display:block;"></svg>
                <div id="cg-tooltip" style="position:absolute;display:none;padding:6px 10px;background:var(--bg-surface,#313244);color:var(--fg,#cdd6f4);border:1px solid var(--border,#45475a);border-radius:4px;font-size:12px;pointer-events:none;white-space:nowrap;z-index:10;"></div>
                <div class="callgraph-legend" style="position:absolute;bottom:12px;left:12px;display:flex;gap:12px;font-size:11px;color:var(--fg-muted,#a6adc8);background:var(--bg-surface,#313244cc);padding:4px 10px;border-radius:4px;">
                    <span><svg width="10" height="10"><circle cx="5" cy="5" r="4" fill="${COLORS.current}"/></svg> Current</span>
                    <span><svg width="10" height="10"><circle cx="5" cy="5" r="4" fill="${COLORS.caller}"/></svg> Caller</span>
                    <span><svg width="10" height="10"><circle cx="5" cy="5" r="4" fill="${COLORS.callee}"/></svg> Callee</span>
                </div>
            </div>`;

        this._container.style.display = 'flex';
        this._container.style.flexDirection = 'column';
        this._container.style.height = '100%';

        this._fqnInput = this._container.querySelector('#cg-fqn');
        this._depthSlider = this._container.querySelector('#cg-depth');
        this._depthVal = this._container.querySelector('#cg-depth-val');
        this._loadBtn = this._container.querySelector('#cg-load');
        this._svg = this._container.querySelector('#cg-svg');
        this._tooltip = this._container.querySelector('#cg-tooltip');
    }

    _bindEvents() {
        this._depthSlider.addEventListener('input', () => {
            this._depthVal.textContent = this._depthSlider.value;
        });

        this._loadBtn.addEventListener('click', () => this._load());

        this._fqnInput.addEventListener('keydown', e => {
            if (e.key === 'Enter') this._load();
        });

        bus.on('state:functionDetail', ({ next }) => {
            if (!next) return;
            const fqn = next.fqn || next.name || '';
            if (fqn) {
                this._fqnInput.value = fqn;
                this._load();
            }
        });
    }

    async _load() {
        const fqn = this._fqnInput.value.trim();
        if (!fqn) return;

        const depth = parseInt(this._depthSlider.value, 10);
        this._loadBtn.disabled = true;
        this._loadBtn.textContent = '\u2026';

        try {
            const data = await apiClient.getCallers(fqn, depth);
            const graph = transformApiData(fqn, data);
            this._graphData = graph;
            state.setCallGraph(graph);
            await this._render();
        } catch (err) {
            console.error('[CallGraph] Load failed:', err);
        } finally {
            this._loadBtn.disabled = false;
            this._loadBtn.textContent = 'Load';
        }
    }

    async _ensureD3() {
        if (!this._d3) {
            this._d3 = await d3Import;
        }
        return this._d3;
    }

    async _render() {
        const d3 = await this._ensureD3();
        const { nodes, links } = this._graphData;
        if (nodes.length === 0) return;

        const svgEl = this._svg;
        const rect = svgEl.getBoundingClientRect();
        const width = rect.width || 800;
        const height = rect.height || 500;

        d3.select(svgEl).selectAll('*').remove();

        // Arrow marker
        const defs = d3.select(svgEl).append('defs');
        defs.append('marker')
            .attr('id', 'cg-arrow')
            .attr('viewBox', '0 0 10 6')
            .attr('refX', 18)
            .attr('refY', 3)
            .attr('markerWidth', 8)
            .attr('markerHeight', 6)
            .attr('orient', 'auto')
            .append('path')
            .attr('d', 'M0,0 L10,3 L0,6 Z')
            .attr('fill', '#585b70');

        const svg = d3.select(svgEl);
        this._g = svg.append('g');
        const g = this._g;

        // Zoom + pan
        const zoom = d3.zoom()
            .scaleExtent([0.2, 5])
            .on('zoom', (event) => g.attr('transform', event.transform));
        svg.call(zoom);

        // Links
        const link = g.append('g')
            .selectAll('line')
            .data(links)
            .join('line')
            .attr('stroke', '#585b70')
            .attr('stroke-width', 1.2)
            .attr('marker-end', 'url(#cg-arrow)');

        // Nodes
        const node = g.append('g')
            .selectAll('circle')
            .data(nodes)
            .join('circle')
            .attr('r', NODE_RADIUS)
            .attr('fill', d => COLORS[d.role] || COLORS.current)
            .attr('stroke', '#1e1e2e')
            .attr('stroke-width', 1.5)
            .attr('cursor', 'pointer');

        // Labels
        const label = g.append('g')
            .selectAll('text')
            .data(nodes)
            .join('text')
            .text(d => truncate(d.id))
            .attr('font-size', 10)
            .attr('fill', 'var(--fg-muted, #a6adc8)')
            .attr('dx', NODE_RADIUS + 4)
            .attr('dy', 3)
            .attr('pointer-events', 'none');

        // Tooltip
        const tooltip = this._tooltip;

        node.on('mouseenter', (event, d) => {
            tooltip.textContent = d.id;
            tooltip.style.display = 'block';
        })
        .on('mousemove', (event) => {
            const bounds = svgEl.getBoundingClientRect();
            tooltip.style.left = `${event.clientX - bounds.left + 12}px`;
            tooltip.style.top = `${event.clientY - bounds.top - 8}px`;
        })
        .on('mouseleave', () => {
            tooltip.style.display = 'none';
        });

        // Click → emit node selected
        node.on('click', (_event, d) => {
            bus.emit('graph:node_selected', { fqn: d.id, role: d.role });
        });

        // Double-click → expand callees
        node.on('dblclick', async (_event, d) => {
            try {
                const data = await apiClient.getCallers(d.id, 1);
                this._expand(d.id, data);
            } catch (err) {
                console.error('[CallGraph] Expand failed:', err);
            }
        });

        // Drag
        node.call(d3.drag()
            .on('start', (event, d) => {
                if (!event.active) this._simulation.alphaTarget(0.3).restart();
                d.fx = d.x;
                d.fy = d.y;
            })
            .on('drag', (_event, d) => {
                d.fx = _event.x;
                d.fy = _event.y;
            })
            .on('end', (event, d) => {
                if (!event.active) this._simulation.alphaTarget(0);
                d.fx = null;
                d.fy = null;
            })
        );

        // Simulation
        if (this._simulation) this._simulation.stop();

        this._simulation = d3.forceSimulation(nodes)
            .force('link', d3.forceLink(links).id(d => d.id).distance(80))
            .force('charge', d3.forceManyBody().strength(-200))
            .force('center', d3.forceCenter(width / 2, height / 2))
            .force('collide', d3.forceCollide(NODE_RADIUS * 2.5))
            .on('tick', () => {
                link
                    .attr('x1', d => d.source.x)
                    .attr('y1', d => d.source.y)
                    .attr('x2', d => d.target.x)
                    .attr('y2', d => d.target.y);

                node
                    .attr('cx', d => d.x)
                    .attr('cy', d => d.y);

                label
                    .attr('x', d => d.x)
                    .attr('y', d => d.y);
            });
    }

    _expand(fqn, data) {
        const existing = new Set(this._graphData.nodes.map(n => n.id));
        const newNodes = [];
        const newLinks = [];

        if (data.callees) {
            for (const c of data.callees) {
                const id = c.fqn || c.name || c;
                if (!existing.has(id) && this._graphData.nodes.length + newNodes.length < MAX_NODES) {
                    newNodes.push({ id, role: 'callee' });
                    existing.add(id);
                }
                newLinks.push({ source: fqn, target: id });
            }
        }

        if (data.callers) {
            for (const c of data.callers) {
                const id = c.fqn || c.name || c;
                if (!existing.has(id) && this._graphData.nodes.length + newNodes.length < MAX_NODES) {
                    newNodes.push({ id, role: 'caller' });
                    existing.add(id);
                }
                newLinks.push({ source: id, target: fqn });
            }
        }

        if (newNodes.length === 0 && newLinks.length === 0) return;

        this._graphData = {
            nodes: [...this._graphData.nodes, ...newNodes],
            links: [...this._graphData.links, ...newLinks],
        };

        this._render();
    }

    destroy() {
        if (this._simulation) {
            this._simulation.stop();
            this._simulation = null;
        }
    }
}

let instance = null;

export function init(paneEl) {
    if (!instance) {
        instance = new CallGraphPanel(paneEl);
    }
    return instance;
}

export default CallGraphPanel;
