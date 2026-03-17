# rust-brain Playground

The unified playground UI provides interactive exploration of Rust codebases with AI-powered insights. Access at **http://localhost:8088/playground**.

## Overview

The playground is a browser-based IDE featuring:
- **10 specialized views** for different aspects of code analysis
- **Streaming chat** with real-time tool invocations
- **Interactive visualizations** (call graphs, dependency trees, type hierarchies)
- **Keyboard-driven navigation** for power users
- **Responsive three-panel layout** (sidebar, main, detail)

## Features

### Dashboard

High-level overview of the codebase and ingestion pipeline.

**Displays:**
- Total functions, types, traits ingested
- Ingestion pipeline status (parse, typecheck, graph, embed)
- Recent searches and activities
- Health metrics (Neo4j nodes, Qdrant collections, DB size)

**Actions:**
- Start/stop ingestion
- View recent analysis results
- Export statistics

---

### Search

Semantic and full-text code search.

**Features:**
- **Semantic Search**: Natural language queries → vector embeddings → top-K results
- **Full-Text**: Keyword search with regex support
- **Filters**: By type (function, struct, trait, module), visibility (pub/private), crate
- **Sorting**: Relevance, line count, modification date

**Example Queries:**
- "find functions that validate user input"
- "async error handlers"
- "trait implementations for Clone"

**Results Format:**
- Code snippet with syntax highlighting
- File path and line numbers
- Relevance score and match context
- Jump to code action

---

### Call Graph

Interactive visualization of function calls and dependencies.

**Features:**
- **Interactive Rendering**: D3.js force-directed layout
- **Bidirectional Navigation**: Click → drill down, zoom → highlight subgraph
- **Filtering**: Show only transitive callers, limit depth
- **Metrics**: Call count, recursion detection, unused functions
- **Export**: PNG/SVG snapshot, adjacency matrix

**Controls:**
- Click node → inspect in detail panel
- Drag → pan, scroll → zoom
- Double-click → center and expand
- Right-click → context menu (expand, highlight, isolate)

---

### Chat

AI-powered code exploration with streaming responses.

**Capabilities:**
- **Question Answering**: "What does this function do?" "Why might this fail?"
- **Refactoring Suggestions**: "How can I simplify this code?"
- **Impact Analysis**: "What breaks if I change this function?"
- **Documentation Generation**: Auto-generate ADRs, docstrings, type annotations
- **Tool Integration**: Automatically invokes tools (search, graph, type info) to answer questions

**Features:**
- **Streaming Responses**: See Claude's thoughts as they stream in
- **Tool Visibility**: See every tool invocation, arguments, and results
- **Context Preservation**: Multi-turn conversations with codebase context
- **Markdown Rendering**: Formatted code blocks, links, tables
- **Code Block Actions**: Copy, jump to file, compare versions

**Example Interactions:**
```
User: "Is this function thread-safe?"
   ↓ (Claude invokes: search for unsafe, get_function_details, get_trait_impls)
Claude: "Yes, it uses Arc<Mutex<>> which is thread-safe. Here's why..." [with code examples]

User: "What are the side effects?"
   ↓ (Claude searches for call sites, mutation patterns)
Claude: "This function modifies shared state via the Mutex. Callers must handle blocking..." [detailed analysis]
```

---

### Cypher

Raw Neo4j query interface for advanced graph analysis.

**Use Cases:**
- **Custom Traversals**: Find all transitively unsafe functions
- **Pattern Matching**: Functions with >10 parameters, cyclic dependencies, unused exports
- **Statistics**: Average function size, trait saturation, module complexity
- **Mutations**: Bulk tagging, merge duplicate types, fix parsing artifacts

**Example Queries:**
```cypher
// Find all functions calling external APIs
MATCH (f:Function)-[:CALLS]->(ext:ExternalFunction)
RETURN f.fqn, ext.module, count(*) as call_count
ORDER BY call_count DESC

// Type hierarchy
MATCH (t:Type)<-[:IMPLEMENTS]-(impl:Implementation)
RETURN t.name, collect(impl.fqn) as implementations

// Complex dependency cycles
MATCH (m1:Module)-[:DEPENDS_ON*2..]->(m1)
RETURN m1.name as cycle_start, [nodes in path | nodes.name] as cycle_path
```

**Features:**
- Syntax highlighting for Cypher
- Result formatting (table, JSON, graph)
- Query history and save/load
- Execution time tracking
- EXPLAIN/PROFILE support

---

