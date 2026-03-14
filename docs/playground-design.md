# rust-brain Playground UI/UX Architecture

**Version:** 1.0  
**Date:** 2026-03-14  
**Status:** Design Document

---

## Overview

The rust-brain Playground is an interactive web interface for exploring the code intelligence platform. It provides real-time visibility into system operations, feature gap analysis, and a query interface for testing the code intelligence capabilities.

### Goals

1. **Interactive Exploration** - Allow users to query and explore the code knowledge graph
2. **Real-Time Audit Trail** - Show what's happening in the system as it processes
3. **Gap Analysis Display** - Clearly show which features work and which are missing
4. **Task Tracking** - Monitor ongoing ingestion and indexing operations

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          BROWSER                                          │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                    Playground UI (HTMX + Tailwind)              │    │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────┐ │    │
│  │  │Dashboard │ │Playground│ │AuditTrail│ │GapAnalysis│ │Settings│ │    │
│  │  └────┬─────┘ └────┬─────┘ └────┬─────┘ └────┬─────┘ └───┬───┘ │    │
│  │       │            │            │            │           │      │    │
│  │       └────────────┴────────────┴────────────┴───────────┘      │    │
│  │                              │                                   │    │
│  │              ┌───────────────┼───────────────┐                  │    │
│  │              ▼               ▼               ▼                  │    │
│  │        HTMX AJAX      WebSocket Conn    HTMX AJAX               │    │
│  └──────────┬─────────────────┬─────────────────┬──────────────────┘    │
└─────────────┼─────────────────┼─────────────────┼───────────────────────┘
              │                 │                 │
              ▼                 ▼                 ▼
