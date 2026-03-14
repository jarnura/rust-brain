# rust-brain Future Scope

Roadmap and potential enhancements for the rust-brain code intelligence platform.

---

## 1. Multi-Repository Support

### Current State
Single repository ingestion with manual configuration.

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

### Dashboard Application

Modern web interface for rust-brain:

**Features:**
- **Code Explorer:** Browse code with semantic understanding
- **Search Interface:** Natural language queries with filters
- **Call Graph Visualization:** Interactive call hierarchy
- **Impact Analysis:** Visualize change impact
- **Documentation Generator:** Auto-generate docs from graph

**Technology Stack:**
```
┌─────────────────────────────────────────────────────────┐
│                    Web UI Stack                          │
│                                                          │
│  Frontend:                                               │
│  ├── Framework: React/Next.js or Svelte/SvelteKit      │
│  ├── Graph Viz: D3.js, Cytoscape.js, or react-flow     │
│  ├── Code Display: Monaco Editor or CodeMirror         │
│  └── Styling: Tailwind CSS                              │
│                                                          │
│  Backend (optional separate API):                       │
│  ├── Framework: Axum or Actix-web (Rust)               │
│  ├── WebSocket: Real-time updates                       │
│  └── Static: Served from /ui or CDN                    │
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

| Priority | Feature | Effort | Impact |
|----------|---------|--------|--------|
| P1 | Multi-repo support | High | High |
| P1 | Caching layer | Medium | High |
| P2 | IDE integrations | High | High |
| P2 | Real-time indexing | Medium | Medium |
| P2 | Authentication | Medium | Medium |
| P3 | Web UI | High | Medium |
| P3 | Performance optimizations | Medium | Medium |

---

## Contributing

To contribute to any of these features:

1. Open an issue discussing the approach
2. Create a feature branch
3. Submit a PR with tests and documentation
4. Ensure CI passes

See [CONTRIBUTING.md](./CONTRIBUTING.md) for details.