### Types

Browse and analyze types in the codebase.

**Views:**
- **Type List**: Searchable list of all types, enums, unions
- **Type Detail**: Fields, methods, trait implementations, usage sites
- **Type Hierarchy**: Visualization of composition and inheritance
- **Generics**: Generic parameters, bounds, monomorphizations

**Displays:**
- Type definition with source location
- All implementations of traits
- All usages (function parameters, field types, return types)
- Size and memory layout (if available)
- Derives and custom attributes

**Actions:**
- Find all usages of a type
- Show trait implementations
- Generate boilerplate (tests, builders, from/into)
- Navigate to definition

---

### Traits

Trait definitions, implementations, and analysis.

**Features:**
- **Trait Catalog**: All traits with method count, implementation count
- **Implementation Finder**: Find all types implementing a trait
- **Bound Analysis**: Show all trait bounds in the codebase
- **Orphan Rule Check**: Identify potential coherence issues
- **Coverage**: Percentage of types implementing common traits

**Views:**
- Trait definition with methods and bounds
- All implementations (local and external)
- Callers of each method
- Standard library trait adoption rates

---

### Modules

Module hierarchy and visibility analysis.

**Features:**
- **Module Tree**: Nested module structure with expand/collapse
- **Visibility Analysis**: Public exports, private internals, re-exports
- **Module Graph**: Dependency relationships and cycles
- **Documentation**: Module-level docs and examples
- **Public API**: What's exposed to external users

**Displays:**
- Module path (e.g., `crate::services::api::handlers`)
- Exported types, traits, functions
- Module dependencies and dependents
- Size metrics (lines of code, number of items)

---

### Audit

Code quality metrics and analysis.

**Reports:**
- **Complexity**: Cyclomatic complexity, cognitive complexity per function
- **Coverage**: Lines of code, documented items percentage
- **Dependencies**: Unused imports, duplicate dependencies
- **Unsafe Code**: Unsafe blocks, justifications, frequency
- **Deprecations**: Deprecated API usage, migration paths

**Metrics:**
- Function count distribution (by size, complexity, parameter count)
- Test coverage (estimated from test file presence)
- Documentation coverage
- Code health score

---

### Gaps

Missing implementations and analysis gaps.

**Findings:**
- **Unimplemented Traits**: Types that should implement common traits
- **Missing Documentation**: Public APIs without doc comments
- **Incomplete Generics**: Generic functions without sufficient bounds
- **Parsing Gaps**: Items that couldn't be fully resolved (external deps, macros)

**Analysis:**
- Suggested trait implementations
- Documentation templates
- Dependency resolution status
- Known limitations and workarounds

---

## Keyboard Shortcuts

### Navigation
| Key | Action |
|-----|--------|
| `Cmd+K` / `Ctrl+K` | Open command palette |
| `Cmd+1` to `Cmd+9` | Jump to tab (1=Dashboard, 2=Search, ..., 9=Gaps) |
| `Escape` | Close overlays, clear search |

### Sidebar & Layout
| Key | Action |
|-----|--------|
| `Cmd+B` | Toggle sidebar |
| `Cmd+Shift+B` | Toggle detail panel |
| `Cmd+/` | Toggle chat sidebar |
| Drag resizer | Resize panels manually |

### Search & Chat
| Key | Action |
|-----|--------|
| `Cmd+F` | Focus search input |
| `Cmd+Shift+F` | Global code search |
| `Cmd+Enter` | Submit query / send message |
| `Shift+Enter` | New line in textarea |
| `Tab` | Autocomplete search filters |

### In Results
| Key | Action |
|-----|--------|
| `Enter` | View detail for selected result |
| `Ctrl+C` | Copy code block |
| `Cmd+L` | Open in external editor (if configured) |

---

## Streaming Architecture

The chat and some search operations use **Server-Sent Events (SSE)** for streaming responses.

### Flow

```
Browser                    API Server                    Claude
  │                          │                              │
  ├──[POST /chat]──────────>│                              │
  │                          ├──[invoke MCP tools]────────>│
  │                          │                              │
  │  ┌──[SSE stream]←────────│<──[streaming response]──────┤
  │  │  tool: search         │  (with tool calls)           │
  │  │  invoke: /tools/...   │                              │
  │  │                       │                              │
  │  │  tool_result: [...] │                              │
  │  │                       │                              │
  │  │  text: "Based on..."  │                              │
  │  └──[SSE close]←─────────│                              │
  │                          │                              │
```

