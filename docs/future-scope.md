# rust-brain Future Scope

Roadmap and potential enhancements for the rust-brain code intelligence platform.

---

## 1. Multi-Repository Support

### Current State
Single repository ingestion with workspace isolation for per-repo analysis. The workspace system (see `docs/workspace-volumes.md`) provides Docker volume isolation and Postgres schema isolation per workspace.

### Future Vision

**Cross-Repo Analysis:**
- Ingest multiple repositories simultaneously
- Cross-reference dependencies and usages across repos
- Understand how changes in one repo affect downstream consumers

**Implementation:**
```
┌─────────────────────────────────────────────────────────┐
│                    Multi-Repo Manager                    │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐    │
│  │ Repo A  │  │ Repo B  │  │ Repo C  │  │ Repo D  │    │
│  │(serde)  │  │(tokio)  │  │(my_app) │  │(lib)    │    │
│  └────┬────┘  └────┬────┘  └────┬────┘  └────┬────┘    │
│       │            │            │            │          │
│       └────────────┴────────────┴────────────┘          │
│                          │                               │
│                    ┌─────▼─────┐                        │
│                    │ Unified   │                        │
│                    │ Knowledge │                        │
│                    │ Graph     │                        │
│                    └───────────┘                        │
└─────────────────────────────────────────────────────────┘
```

**Features:**
- Workspace configuration file for repo groups
- Dependency-aware ingestion order
- Cross-repo call graph traversal
- Impact analysis across repository boundaries
- Automatic dependency version tracking

**CLI:**
```bash
# Add repo to workspace
rust-brain repo add --name serde --path /path/to/serde

# Ingest all repos
rust-brain ingest --all

# Query across repos
rust-brain search "JSON serialization" --repos serde,my_app
```

---

## 2. IDE Integrations

### Current State
OpenCode IDE integration is built and running (see `docs/opencode-integration.md`). MCP server supports both stdio and SSE transports for Claude Code, Claude Desktop, Cline, and OpenCode.

### Language Server Protocol (LSP)

Build a full LSP server for rust-brain features:

**Features:**
- Go to definition (enhanced with semantic understanding)
- Find all references (including transitive)
- Call hierarchy view
- Type usage exploration
- Semantic code search via command palette

**Architecture:**
```
┌─────────────┐     LSP      ┌─────────────┐
│    VS Code  │◄────────────►│  rust-brain │
│   Extension │              │ LSP Server  │
└─────────────┘              └──────┬──────┘
                                    │
                             ┌──────▼──────┐
                             │  Tool API   │
                             │  (port 8088)│
                             └──────┬──────┘
                                    │
                    ┌───────────────┼───────────────┐
                    │               │               │
              ┌─────▼─────┐  ┌──────▼──────┐  ┌─────▼─────┐
              │  Postgres │  │    Neo4j    │  │  Qdrant   │
              └───────────┘  └─────────────┘  └───────────┘
```

### JetBrains Plugin

IntelliJ/CLion/RustRover integration:
- Right-click context menu for rust-brain queries
- Tool window for semantic search
- Inline documentation from knowledge graph

### Vim/Neovim

Lua plugin for Neovim:
- Telescope integration for semantic search
- LSP client configuration
- Floating window for query results

---

## 3. Real-Time Indexing

### Current State
Batch ingestion pipeline with manual triggering. No file watching or incremental updates. Ingestion takes 30+ minutes for large codebases (see `docs/INGESTION_PERFORMANCE.md` for baselines).

### Future Vision

### File Watcher Integration

Monitor file system changes and update the knowledge graph incrementally.

**Architecture:**
```
┌─────────────────────────────────────────────────────────┐
│                    File Watcher                          │
│  ┌───────────┐  ┌───────────┐  ┌───────────┐          │
│  │  notify   │  │  inotify  │  │  FSEvents │          │
│  │ (Linux)   │  │ (Linux)   │  │  (macOS)  │          │
│  └─────┬─────┘  └─────┬─────┘  └─────┬─────┘          │
│        └──────────────┼──────────────┘                 │
│                       ▼                                 │
│              ┌─────────────────┐                       │
│              │  Change Queue   │                       │
│              │  (debounced)    │                       │
│              └────────┬────────┘                       │
│                       ▼                                 │
│              ┌─────────────────┐                       │
│              │ Incremental    │                       │
│              │  Update Engine  │                       │
│              └────────┬────────┘                       │
│                       ▼                                 │
│        ┌──────────────┼──────────────┐                │
│        ▼              ▼              ▼                │
│   ┌─────────┐   ┌──────────┐   ┌─────────┐           │
│   │Postgres │   │  Neo4j   │   │ Qdrant  │           │
│   │ Update  │   │ Update   │   │ Update  │           │
│   └─────────┘   └──────────┘   └─────────┘           │
└─────────────────────────────────────────────────────────┘
```