┌─────────────────────────────────────────────────────────────────────────┐
│                      PLAYGROUND SERVER (Rust)                            │
│  ┌─────────────────────────────────────────────────────────────────┐    │
│  │                     Axum Router (:8081)                          │    │
│  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐   │    │
│  │  │ REST API     │  │ WebSocket    │  │ Static File Server   │   │    │
│  │  │ /playground/*│  │ /playground/ws│  │ /ui/* → HTML/CSS/JS  │   │    │
│  │  └──────┬───────┘  └──────┬───────┘  └──────────────────────┘   │    │
│  │         │                 │                                      │    │
│  │  ┌──────▼─────────────────▼──────────────────────────────────┐   │    │
│  │  │              Playground Service Layer                      │   │    │
│  │  │  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌─────────┐ │   │    │
│  │  │  │StatusService│ │AuditService│ │GapService  │ │TaskService│   │    │
│  │  │  └─────┬──────┘ └─────┬──────┘ └─────┬──────┘ └────┬────┘ │   │    │
│  │  └────────┼──────────────┼──────────────┼─────────────┼──────┘   │    │
│  └───────────┼──────────────┼──────────────┼─────────────┼──────────┘    │
│              │              │              │             │               │
│              ▼              ▼              ▼             ▼               │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    Shared State (Arc<RwLock>)                      │  │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌─────────────┐  │  │
│  │  │SystemStatus │ │ AuditLog    │ │ GapRegistry │ │ TaskTracker │  │  │
│  │  │(Arc<Atomic>) │ │ (VecDeque)  │ │ (HashMap)   │ │ (HashMap)   │  │  │
│  │  └─────────────┘ └─────────────┘ └─────────────┘ └─────────────┘  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│              │              │              │             │               │
│              └──────────────┴──────────────┴─────────────┘               │
│                                    │                                     │
│                                    ▼                                     │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                 External Service Clients                            │  │
│  │  ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────┐ ┌─────────────┐  │  │
│  │  │Postgres │ │ Neo4j   │ │ Qdrant  │ │ Ollama  │ │ Tool API    │  │  │
│  │  │Client   │ │ Client  │ │ Client  │ │ Client  │ │ (:8080)     │  │  │
│  │  └─────────┘ └─────────┘ └─────────┘ └─────────┘ └─────────────┘  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Page Structure

### 1. Dashboard (`/playground`)

System overview with real-time status indicators and key metrics.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🧠 rust-brain Playground                            [Connected ●]      │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌─────────────────────┐  ┌─────────────────────┐  ┌─────────────────┐ │
│  │    System Health    │  │   Ingestion Stats   │  │  Active Tasks   │ │
│  │  ┌───────────────┐  │  │  ┌───────────────┐  │  │                 │ │
│  │  │ ● Postgres    │  │  │  │ Files: 247    │  │  │  ▶ Ingesting   │ │
│  │  │ ● Neo4j       │  │  │  │ Functions: 1.2K│  │  │    serde       │ │
│  │  │ ● Qdrant      │  │  │  │ Structs: 312  │  │  │    47% ████░░ │ │
│  │  │ ● Ollama      │  │  │  │ Embeddings: 1K│  │  │                 │ │
│  │  │ ○ API (8080)  │  │  │  └───────────────┘  │  │  ✓ Completed   │ │
│  │  └───────────────┘  │  │                     │  │    tokio       │ │
│  │  All systems up     │  │  Last run: 2h ago   │  │    100% ██████│ │
│  └─────────────────────┘  └─────────────────────┘  └─────────────────┘ │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │                    Recent Activity (Live)                     [▶] │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │  18:45:23  ✓ Indexed src/parser/mod.rs (47 items)               │  │
│  │  18:45:21  → Generating embeddings for 23 functions...           │  │
│  │  18:45:18  ✓ Parsed src/parser/tree_sitter.rs                   │  │
│  │  18:45:15  ⚠ Warning: Failed to resolve type in src/lib.rs:142  │  │
│  │  18:45:12  ✓ Connected to Neo4j (12 constraints, 9 indexes)     │  │
│  │  ─────────────────────────────────────────────────────────────── │  │
│  │  [View Full Audit Trail →]                                       │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────┐  ┌───────────────────────────────┐  │
│  │       Quick Actions           │  │       Feature Status          │  │
│  │  [▶ Start Ingestion]          │  │  ████████████░░░░  75% Ready │  │
│  │  [🔍 Semantic Search]         │  │  12 working | 4 partial | 0 fail│
│  │  [📊 View Graph]              │  │  [View Gap Analysis →]        │  │
│  └───────────────────────────────┘  └───────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Components:**

| Component | Description | Refresh |
|-----------|-------------|---------|
| System Health | Status of all services (Postgres, Neo4j, Qdrant, Ollama, API) | 5s via WebSocket |
| Ingestion Stats | Total files, functions, structs, embeddings | On change via WS |
| Active Tasks | Running/pending ingestion jobs with progress | Real-time via WS |
| Recent Activity | Last 5 log entries with live updates | Real-time via WS |
| Quick Actions | Buttons for common operations | Static |
| Feature Status | Summary of gap analysis | On change |

### 2. Playground (`/playground/query`)

Interactive query interface for testing code intelligence features.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🧠 Playground > Query                            [Connected ●]         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Query Type:  [Semantic Search ▼] [Get Function] [Callers] [Graph]│  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  ┌─────────────────────────────────────────────────────────────┐  │  │
│  │  │  // Natural language query or FQN                          │  │  │
│  │  │  function that parses JSON into a struct                    │  │  │
│  │  │                                                             │  │  │
│  │  │                                                             │  │  │
│  │  └─────────────────────────────────────────────────────────────┘  │  │
│  │                                                                    │  │
│  │  Filters:                                                         │  │
│  │  [✓ Functions] [✓ Structs] [✐ Enums] [✐ Traits] [✐ All Crates ▼]│  │
│  │                                                                    │  │
│  │  [▶ Execute Query]  [⏹ Cancel]  [💾 Save Query]                  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Results (3 found)                              ⏱ 23ms  📊 Score  │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  ┌─────────────────────────────────────────────────────────────┐  │  │
│  │  │  serde_json::from_str                         94% ●●●●●○    │  │  │
│  │  │  ───────────────────────────────────────────────────────────│  │  │
│  │  │  pub fn from_str<'a, T>(s: &'a str) -> Result<T>           │  │  │
│  │  │  where T: Deserialize<'a>                                   │  │  │
│  │  │                                                             │  │  │
│  │  │  Deserialize an instance of type T from a string of JSON.   │  │  │
│  │  │                                                             │  │  │
│  │  │  📁 serde_json/src/de.rs:45-62  👁 [View] [Callers] [Graph]│  │  │
│  │  └─────────────────────────────────────────────────────────────┘  │  │
│  │                                                                    │  │
│  │  ┌─────────────────────────────────────────────────────────────┐  │  │
│  │  │  serde_json::from_slice                       89% ●●●●○○    │  │  │
│  │  │  ───────────────────────────────────────────────────────────│  │  │
│  │  │  pub fn from_slice<'a, T>(slice: &'a [u8]) -> Result<T>    │  │  │
│  │  │                                                             │  │  │
│  │  │  📁 serde_json/src/de.rs:78-92   👁 [View] [Callers] [Graph]│  │  │
│  │  └─────────────────────────────────────────────────────────────┘  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Query History                          [Clear] [Export Results]  │  │
│  │  ─────────────────────────────────────────────────────────────── │  │
│  │  • "function that parses JSON" (3 results)      2 min ago        │  │
│  │  • "serde_json::from_str" (1 result)            5 min ago        │  │
│  │  • "get_callers depth=2" (12 results)           10 min ago       │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Query Types:**

| Type | Endpoint | Description |
|------|----------|-------------|
| Semantic Search | `POST /playground/query/semantic` | Natural language code search |
| Get Function | `POST /playground/query/function` | Get details by FQN |
| Get Callers | `POST /playground/query/callers` | Call hierarchy |
| Get Trait Impls | `POST /playground/query/traits` | Trait implementations |
| Graph Query | `POST /playground/query/graph` | Raw Cypher execution |

**Result Views:**

| View | Description |
|------|-------------|
| Code | Syntax-highlighted source code |
| Graph | Interactive call graph visualization |
| Tree | Hierarchical module/item tree |
| JSON | Raw API response |

### 3. Audit Trail (`/playground/audit`)

Real-time operation log with filtering and search.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🧠 Playground > Audit Trail                       [Connected ●]        │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Filters:                                                         │  │
│  │  [All Levels ▼] [All Sources ▼] [Last Hour ▼]    [🔍 Search...]  │  │
│  │                                                                   │  │
│  │  Levels: [✓ Info] [✓ Warn] [✓ Error] [✐ Debug]                  │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Timeline                                              [▶ Live]  │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  18:45:32.142                                                      │  │
│  │  ├── ✓ INFO  [ingestion] Indexed src/parser/syn_parser.rs         │  │
│  │  │         23 items extracted in 142ms                            │  │
│  │  │                                                                 │  │
│  │  18:45:31.891                                                      │  │
│  │  ├── ⚠ WARN  [typecheck] Failed to resolve type                   │  │
│  │  │         File: src/lib.rs:142                                   │  │
│  │  │         Type: MyGeneric<T>                                     │  │
│  │  │         Reason: Type parameter T has no bounds                 │  │
│  │  │                                                                 │  │
│  │  18:45:30.455                                                      │  │
│  │  ├── ✓ INFO  [embedding] Generated embedding for                  │  │
│  │  │         serde_json::from_str (768 dimensions)                  │  │
│  │  │                                                                 │  │
│  │  18:45:28.123                                                      │  │
│  │  ├── ✗ ERROR [graph] Neo4j connection timeout                     │  │
│  │  │         Retrying in 2s... (attempt 2/3)                        │  │
│  │  │                                                                 │  │
│  │  18:45:25.000                                                      │  │
│  │  └── ✓ INFO  [pipeline] Stage 3/6 complete: typecheck             │  │
│  │           1,247 items processed, 3 warnings                       │  │
│  │                                                                    │  │
│  │  ─────────────────────────────────────────────────────────────── │  │
│  │  [Load More]                                       Showing 5/1,247 │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Statistics                          [Export Logs] [Clear Logs]   │  │
│  │  ───────────────────────────────────────────────────────────────  │  │
│  │  Total entries: 1,247  |  Errors: 3  |  Warnings: 12  |  Rate: 5/s│  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Log Entry Structure:**

```json
{
  "id": "log_abc123",
  "timestamp": "2026-03-14T18:45:32.142Z",
  "level": "info",
  "source": "ingestion",
  "message": "Indexed src/parser/syn_parser.rs",
  "details": {
    "items_extracted": 23,
    "duration_ms": 142
  },
  "trace_id": "trace_xyz789",
  "span_id": "span_def456"
}
```

**Features:**

- Real-time log streaming via WebSocket
- Filter by level, source, time range
- Full-text search across messages
- Expandable detail panels
- Trace ID linking for debugging
- Export to JSON/CSV

### 4. Gap Analysis (`/playground/gaps`)

Feature completeness report showing what works, what's partial, and what's missing.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🧠 Playground > Gap Analysis                     [Connected ●]         │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Overall Status: 75% Complete                                     │  │
│  │  ████████████████████████████████░░░░░░░░░░░░                    │  │
│  │  12 features working | 4 partial | 0 broken | 4 planned          │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Ingestion Pipeline                                        [Test] │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  ✓ cargo expand         Macro expansion           Working         │  │
│  │  ✓ tree-sitter parse    Fast skeleton extraction  Working         │  │
│  │  ✓ syn parse            Deep semantic analysis    Working         │  │
│  │  ⚠ rust-analyzer        Type information          Partial         │  │
│  │    └── Issue: Missing generic type bounds resolution              │  │
│  │  ✓ item extraction      Functions, structs, etc.  Working         │  │
│  │  ✓ graph population     Neo4j relationships       Working         │  │
│  │  ✓ embedding generation Qdrant vectors            Working         │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Query API                                                  [Test]│  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  ✓ POST /tools/search_semantic  Semantic search    Working        │  │
│  │  ✓ GET  /tools/get_function    Function details    Working        │  │
│  │  ✓ GET  /tools/get_callers     Call hierarchy      Working        │  │
│  │  ✓ GET  /tools/get_trait_impls Trait impls         Working        │  │
│  │  ✓ GET  /tools/find_usages     Type usages         Working        │  │
│  │  ✓ GET  /tools/get_module_tree Module hierarchy    Working        │  │
│  │  ✓ POST /tools/query_graph     Raw Cypher          Working        │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Planned Features                                           [+]   │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  ○ Multi-repo support        Cross-repo analysis    Priority: P1  │  │
│  │  ○ Real-time indexing        File watcher           Priority: P2  │  │
│  │  ○ IDE integrations          LSP, VS Code           Priority: P2  │  │
│  │  ○ Authentication            API keys, OAuth        Priority: P2  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Known Issues                                            [Report] │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  INGEST-001  sqlx 0.8 try_get API change            Status: Open  │  │
│  │  INGEST-002  neo4rs 0.8 execute method visibility   Status: Open  │  │
│  │  INGEST-003  uuid crate new_v5 signature            Status: Open  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Feature Status Types:**

| Status | Icon | Description |
|--------|------|-------------|
| Working | ✓ | Fully functional, tested |
| Partial | ⚠ | Works but has known issues |
| Broken | ✗ | Not working, needs fix |
| Planned | ○ | On roadmap, not yet implemented |

**Gap Analysis API:**

```json
{
  "overall_status": {
    "total": 20,
    "working": 12,
    "partial": 4,
    "broken": 0,
    "planned": 4,
    "percentage": 75
  },
  "categories": [
    {
      "name": "Ingestion Pipeline",
      "features": [
        {
          "id": "cargo-expand",
          "name": "cargo expand",
          "description": "Macro expansion",
          "status": "working",
          "last_tested": "2026-03-14T18:00:00Z",
          "issues": []
        },
        {
          "id": "rust-analyzer",
          "name": "rust-analyzer",
          "description": "Type information",
          "status": "partial",
          "last_tested": "2026-03-14T18:00:00Z",
          "issues": ["Missing generic type bounds resolution"]
        }
      ]
    }
  ]
}
```

### 5. Settings (`/playground/settings`)

Configuration options for the playground and connected services.

```
┌─────────────────────────────────────────────────────────────────────────┐
│  🧠 Playground > Settings                          [Connected ●]        │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Connection Settings                                      [Save]  │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  Tool API URL         [http://localhost:8080            ] [Test]  │  │
│  │  Neo4j URL            [bolt://localhost:7687           ] [Test]  │  │
│  │  Qdrant URL           [http://localhost:6333           ] [Test]  │  │
│  │  Ollama URL           [http://localhost:11434          ] [Test]  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Query Defaults                                            [Save] │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  Default result limit [10        ]                                │  │
│  │  Score threshold      [0.5       ] (0.0 - 1.0)                    │  │
│  │  Max call depth       [3         ]                                │  │
│  │  Query timeout (ms)   [30000     ]                                │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Audit Trail Settings                                      [Save] │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  Max log entries      [1000      ]                                │  │
│  │  Log retention (hrs)  [24        ]                                │  │
│  │  [✓] Show debug logs                                             │  │
│  │  [✓] Auto-scroll to new entries                                  │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐  │
│  │  Danger Zone                                                      │  │
│  ├───────────────────────────────────────────────────────────────────┤  │
│  │                                                                    │  │
│  │  [🗑 Clear All Logs]    [🔄 Reset Settings]   [⚠ Reindex All]     │  │
│  │                                                                    │  │
│  └───────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Tech Stack Recommendation

### Option A: HTML + HTMX + Tailwind (Recommended for MVP)

**Why this approach:**
- No build step required
- Fast development iteration
- Easy to embed in existing Rust binary
- Good enough for internal tooling
- Tailwind via CDN for styling

**Stack:**

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          Tech Stack (Option A)                           │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Frontend (served from Rust):                                           │
│  ├── HTML5 with HTMX 1.9+ for interactivity                             │
│  ├── Tailwind CSS 3.4+ via CDN                                          │
│  ├── Alpine.js 3.x for client-side state (optional)                     │
│  ├── Highlight.js for syntax highlighting                               │
│  └── D3.js or Cytoscape.js for graph visualization                      │
│                                                                          │
│  Backend (extends existing API):                                        │
│  ├── Axum (already in use)                                              │
│  ├── Tokio for async runtime                                            │
│  ├── Tower for middleware                                               │
│  └── Existing service clients (Postgres, Neo4j, Qdrant, Ollama)        │
│                                                                          │
│  Real-time:                                                              │
│  ├── WebSocket via axum::extract::ws                                    │
│  ├── Broadcast channel for audit trail                                  │
│  └── Server-Sent Events as fallback                                     │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**File Structure:**

```
services/api/
├── src/
│   ├── main.rs              # Existing entry point
│   ├── playground/
│   │   ├── mod.rs           # Playground module
│   │   ├── routes.rs        # Route definitions
│   │   ├── handlers.rs      # Request handlers
│   │   ├── ws.rs            # WebSocket handling
│   │   ├── state.rs         # Shared state
│   │   └── models.rs        # Request/response types
│   └── ...
├── static/
│   ├── index.html           # Main layout
│   ├── dashboard.html       # Dashboard partial
│   ├── query.html           # Query interface
│   ├── audit.html           # Audit trail
│   ├── gaps.html            # Gap analysis
│   └── settings.html        # Settings page
└── Cargo.toml
```

### Option B: React + Vite (Future Enhancement)

For a more polished UI with better UX:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                          Tech Stack (Option B)                           │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Frontend (separate build):                                             │
│  ├── React 18+ with TypeScript                                          │
│  ├── Vite for fast builds                                               │
│  ├── TanStack Query for data fetching                                   │
│  ├── Zustand for client state                                           │
│  ├── Tailwind CSS for styling                                           │
│  ├── Monaco Editor for code editing                                     │
│  └── React Flow or Cytoscape.js for graphs                              │
│                                                                          │
│  Build output served from Rust or CDN                                   │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Recommendation:** Start with Option A (HTMX) for MVP, migrate to Option B if UX demands grow.

---

## API Endpoints

### REST Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/playground` | Dashboard HTML |
| GET | `/playground/status` | System status JSON |
| GET | `/playground/stats` | Ingestion statistics |
| GET | `/playground/tasks` | Active task list |
| POST | `/playground/tasks` | Start new ingestion task |
| DELETE | `/playground/tasks/:id` | Cancel task |
| GET | `/playground/gaps` | Gap analysis report |
| GET | `/playground/gaps/:category` | Category-specific gaps |
| GET | `/playground/audit` | Audit log entries |
| DELETE | `/playground/audit` | Clear audit log |
| GET | `/playground/settings` | Current settings |
| PUT | `/playground/settings` | Update settings |
| POST | `/playground/query/semantic` | Semantic search query |
| POST | `/playground/query/function` | Get function details |
| POST | `/playground/query/callers` | Get call hierarchy |
| POST | `/playground/query/traits` | Get trait implementations |
| POST | `/playground/query/graph` | Execute Cypher query |

### WebSocket Endpoint

| Endpoint | Description |
|----------|-------------|
| `ws://localhost:8081/playground/ws` | Real-time updates |

**Message Types:**

```json
// Server → Client: Status Update
{
  "type": "status",
  "data": {
    "postgres": "healthy",
    "neo4j": "healthy",
    "qdrant": "healthy",
    "ollama": "healthy",
    "api": "healthy"
  }
}

// Server → Client: Audit Log Entry
{
  "type": "audit",
  "data": {
    "id": "log_abc123",
    "timestamp": "2026-03-14T18:45:32.142Z",
    "level": "info",
    "source": "ingestion",
    "message": "Indexed src/parser/syn_parser.rs",
    "details": { "items": 23 }
  }
}

// Server → Client: Task Update
{
  "type": "task",
  "data": {
    "id": "task_xyz",
    "status": "running",
    "progress": 47,
    "message": "Processing src/parser/mod.rs"
  }
}

// Client → Server: Subscribe to events
{
  "type": "subscribe",
  "channels": ["audit", "tasks", "status"]
}

// Client → Server: Execute query
{
  "type": "query",
  "query_type": "semantic",
  "data": { "query": "parse JSON", "limit": 10 }
}
```

---

## Key Components

### 1. Query Editor

**Requirements:**
- Natural language input for semantic search
- FQN input for targeted queries
- Query type selector (semantic, function, callers, graph)
- Filter controls (item types, crates, visibility)
- Execute/Cancel buttons
- Save query to history

**Implementation (HTMX):**

```html
<div class="query-editor">
  <select name="query_type" hx-get="/playground/query/form" hx-target="#query-form">
    <option value="semantic">Semantic Search</option>
    <option value="function">Get Function</option>
    <option value="callers">Get Callers</option>
    <option value="graph">Graph Query</option>
  </select>
  
  <div id="query-form">
    <!-- Dynamic form loaded via HTMX -->
  </div>
  
  <button hx-post="/playground/query/execute" 
          hx-target="#results"
          hx-indicator="#loading">
    Execute Query
  </button>
</div>
```

### 2. Result Visualizer

**Code View:**
- Syntax highlighting (Highlight.js or Prism)
- Line numbers
- Click-to-copy
- Link to file/line

**Graph View:**
- Interactive node-edge diagram
- Zoom/pan controls
- Click to expand
- Export as SVG/PNG

**Tree View:**
- Collapsible hierarchy
- Item counts
- Search/filter within tree

**Implementation:**

```html
<div class="result-views" hx-tabs>
  <button hx-get="/playground/results/code" hx-target="#result-content">Code</button>
  <button hx-get="/playground/results/graph" hx-target="#result-content">Graph</button>
  <button hx-get="/playground/results/tree" hx-target="#result-content">Tree</button>
  <button hx-get="/playground/results/json" hx-target="#result-content">JSON</button>
  
  <div id="result-content">
    <!-- Dynamic content -->
  </div>
</div>
```

### 3. Status Indicators

**Service Status:**
- Color-coded (green/yellow/red)
- Latency display
- Click for details
- Auto-refresh via WebSocket

**Implementation:**

```html
<div class="status-grid" hx-ext="ws" ws-connect="/playground/ws">
  <div class="status-item" id="status-postgres">
    <span class="status-dot"></span>
    <span class="status-name">Postgres</span>
    <span class="status-latency" ws-message="status.postgres.latency"></span>
  </div>
  <!-- More services -->
</div>
```

### 4. Audit Timeline

**Features:**
- Chronological list
- Color-coded levels (info/warn/error)
- Expandable details
- Filter/search bar
- Auto-scroll option

**Implementation:**

```html
<div class="audit-timeline" hx-ext="ws" ws-connect="/playground/ws">
  <template id="log-entry-template">
    <div class="log-entry">
      <span class="log-time"></span>
      <span class="log-level"></span>
      <span class="log-source"></span>
      <span class="log-message"></span>
      <div class="log-details"></div>
    </div>
  </template>
  
  <div id="audit-entries" ws-message="audit">
    <!-- Entries appended via WebSocket -->
  </div>
</div>
```

### 5. Error/Warning Display

**Inline Errors:**
- Show below relevant component
- Dismissible
- Link to details if available

**Toast Notifications:**
- Slide in from corner
- Auto-dismiss after delay
- Click to expand

**Implementation:**

```html
<div id="toast-container">
  <template id="toast-template">
    <div class="toast">
      <span class="toast-icon"></span>
      <span class="toast-message"></span>
      <button class="toast-dismiss">×</button>
    </div>
  </template>
</div>

<!-- HTMX trigger for errors -->
<div hx-post="/playground/query"
     hx-trigger="error:show-toast">
</div>
```

---

## Data Models

### Playground State

```rust
/// Shared state for the playground
pub struct PlaygroundState {
    /// System status for each service
    pub status: Arc<RwLock<SystemStatus>>,
    
    /// Audit log entries (ring buffer)
    pub audit_log: Arc<RwLock<VecDeque<AuditEntry>>>,
    
    /// Active tasks
    pub tasks: Arc<RwLock<HashMap<String, Task>>>,
    
    /// Gap analysis results
    pub gaps: Arc<RwLock<GapAnalysis>>,
    
    /// Settings
    pub settings: Arc<RwLock<Settings>>,
    
    /// WebSocket broadcaster
    pub broadcaster: Arc<BroadcastSender<Message>>,
}

#[derive(Clone, Serialize)]
pub struct SystemStatus {
    pub postgres: ServiceStatus,
    pub neo4j: ServiceStatus,
    pub qdrant: ServiceStatus,
    pub ollama: ServiceStatus,
    pub api: ServiceStatus,
}

#[derive(Clone, Serialize)]
pub struct ServiceStatus {
    pub healthy: bool,
    pub latency_ms: Option<u64>,
    pub last_check: DateTime<Utc>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
    pub trace_id: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct Task {
    pub id: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub progress: u8,
    pub message: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct GapAnalysis {
    pub overall_status: OverallStatus,
    pub categories: Vec<CategoryGaps>,
    pub last_updated: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct CategoryGaps {
    pub name: String,
    pub features: Vec<FeatureStatus>,
}

#[derive(Clone, Serialize)]
pub struct FeatureStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub status: FeatureState,
    pub issues: Vec<String>,
    pub last_tested: Option<DateTime<Utc>>,
}
```

---

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1)

1. **Playground Router Setup**
   - Add `/playground/*` routes to existing Axum router
   - Set up static file serving for HTML/CSS/JS
   - Configure CORS for local development

2. **WebSocket Infrastructure**
   - Implement WebSocket endpoint
   - Create broadcast channel for real-time updates
   - Add connection management

3. **State Management**
   - Implement `PlaygroundState` struct
   - Add to app state alongside existing resources
   - Initialize with current system status

### Phase 2: Dashboard & Status (Week 1-2)

1. **Dashboard HTML**
   - Create main layout with navigation
   - Build status grid component
   - Add stats cards

2. **Status Service**
   - Implement health check polling
   - Collect ingestion statistics
   - Track active tasks

3. **WebSocket Updates**
   - Push status changes to connected clients
   - Handle reconnect gracefully

### Phase 3: Query Interface (Week 2)

1. **Query Forms**
   - Semantic search form
   - Function lookup form
   - Callers/trait forms
   - Graph query form

2. **Query Execution**
   - Proxy to existing Tool API
   - Handle errors gracefully
   - Display loading states

3. **Result Display**
   - Code view with syntax highlighting
   - Result cards for semantic search
   - JSON view for raw responses

### Phase 4: Audit Trail (Week 2-3)

1. **Audit Service**
   - Create log ingestion endpoint
   - Implement ring buffer storage
   - Add filtering/search

2. **Audit UI**
   - Timeline component
   - Filter controls
   - Auto-scroll behavior

3. **Integration**
   - Hook into existing logging
   - Create structured log adapter
   - Add trace ID propagation

### Phase 5: Gap Analysis (Week 3)

1. **Gap Detection**
   - Define feature checklist
   - Implement health checks per feature
   - Store results in state

2. **Gap UI**
   - Category sections
   - Feature status cards
   - Issue display

3. **Issue Tracking**
   - Integrate with PROJECT_STATE.md
   - Show known issues
   - Link to issue tracker

### Phase 6: Polish & Deploy (Week 3-4)

1. **Settings Page**
   - Connection settings
   - Default values
   - Danger zone

2. **Error Handling**
   - Toast notifications
   - Inline error display
   - Graceful degradation

3. **Documentation**
   - Update README with playground info
   - Add screenshots
   - Document API

---

## Security Considerations

### Current (MVP)

- **Local-only binding:** Listen on 127.0.0.1 only
- **No authentication:** Internal tooling only
- **Read-only graph queries:** Block write operations in Cypher

### Future (Production)

- **Authentication:** Add API key or OAuth support
- **Rate limiting:** Prevent abuse
- **Input validation:** Sanitize all user inputs
- **CORS:** Restrict to known origins

---

## Performance Targets

| Metric | Target | Notes |
|--------|--------|-------|
| Page load | <500ms | First paint |
| Query latency | <200ms | P95 |
| WebSocket latency | <50ms | Message delivery |
| Audit log capacity | 10K entries | Ring buffer |
| Concurrent connections | 50 | WebSocket clients |

---

## Testing Strategy

### Unit Tests

- Service status checks
- Query parameter validation
- Gap detection logic
- Audit log rotation

### Integration Tests

- Full query flow (UI → API → DB)
- WebSocket message delivery
- Error handling paths

### Manual Testing Checklist

- [ ] All services show correct status
- [ ] Semantic search returns results
- [ ] Graph visualization renders correctly
- [ ] Audit log updates in real-time
- [ ] Gap analysis reflects current state
- [ ] Settings persist across restarts
- [ ] Error messages are helpful
- [ ] Mobile layout works (if needed)

---

## Appendix A: HTML Template Example

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>rust-brain Playground</title>
  <script src="https://cdn.tailwindcss.com"></script>
  <script src="https://unpkg.com/htmx.org@1.9.10"></script>
  <script src="https://unpkg.com/htmx.org/dist/ext/ws.js"></script>
  <link rel="stylesheet" href="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/styles/github.min.css">
  <script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/highlight.min.js"></script>
  <style>
    .htmx-request .htmx-indicator { display: inline-block; }
    .htmx-indicator { display: none; }
  </style>
</head>
<body class="bg-gray-900 text-gray-100 min-h-screen">
  <div class="flex">
    <!-- Sidebar -->
    <nav class="w-64 bg-gray-800 min-h-screen p-4">
      <div class="text-xl font-bold mb-8">🧠 rust-brain</div>
      <ul class="space-y-2">
        <li><a href="/playground" class="block p-2 rounded hover:bg-gray-700">Dashboard</a></li>
        <li><a href="/playground/query" class="block p-2 rounded hover:bg-gray-700">Query</a></li>
        <li><a href="/playground/audit" class="block p-2 rounded hover:bg-gray-700">Audit Trail</a></li>
        <li><a href="/playground/gaps" class="block p-2 rounded hover:bg-gray-700">Gap Analysis</a></li>
        <li><a href="/playground/settings" class="block p-2 rounded hover:bg-gray-700">Settings</a></li>
      </ul>
    </nav>
    
    <!-- Main Content -->
    <main class="flex-1 p-8">
      <div id="content" hx-ext="ws" ws-connect="/playground/ws">
        <!-- Content loaded via HTMX -->
      </div>
    </main>
  </div>
  
  <!-- Toast Container -->
  <div id="toast-container" class="fixed bottom-4 right-4 space-y-2 z-50"></div>
  
  <script>
    // Initialize HTMX WebSocket extension
    document.body.addEventListener('htmx:wsMessage', function(evt) {
      const data = JSON.parse(evt.detail.message);
      if (data.type === 'audit') {
        appendAuditEntry(data.data);
      } else if (data.type === 'status') {
        updateStatus(data.data);
      }
    });
    
    function appendAuditEntry(entry) {
      const container = document.getElementById('audit-entries');
      if (container) {
        const levelColors = {
          'info': 'text-blue-400',
          'warn': 'text-yellow-400',
          'error': 'text-red-400'
        };
        const html = `
          <div class="log-entry border-l-2 border-gray-700 pl-4 py-2">
            <span class="text-gray-500 text-sm">${entry.timestamp}</span>
            <span class="${levelColors[entry.level] || 'text-gray-400'} font-mono ml-2">[${entry.level.toUpperCase()}]</span>
            <span class="text-gray-400 ml-2">[${entry.source}]</span>
            <span class="ml-2">${entry.message}</span>
          </div>
        `;
        container.insertAdjacentHTML('afterbegin', html);
      }
    }
    
    hljs.highlightAll();
  </script>
</body>
</html>
```

---

## Appendix B: Rust Route Example

```rust
use axum::{
    extract::{State, WebSocketUpgrade},
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use tower_http::services::ServeDir;

pub fn playground_routes(state: PlaygroundState) -> Router {
    Router::new()
        // Static files
        .nest_service("/static", ServeDir::new("static"))
        
        // HTML pages
        .route("/playground", get(dashboard))
        .route("/playground/query", get(query_page))
        .route("/playground/audit", get(audit_page))
        .route("/playground/gaps", get(gaps_page))
        .route("/playground/settings", get(settings_page))
        
        // API endpoints
        .route("/playground/status", get(get_status))
        .route("/playground/stats", get(get_stats))
        .route("/playground/tasks", get(get_tasks))
        .route("/playground/tasks", post(start_task))
        .route("/playground/gaps", get(get_gaps))
        .route("/playground/audit", get(get_audit_entries))
        .route("/playground/query/semantic", post(query_semantic))
        .route("/playground/query/function", post(query_function))
        
        // WebSocket
        .route("/playground/ws", get(websocket_handler))
        
        .with_state(state)
}

async fn dashboard() -> Html<&'static str> {
    Html(include_str!("../static/dashboard.html"))
}

async fn get_status(State(state): State<PlaygroundState>) -> impl IntoResponse {
    let status = state.status.read().await;
    Json(status.clone())
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<PlaygroundState>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_websocket(socket, state))
}
```

---

## Appendix C: Future Enhancements

1. **Code Graph Visualization**
   - Interactive D3.js force-directed graph
   - Click nodes to expand relationships
   - Color-code by item type
   - Filter by relationship type

2. **Impact Analysis**
   - Visualize what would be affected by a change
   - Show dependency chains
   - Risk scoring

3. **Comparison Views**
   - Compare two versions of code
   - Highlight structural changes
   - Track API evolution

4. **Export Capabilities**
   - Export queries as cURL commands
   - Export results as JSON/CSV
   - Generate shareable links

5. **Collaboration**
   - Share queries with team
   - Saved query library
   - Annotations on results

---

*Document created: 2026-03-14*  
*Last updated: 2026-03-14*