### Events in Stream

1. **tool**: Tool name to invoke
2. **invoke**: Endpoint path (e.g., `/tools/search_semantic`)
3. **args**: JSON arguments for the tool
4. **tool_result**: Result from the tool (received mid-stream)
5. **text**: Streamed response text
6. **stop_reason**: Final message (stop, tool_use, etc.)
7. **usage**: Token counts

### Benefits

- **Real-time feedback**: See responses as they're generated
- **Tool transparency**: Understand how the AI is analyzing your code
- **Early termination**: Stop long queries without waiting for completion
- **Backpressure handling**: Browser can pause/resume stream

---

## Tool Call Visibility

All tool invocations in the Chat view are rendered with:

1. **Tool Name & Arguments**: What the AI decided to invoke and why
2. **Execution Time**: How long the tool took
3. **Result Preview**: First 500 chars of the result
4. **Expand Action**: Click to see full result in detail panel
5. **Copy & Export**: Export tool results for debugging

**Example Rendering:**
```
🔧 Tool: search_semantic
   Query: "error handling patterns"
   Top-k: 5
   ⏱️ 245ms

📋 Results: (5 matches, avg similarity 0.87)
   1. src/error.rs:42 - custom error type impl
   2. src/handlers/mod.rs:156 - ? operator usage
   ... [expand to see full results]
```

---

## Session Management

### Persistence

- **Session ID**: Unique browser session, persisted in localStorage
- **History**: Recent searches, queries, open tabs
- **Preferences**: Tab size, theme (light/dark), sidebar width
- **Favorites**: Bookmark functions, types, searches

### Export

- **Session Export**: Download session JSON with all history
- **Code Export**: Copy/download explored code sections
- **Analysis Export**: Export graphs, reports as PNG/SVG/CSV

---

## Troubleshooting

### Chat Not Responding
1. Check `ws://localhost:3001` is accessible (MCP SSE server)
2. Verify `ANTHROPIC_API_KEY` is set in `.env`
3. Check browser console for connection errors
4. Inspect Network tab for SSE stream status

### Search Results Empty
1. Verify ingestion completed (check Dashboard)
2. Check Qdrant collections exist: `curl http://localhost:6333/collections`
3. Try broader search terms
4. Use Cypher tab to manually query: `MATCH (n) RETURN count(*) as total`

### Call Graph Rendering Issues
1. Check for large graphs (>1000 nodes) → increase timeout
2. Clear browser cache
3. Try filtering by depth/type
4. Use Cypher tab for precise queries

### Keyboard Shortcuts Not Working
1. Ensure playground is focused (click in main area)
2. Check for browser extensions that override keybindings
3. Verify OS keybindings don't conflict (macOS: System Settings → Keyboard)
4. Try alternative bindings in settings

---

## Architecture

### Frontend Stack
- **Framework**: Vanilla JS (ES modules)
- **UI Framework**: Custom CSS grid + flexbox
- **Visualization**: D3.js v7 (call graphs), highlight.js (syntax)
- **Markdown**: marked.js

### Backend Integration
- **REST API**: `/playground`, `/tools/*`, `/chat`
- **Streaming**: SSE for chat and streaming results
- **MCP**: Tool definitions fetched from `/mcp/tools`

### State Management
- **Session**: localStorage + IndexedDB for persistence
- **Cache**: LRU cache for recent searches and call graphs
- **Realtime**: WebSocket for live collaboration (future)

---

## Best Practices

1. **Start with Dashboard**: Get overview before diving into details
2. **Use Search for discovery**: Find similar patterns, learn idioms
3. **Explore Call Graphs**: Understand flow before making changes
4. **Chat for context**: Ask "why" questions that need reasoning
5. **Cypher for validation**: Double-check findings with precise queries
6. **Shortcuts for power users**: Cmd+K palette for fast navigation

---

## Limitations & Future Work

### Known Limitations
- External dependencies shown as nodes but not introspected
- Macro expansions not included in analysis
- Generic monomorphizations approximated
- Async/await control flow not fully captured

### Planned Features
- Live collaboration (multiple users exploring same codebase)
- Code editing with real-time analysis updates
- Plugin system for custom tools
- Bookmark management and sharing
- Integration with GitHub issues/PRs

---

## See Also

- [opencode-integration.md](./opencode-integration.md) — OpenCode IDE + LiteLLM setup
- [architecture.md](./architecture.md) — System design and data flow
- [runbook.md](./runbook.md) — Operations and troubleshooting