**Features:**
- Debounced file change handling (500ms default)
- Incremental parsing (only changed files)
- Smart re-indexing (detect API changes vs. implementation)
- Version control integration (only index committed files)
- Background processing queue

**Performance Targets:**
- File change → graph update: <2 seconds
- Full re-index of 100K LOC: <5 minutes
- Incremental update: <500ms per file

---

## 4. Performance Optimizations

### Current State
- Embedding: qwen3-embedding:4b (2560-dim), ~50-100 items/min CPU, ~500+/min GPU
- Neo4j: batch writes with configurable batch size (default 1000)
- Qdrant: HNSW index with 2560-dim vectors
- No caching layer (all queries hit databases directly)

### Caching Layer

**Multi-level Cache:**
```
┌─────────────────────────────────────────────────────────┐
│                    Cache Hierarchy                       │
│                                                          │
│  L1: In-Memory LRU Cache                                │
│  ├── Hot queries (last 5 min)                           │
│  ├── Frequently accessed FQNs                           │
│  └── Size: 100MB, TTL: 5 minutes                        │
│                                                          │
│  L2: Redis Cache                                        │
│  ├── Query results                                      │
│  ├── Embedding cache                                    │
│  └── Size: 1GB, TTL: 1 hour                             │
│                                                          │
│  L3: Pre-computed Views                                 │
│  ├── Call graph snapshots                               │
│  ├── Trait impl indexes                                 │
│  └── Refresh: 15 minutes                                │
└─────────────────────────────────────────────────────────┘
```

### Query Optimization

**Neo4j Optimizations:**
- Composite indexes on frequently queried property combinations
- Query plan caching
- Stored procedures for common traversals

**Qdrant Optimizations:**
- Quantization for reduced memory (Scalar, Product)
- HNSW parameter tuning for recall/speed tradeoff
- Multi-tenancy support for multi-repo

### Parallel Processing

- Parallel embedding generation (batch processing)
- Concurrent graph writes (Neo4j transactions)
- Async query execution with result streaming

---

## 5. Authentication & Authorization

### Authentication Methods

**API Keys:**
```bash
# Generate API key
rust-brain auth create-key --name "ci-bot" --expires 365d

# Use API key
curl -H "X-API-Key: rb_live_xxx" http://localhost:8088/tools/search_semantic
```

**OAuth2/OIDC:**
- Integration with enterprise identity providers
- Support for Keycloak, Auth0, Okta
- JWT token validation

**mTLS:**
- Client certificate authentication
- Certificate rotation support

### Authorization Model

**Role-Based Access Control (RBAC):**
```
┌─────────────────────────────────────────────────────────┐
│                    RBAC Model                           │
│                                                          │
│  Roles:                                                  │
│  ├── admin     → Full access, manage users             │
│  ├── developer → Read access, semantic search          │
│  ├── analyst   → Read-only, limited queries            │
│  └── service   → API access for CI/CD integration      │
│                                                          │
│  Permissions:                                            │
│  ├── search:semantic    → POST /tools/search_semantic  │
│  ├── graph:read         → POST /tools/query_graph      │
│  ├── graph:write        → (future) modify graph        │
│  ├── admin:users        → Manage API keys              │
│  └── admin:config       → System configuration         │
└─────────────────────────────────────────────────────────┘
```

---

## 6. Web UI

### Current State
The Playground UI is built and running at http://localhost:8088/playground:
- **Dashboard** (`index.html`): Real-time service health, ingestion stats, quick actions
- **Query Playground** (`playground.html`): 7 query types with JSON/table view toggle
- **Audit Trail** (`audit.html`): Known issues and system audit information
- **Gap Analysis** (`gaps.html`): Feature completeness tracking
- **Benchmarker** (`benchmarker.html`): Validation run management
- **Editor Playground**: Workspace creation, AI agent execution, diff review, and commit

Built with React 18 + Vite + Tailwind CSS. See `docs/playground.md` and `docs/playground-design.md` for details.

### Future Enhancements

**Potential enhancements:**
- **Code Explorer:** Browse code with semantic understanding (beyond current query playground)
- **Impact Analysis:** Visualize change impact across the call graph
- **Documentation Generator:** Auto-generate docs from graph

**Technology Stack (current):**
```
┌─────────────────────────────────────────────────────────┐
│                    Web UI Stack                          │
│                                                          │
│  Frontend (current):                                     │
│  ├── Framework: React 18 + Vite                         │
│  ├── Styling: Tailwind CSS                              │
│  └── Serving: Static files via Axum                     │
│                                                          │
│  Future enhancements:                                    │
│  ├── Graph Viz: D3.js, Cytoscape.js, or react-flow     │
│  ├── Code Display: Monaco Editor or CodeMirror         │
│  └── Real-time: WebSocket for live updates              │
└─────────────────────────────────────────────────────────┘
```

### UI Mockups

**Search Page:**
```
┌─────────────────────────────────────────────────────────┐
│  🔍 [function that parses JSON____________] [Search]    │
├─────────────────────────────────────────────────────────┤
│  Filters: [All Types ▼] [All Crates ▼] [Any Vis ▼]     │
├─────────────────────────────────────────────────────────┤
│                                                          │
│  ┌──────────────────────────────────────────────────┐  │
│  │ serde_json::from_str              function  92%  │  │
│  │ Deserialize an instance of type T from JSON...   │  │
│  │ src/read.rs:45                                   │  │
│  └──────────────────────────────────────────────────┘  │
│                                                          │
│  ┌──────────────────────────────────────────────────┐  │
│  │ serde_json::from_slice            function  87%  │  │
│  │ Deserialize from bytes instead of string...      │  │
│  │ src/read.rs:78                                   │  │
│  └──────────────────────────────────────────────────┘  │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

**Call Graph View:**
```
┌─────────────────────────────────────────────────────────┐
│  Call Graph: serde_json::from_str                       │
├─────────────────────────────────────────────────────────┤
│                                                          │
│          ┌──────────────┐                               │
│          │    main      │                               │
│          └──────┬───────┘                               │
│                 │                                        │
│          ┌──────▼───────┐                               │
│          │ load_config  │                               │
│          └──────┬───────┘                               │
│                 │                                        │
│          ┌──────▼───────┐                               │
│          │  from_str ★  │  ← Target                    │
│          └──────┬───────┘                               │
│                 │                                        │
│     ┌───────────┼───────────┐                          │
│     ▼           ▼           ▼                          │
│  ┌──────┐  ┌─────────┐  ┌─────────┐                    │
│  │Read  │  │Deserialize│  │Result  │                    │
│  │::new │  │::deserialize│ │::Ok   │                    │
│  └──────┘  └─────────┘  └─────────┘                    │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

---

## 7. Additional Future Features

### Code Metrics

- Cyclomatic complexity per function
- Code coverage integration
- Technical debt indicators
- Dependency health scoring

### Smart Suggestions

- "Similar code exists in..." prompts
- Potential refactoring opportunities
- Unused code detection
- API contract violations

### Git Integration

- Blame-aware queries (who wrote what)
- Change history tracking
- PR impact analysis
- Merge conflict prediction

### Export Capabilities

- Generate PlantUmer/Mermaid diagrams
- Export to GraphML/GEXF
- Generate documentation sites
- Create architecture decision records

### CI/CD Integration

- Pre-commit hooks for impact analysis
- GitHub Actions integration
- GitLab CI integration
- Jenkins plugin

---

## Implementation Priority

| Priority | Feature | Effort | Impact | Status |
|----------|---------|--------|--------|--------|
| P1 | Multi-repo support | High | High | Workspace isolation done; cross-repo queries pending |
| P1 | Caching layer | Medium | High | Not started |
| P1 | Incremental ingestion | High | High | Planned for v0.4.0 |
| P2 | IDE integrations | High | High | OpenCode + MCP done; LSP/JetBrains pending |
| P2 | Real-time indexing | Medium | Medium | Not started |
| P2 | Authentication | Medium | Medium | Planned for v0.4.0 |
| P3 | Web UI enhancements | High | Medium | Playground exists; graph viz, code explorer pending |
| P3 | Performance optimizations | Medium | Medium | qwen3-embedding upgrade done; caching pending |

---

## Contributing

To contribute to any of these features:

1. Open an issue discussing the approach
2. Create a feature branch
3. Submit a PR with tests and documentation
4. Ensure CI passes

See [CONTRIBUTING.md](./CONTRIBUTING.md) for details.
