# Ingestion Pipeline: Step-by-Step Detailed Walkthrough

This document provides a line-by-line explanation of the rust-brain ingestion process, including all libraries, functions, inputs, outputs, and how each step connects to the next.

---

## Table of Contents

1. [Entry Point](#1-entry-point)
2. [Pipeline Initialization](#2-pipeline-initialization)
3. [Stage 1: ExpandStage](#3-stage-1-expandstage)
4. [Stage 2: ParseStage](#4-stage-2-parsestage)
5. [Stage 3: TypecheckStage](#5-stage-3-typecheckstage)
6. [Stage 4: ExtractStage](#6-stage-4-extractstage)
7. [Stage 5: GraphStage](#7-stage-5-graphstage)
8. [Stage 6: EmbedStage](#8-stage-6-embedstage)
9. [Data Flow Diagram](#9-data-flow-diagram)

---

## 1. Entry Point

### File: `services/ingestion/src/main.rs`

### Step 1.1: Parse CLI Arguments

**Library**: `clap` (Command Line Argument Parser)  
**Function**: `clap::Parser::parse()`

```rust
// main.rs:113
let args = Args::parse();
```

**Input**: Command line arguments (e.g., `rustbrain-ingestion -c /path/to/crate -d postgres://...`)

**Output**: `Args` struct with fields:
```rust
struct Args {
    crate_path: PathBuf,      // -c, --crate-path (default: ".")
    database_url: Option<String>,  // -d, --database-url or $DATABASE_URL
    neo4j_url: Option<String>,     // --neo4j-url or $NEO4J_URL
    embedding_url: Option<String>, // --embedding-url or $EMBEDDING_URL
    stages: Option<Vec<String>>,   // -s, --stages (comma-separated)
    dry_run: bool,                 // --dry-run
    fail_fast: bool,               // --fail-fast
    max_concurrency: usize,        // --max-concurrency (default: 4)
    verbose: bool,                 // -v, --verbose
}
```

**Why**: `clap` provides derive-based argument parsing with automatic help generation, validation, and environment variable support.

---

### Step 1.2: Initialize Logging

**Library**: `tracing` + `tracing-subscriber`  
**Function**: `FmtSubscriber::builder().init()`

```rust
// main.rs:116-122
let log_level = if args.verbose { Level::DEBUG } else { Level::INFO };
let _subscriber = FmtSubscriber::builder()
    .with_max_level(log_level)
    .with_target(false)
    .with_thread_ids(false)
    .pretty()
    .init();
```

**Input**: Log level (DEBUG or INFO based on `--verbose` flag)

**Output**: Global subscriber registered for structured logging

**Why**: `tracing` provides structured, async-aware logging with spans for tracking operations across threads.

---

### Step 1.3: Build Pipeline Configuration

```rust
// main.rs:128-139
let config = PipelineConfig {
    crate_path: args.crate_path.clone(),
    database_url: args.database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .expect("DATABASE_URL must be provided"),
    neo4j_url: args.neo4j_url,
    embedding_url: args.embedding_url,
    stages: args.stages,
    dry_run: args.dry_run,
    continue_on_error: !args.fail_fast,
    max_concurrency: args.max_concurrency,
};
```

**Input**: CLI arguments

**Output**: `PipelineConfig` struct

---

### Step 1.4: Create Pipeline Runner

**Library**: Custom `PipelineRunner`  
**Function**: `PipelineRunner::new(config)`

```rust
// main.rs:152-153
let mut runner = PipelineRunner::new(config)
    .context("Failed to create pipeline runner")?;
```

**What happens inside** (runner.rs:44-64):

```rust
pub fn new(config: PipelineConfig) -> Result<Self> {
    let ctx = PipelineContext::new(config.clone());
    
    // Initialize stages in order
    let stages: Vec<Box<dyn PipelineStage>> = vec![
        Box::new(ExpandStage::new()?),
        Box::new(ParseStage::new()?),
        Box::new(TypecheckStage::new()),
        Box::new(ExtractStage::new()),
        Box::new(GraphStage::new()),
        Box::new(EmbedStage::new()),
    ];
    
    Ok(Self { ctx, pool: None, stages, ... })
}
```

**Output**: `PipelineRunner` with:
- `ctx`: Shared `PipelineContext` with unique `PipelineId` (UUID)
- `stages`: Ordered list of 6 pipeline stages
- `pool`: Optional database connection pool

---

### Step 1.5: Connect to Database

**Library**: `sqlx`  
**Function**: `PgPoolOptions::new().connect()`

```rust
// main.rs:156-159
if !args.dry_run {
    runner.connect().await
        .context("Failed to connect to database")?;
}
```

**What happens inside** (runner.rs:89-113):
1. Creates PostgreSQL connection pool with max 5 connections
2. Initializes `ResilienceCoordinator` for memory management
3. Initializes `Monitor` for progress tracking
4. Creates ingestion run record in database

---

### Step 1.6: Run Pipeline

```rust
// main.rs:162-163
let result = runner.run().await
    .context("Pipeline execution failed")?;
```

**This triggers the main pipeline execution loop** (runner.rs:118-340).

---

## 2. Pipeline Initialization

### Shared State: `PipelineContext`

**File**: `services/ingestion/src/pipeline/mod.rs`

```rust
pub struct PipelineContext {
    pub id: PipelineId,                           // UUID for this run
    pub config: PipelineConfig,                   // CLI configuration
    pub state: Arc<RwLock<PipelineState>>,        // Mutable shared state
}

pub struct PipelineState {
    pub source_files: Vec<SourceFileInfo>,        // Discovered .rs files
    pub expanded_sources: Arc<HashMap<PathBuf, PathBuf>>,  // source_path -> cache_path
    pub parsed_items: HashMap<PathBuf, Vec<ParsedItemInfo>>, // file -> items
    pub extracted_items: HashMap<String, Uuid>,   // FQN -> database ID
    pub graph_nodes: HashMap<String, String>,     // FQN -> Neo4j node ID
    pub errors: Vec<StageError>,                  // Accumulated errors
    pub counts: StageCounts,                      // Metrics
    pub expand_cache: HashMap<String, String>,    // Incremental run cache
    pub store_references: HashMap<String, StoreReference>, // Cross-store refs
}
```

**Why**: `Arc<RwLock<>>` allows multiple stages to read state concurrently, while writes are exclusive. This is the communication mechanism between stages.

---

## 3. Stage 1: ExpandStage

### Purpose
Expand all macros in the Rust codebase using `cargo expand`, producing fully-expanded source code for analysis.

### File: `services/ingestion/src/pipeline/stages.rs:498-1472`

---

### Step 3.1: Discover Crates

**Library**: `walkdir`  
**Function**: `WalkDir::new().into_iter()`

```rust
// stages.rs:966-997
async fn discover_crates(&self, workspace_path: &Path) -> Result<Vec<PathBuf>> {
    let cargo_toml = workspace_path.join("Cargo.toml");
    
    if content.contains("[workspace]") {
        // Find workspace members
        for entry in WalkDir::new(workspace_path)
            .min_depth(1)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_dir())
        {
            if entry.path().join("Cargo.toml").exists() {
                crates.push(entry.path().to_path_buf());
            }
        }
    } else {
        // Single crate
        Ok(vec![workspace_path.to_path_buf()])
    }
}
```

**Input**: Workspace root path

**Output**: List of crate paths (workspace members or single crate)

**Why**: `walkdir` provides efficient recursive directory traversal with filtering.

**Example**:
```
Input:  /home/user/hyperswitch
Output: [
    /home/user/hyperswitch/router,
    /home/user/hyperswitch/crate_a,
    /home/user/hyperswitch/crate_b,
    ...
]
```

---

### Step 3.2: Pre-patch Cargo.toml Files

**Library**: `toml_edit`  
**Function**: `DocumentMut::parse()`, table manipulation

```rust
// stages.rs:1017-1078
fn pre_patch_crates_for_features(&self, workspace_root: &Path, crates: &[PathBuf]) 
    -> HashMap<PathBuf, String> 
{
    // Find crates that need hyperswitch_domain_models with olap,frm features
    // or storage_impl with olap feature
    
    // Patch Cargo.toml to add these features
    // Returns map of (crate_path -> original_content) for later restoration
}
```

**Why**: Some crates have complex feature dependencies that require explicit feature enabling for `cargo expand` to succeed.

---

### Step 3.3: Run Cargo Expand

**Library**: `std::process::Command`  
**External Tool**: `cargo-expand` (must be installed)

```rust
// stages.rs:846-937
fn run_cargo_expand(&self, workspace_path: &Path, crate_name: &str, extra_args: &[&str]) 
    -> Result<String> 
{
    let mut args: Vec<&str> = vec!["expand", "--lib", "-p", crate_name, "--jobs", "1", "--ugly"];
    args.extend(extra_args);

    let mut child = Command::new("cargo")
        .args(&args)
        .env("RUSTFLAGS", "-C codegen-units=16")
        .env("CARGO_BUILD_JOBS", "1")
        .current_dir(workspace_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Stream output to buffers with timeout
    // Timeout: 180 seconds (CARGO_EXPAND_TIMEOUT)
}
```

**Input**: 
- `workspace_path`: Root of workspace
- `crate_name`: Name of crate to expand (from Cargo.toml)
- `extra_args`: Feature flags like `["--features", "v1,olap"]`

**Output**: Expanded Rust source code as string

**Why**: `cargo expand` runs `rustc --pretty=expanded` internally, producing code with all macros expanded. This is essential for:
1. Seeing generated code from derive macros
2. Understanding macro-generated implementations
3. Analyzing code that the compiler actually sees

**Example**:
```
Input code:
    #[derive(Debug)]
    struct Point { x: i32, y: i32 }

Expanded output:
    struct Point { x: i32, y: i32 }
    impl ::std::fmt::Debug for Point {
        fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
            ::std::fmt::Formatter::debug_struct_field2_finish(
                f,
                "Point",
                "x",
                &&self.x,
                "y",
                &&self.y,
            )
        }
    }
```

---

### Step 3.4: Cache Expanded Source

```rust
// stages.rs:1382-1421
let cache_key = format!("{}-{}.expand", crate_name, content_hash);
let cache_file = PathBuf::from(EXPAND_CACHE_DIR).join(&cache_key);

// After successful expand:
for file_path in &source_files {
    all_source_files.push(SourceFileInfo {
        path: file_path.clone(),
        crate_name: crate_name.clone(),
        module_path: compute_module_path(crate_path, file_path, &crate_name),
        original_source: Arc::new(source),
        git_hash: git_hash.clone(),
        content_hash: file_hash,
    });
    
    // Store cache file path, not content
    expanded_map.insert(file_path.clone(), cache_file.clone());
}
```

**Important**: Expanded source is written to disk cache files (`/tmp/rustbrain-expand-cache/`), NOT stored in memory. The state only stores the path mapping.

---

### Step 3.5: Update State

```rust
// stages.rs:1443-1452
{
    let mut state = ctx.state.write().await;
    state.expanded_sources = Arc::new(expanded_map);  // {source_path -> cache_path}
    state.source_files = all_source_files;            // Vec<SourceFileInfo>
    state.counts.files_expanded = expanded_count;
}
```

**Output to State**:
- `state.expanded_sources`: HashMap mapping each `.rs` file to its expanded cache file
- `state.source_files`: Metadata for each source file

---

## 4. Stage 2: ParseStage

### Purpose
Parse expanded Rust source code into structured `ParsedItem` objects using a dual-parser strategy (tree-sitter + syn).

### File: `services/ingestion/src/pipeline/stages.rs:1474-1818`

---

### Step 4.1: Initialize Dual Parser

**Library**: `tree-sitter` + `tree-sitter-rust` + `syn`  
**Function**: `DualParser::new()`

```rust
// stages.rs:1490-1496
impl ParseStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            parser: Arc::new(DualParser::new()?),
            derive_detector: Arc::new(DeriveDetector::new()),
        })
    }
}
```

**DualParser** (parsers/mod.rs:130-138):
```rust
pub struct DualParser {
    tree_sitter: TreeSitterParser,
    syn: SynParser,
}

impl DualParser {
    pub fn new() -> Result<Self> {
        let tree_sitter = TreeSitterParser::new()?;  // Fast skeleton extraction
        let syn = SynParser::new();                   // Deep semantic parsing
        Ok(Self { tree_sitter, syn })
    }
}
```

---

### Step 4.2: Phase 1 - Tree-Sitter Skeleton Extraction

**Library**: `tree-sitter` + `tree-sitter-rust`  
**Function**: `Parser::parse()`, cursor traversal

```rust
// parsers/tree_sitter_parser.rs:32-52
pub fn extract_skeletons(&self, source: &str) -> Result<Vec<SkeletonItem>> {
    let source_bytes = source.as_bytes();
    let tree = self.parser.lock().unwrap().parse(source, None)?;

    let root = tree.root_node();
    let mut skeletons = Vec::new();
    let mut cursor = root.walk();
    
    self.collect_items(&mut cursor, source_bytes, &mut skeletons);
    
    skeletons.sort_by_key(|s| s.start_byte);
    Ok(skeletons)
}
```

**How tree-sitter works**:
1. Generates a concrete syntax tree (CST) from source code
2. Each node has: `kind()`, `start_byte()`, `end_byte()`, `start_position()`, `end_position()`
3. Cursor API allows efficient tree traversal

**Input**: Source code string

**Output**: `Vec<SkeletonItem>`

```rust
pub struct SkeletonItem {
    pub item_type: ItemType,      // function, struct, enum, trait, impl, etc.
    pub name: Option<String>,     // Item name (None for use declarations)
    pub start_byte: usize,        // Byte offset in source
    pub end_byte: usize,          // Byte offset in source
    pub start_line: usize,        // 1-indexed line number
    pub end_line: usize,          // 1-indexed line number
}
```

**Why tree-sitter**:
- **Fast**: Incremental parsing, handles malformed code gracefully
- **Robust**: Can parse code with syntax errors
- **Lightweight**: Only extracts structural information, not semantic details

**Example**:
```
Input:
    pub fn hello<T: Clone>(x: T) -> T {
        x.clone()
    }

Output:
    SkeletonItem {
        item_type: ItemType::Function,
        name: Some("hello"),
        start_byte: 0,
        end_byte: 60,
        start_line: 1,
        end_line: 3,
    }
```

---

### Step 4.3: Phase 2 - Syn Deep Parsing

**Library**: `syn`  
**Function**: `syn::parse_str::<SynItem>()`

```rust
// parsers/syn_parser.rs:43-58
pub fn parse_item(&self, source: &str, module_path: &str, skeleton: &SkeletonItem) 
    -> Result<ParsedItem> 
{
    // Try to parse as a valid Rust item
    let item: SynItem = syn::parse_str(source).with_context(|| {
        format!("Failed to parse item: {}", source.lines().next().unwrap_or(""))
    })?;

    self.item_to_parsed(item, source, module_path, skeleton)
}
```

**How syn works**:
1. Parses Rust source into a full AST
2. Provides detailed type information, generics, where clauses, attributes
3. Returns `Item` enum with variants for each item type

**Input**: Individual item source code (extracted from skeleton byte range)

**Output**: `ParsedItem`

```rust
pub struct ParsedItem {
    pub fqn: String,                    // "crate::module::function_name"
    pub item_type: ItemType,            // function, struct, enum, etc.
    pub name: String,                   // "function_name"
    pub visibility: Visibility,         // public, pub(crate), private
    pub signature: String,              // "fn hello<T: Clone>(x: T) -> T"
    pub generic_params: Vec<GenericParam>,  // [{name: "T", bounds: ["Clone"]}]
    pub where_clauses: Vec<WhereClause>,    // ["T: Clone", "U: Send"]
    pub attributes: Vec<String>,        // ["#[derive(Clone)]", "#[cfg(test)]"]
    pub doc_comment: String,            // "This function does X"
    pub start_line: usize,              // 1
    pub end_line: usize,                // 10
    pub body_source: String,            // Full source code (truncated if > 50KB)
    pub generated_by: Option<String>,   // "derive(Debug)" if macro-generated
}
```

**Why syn**:
- **Accurate**: Full semantic parsing with type information
- **Detailed**: Extracts generics, where clauses, attributes
- **Standard**: Uses the same parser as `rust-analyzer`

**Example - Function Parsing**:
```
Input (from tree-sitter skeleton):
    pub fn hello<T: Clone>(x: T) -> T {
        x.clone()
    }

Syn parsing:
    1. Parse as Item::Fn
    2. Extract:
       - name: "hello" from item.sig.ident
       - visibility: Public from item.vis
       - generics: [GenericParam { name: "T", bounds: ["Clone"] }]
       - signature: "fn hello<T: Clone>(x: T) -> T"
       - body: "{ x.clone() }"

Output:
    ParsedItem {
        fqn: "crate::module::hello",
        item_type: ItemType::Function,
        name: "hello",
        visibility: Visibility::Public,
        signature: "fn hello<T: Clone>(x: T) -> T",
        generic_params: vec![GenericParam { name: "T", bounds: vec!["Clone"] }],
        where_clauses: vec![],
        attributes: vec![],
        doc_comment: "",
        start_line: 1,
        end_line: 3,
        body_source: "{\n    x.clone()\n}",
        generated_by: None,
    }
```

**Example - Impl Block Parsing** (syn_parser.rs:264-300):
```
Input:
    impl Clone for Point {
        fn clone(&self) -> Self {
            Point { x: self.x, y: self.y }
        }
    }

Syn parsing:
    1. Parse as Item::Impl
    2. Check if trait impl: item.trait_ = Some((_, path, _))
    3. Extract:
       - trait_name: "Clone"
       - self_type: "Point"
       - name: "Clone_Point" (format: {Trait}_{Type})
       - attribute: "impl_for=Clone"

Output:
    ParsedItem {
        fqn: "crate::module::Clone_Point",
        item_type: ItemType::Impl,
        name: "Clone_Point",
        signature: "impl Clone for Point { ... }",
        attributes: vec!["impl_for=Clone"],
        ...
    }
```

---

### Step 4.4: Fallback on Syn Failure

```rust
// parsers/mod.rs:173-202
match self.syn.parse_item(item_source, module_path, &skeleton) {
    Ok(parsed_item) => {
        items.push(parsed_item);
    }
    Err(e) => {
        errors.push(ParseError { ... });
        partial_items.push(skeleton.clone());
        
        // Create minimal ParsedItem from tree-sitter data
        if let Some(name) = &skeleton.name {
            let partial_parsed = self.create_partial_item(
                source, item_source, module_path, name, &skeleton,
            );
            items.push(partial_parsed);
        }
    }
}
```

**Why fallback**: Syn requires valid Rust syntax. Expanded code sometimes has edge cases that syn can't parse. Tree-sitter is more forgiving.

---

### Step 4.5: Store Parsed Items

```rust
// stages.rs:1684-1712
for (path, file_items) in &all_parsed_items {
    let fqns: Vec<String> = file_items.iter().map(|i| i.fqn.clone()).collect();
    
    // Batch insert into extracted_items table
    let json_values: Vec<serde_json::Value> = file_items.iter().map(|item| {
        serde_json::json!({
            "id": Uuid::new_v4(),
            "item_type": item.item_type,
            "fqn": item.fqn,
            "name": item.name,
            "visibility": item.visibility,
            "signature": item.signature,
            ...
        })
    }).collect();
    
    sqlx::query(&insert_query).bind(&json_values).execute(&pool).await?;
}
```

**Output to Database**: Items inserted into `extracted_items` table

**Output to State**:
- `state.parsed_items`: HashMap<PathBuf, Vec<ParsedItemInfo>>
- `state.counts.items_parsed`: Total count

---

## 5. Stage 3: TypecheckStage

### Purpose
Analyze expanded source code to extract:
1. **Call sites**: Monomorphized function calls with turbofish syntax
2. **Trait implementations**: Quality analysis of trait impls

**⚠️ KNOWN BUG**: This stage is currently broken due to state being cleared by ParseStage. See [ISSUE-001](./issues/ISSUE-001-typecheck-stage-skipped.md).

### File: `services/ingestion/src/pipeline/stages.rs:1880-2018`

---

### Step 5.1: Check for Expanded Sources

```rust
// stages.rs:1905-1914
let state = ctx.state.read().await;
let expanded_cache_paths = state.expanded_sources.clone();
drop(state);

if expanded_cache_paths.is_empty() {
    info!("No expanded sources to typecheck");
    return Ok(StageResult::skipped("typecheck"));  // ← ALWAYS TRUE DUE TO BUG
}
```

---

### Step 5.2: Analyze Expanded Source

**Library**: Custom `TypeResolutionService`  
**Function**: `analyze_expanded_source()`

```rust
// stages.rs:1973-1997
match type_resolution_service.analyze_expanded_source(
    &file_info.crate_name,
    &file_info.module_path,
    &file_info.path.to_string_lossy(),
    &expanded_source,
    &caller_fqns,
).await {
    Ok(result) => {
        typechecked_count += 1;
        trait_impls_count += result.trait_impls.len();
        call_sites_count += result.call_sites.len();
    }
    Err(e) => {
        warn!("Type resolution failed for {:?}: {}", file_info.path, e);
    }
}
```

**What TypeResolutionService does** (typecheck/resolver.rs):
1. Parses expanded source with syn
2. Visits all function calls looking for turbofish syntax: `function::<Type>()`
3. Extracts type arguments and records call sites
4. Analyzes impl blocks for trait implementation quality

**Turbofish Detection** (typecheck/resolver.rs:443-469):
```rust
fn extract_call_site(&self, call: &ExprCall, ...) -> Option<CallSite> {
    // Look for turbofish: function::<Type>()
    let type_args = self.extract_turbofish_types(path_expr);
    let is_monomorphized = !type_args.is_empty();
    
    if is_monomorphized {
        Some(CallSite {
            caller_fqn,
            callee_fqn,
            type_arguments,
            is_monomorphized: true,
            ...
        })
    } else {
        None
    }
}
```

**Output to Database**:
- `call_sites` table: Records monomorphized call sites
- `trait_implementations` table: Records trait impls with quality scores

---

## 6. Stage 4: ExtractStage

### Purpose
Store source files in database and link parsed items to their source files.

### File: `services/ingestion/src/pipeline/stages.rs:2020-2283`

---

### Step 6.1: Store Source Files

```rust
// stages.rs:2073-2106
async fn store_source_file(&self, file_info: &SourceFileInfo, expanded_source: Option<&str>) 
    -> Result<Uuid> 
{
    sqlx::query(r#"
        INSERT INTO source_files
            (id, crate_name, module_path, file_path, original_source, expanded_source, git_hash, content_hash)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (crate_name, module_path, file_path) DO UPDATE SET
            original_source = EXCLUDED.original_source,
            expanded_source = EXCLUDED.expanded_source,
            ...
    "#)
    .bind(id)
    .bind(&file_info.crate_name)
    .bind(&file_info.module_path)
    .bind(file_info.path.to_string_lossy().to_string())
    .bind(file_info.original_source.as_str())
    .bind(expanded_source)
    .fetch_one(pool)
    .await?;
}
```

**Output**: Source files stored in `source_files` table

---

### Step 6.2: Link Items to Source Files

```rust
// stages.rs:2244-2263
let update_result = sqlx::query(r#"
    UPDATE extracted_items ei
    SET source_file_id = sf.id
    FROM source_files sf
    WHERE ei.source_file_id IS NULL
      AND sf.module_path = regexp_replace(ei.fqn, '::[^:]+$', '')
"#)
.execute(pool)
.await;
```

**Why**: Items were inserted during ParseStage with NULL `source_file_id`. This step links them.

---

## 7. Stage 5: GraphStage

### Purpose
Build a Neo4j graph representing code structure and relationships.

### File: `services/ingestion/src/pipeline/stages.rs:2285-3073`

---

### Step 7.1: Create Nodes

**Library**: `neo4rs` (Neo4j Rust driver)  
**Function**: `Graph::run()`, `Query::new()`

```rust
// stages.rs:2637-2676
let mut all_nodes: Vec<NodeData> = Vec::new();

// Create Crate nodes
for crate_name in &crate_names {
    all_nodes.push(Self::create_crate_node(crate_name));
}

// Create item nodes
for (_path, items) in &parsed_items {
    for item in items {
        let node = Self::item_to_node(item);
        all_nodes.push(node);
    }
}

// Batch insert
graph_builder.create_nodes_batch(all_nodes).await?;
```

**Node Structure**:
```rust
pub struct NodeData {
    pub id: String,           // FQN
    pub fqn: String,          // Full qualified name
    pub name: String,         // Short name
    pub node_type: NodeType,  // Function, Struct, Enum, Trait, Impl, etc.
    pub properties: HashMap<String, PropertyValue>,
}
```

**Cypher Generated**:
```cypher
MERGE (n:Function {fqn: $fqn})
SET n.name = $name, n.signature = $signature, n.is_async = $is_async, ...
```

---

### Step 7.2: Create Relationships

```rust
// stages.rs:2684-2799
let mut relationships: Vec<RelationshipData> = Vec::new();

// CONTAINS relationships
for item in &items {
    if let Some(parent_fqn) = Self::get_parent_fqn(&item.fqn) {
        relationships.push(RelationshipBuilder::create_contains(
            parent_fqn, item.fqn, "Module", "Function",
        ));
    }
}

// IMPLEMENTS relationships (Impl -> Trait)
for impl_item in &impl_items {
    if let Some(trait_name) = Self::extract_trait_from_impl(&impl_item.attributes) {
        relationships.push(RelationshipBuilder::create_implements(
            impl_item.fqn.clone(), trait_fqn,
        ));
    }
}

// FOR relationships (Impl -> Type)
for impl_item in &impl_items {
    if let Some(self_type) = Self::extract_impl_self_type(&impl_item.fqn, &impl_item.name) {
        relationships.push(RelationshipBuilder::create_for(
            impl_item.fqn.clone(), type_fqn,
        ));
    }
}

// CALLS relationships (extracted from function bodies)
// USES_TYPE relationships (extracted from signatures and bodies)
```

**Cypher Generated**:
```cypher
MATCH (caller:Function {fqn: $caller_fqn})
MATCH (callee:Function {fqn: $callee_fqn})
MERGE (caller)-[:CALLS {line: $line}]->(callee)
```

---

## 8. Stage 6: EmbedStage

### Purpose
Generate vector embeddings for all parsed items and store them in Qdrant for semantic search.

### File: `services/ingestion/src/pipeline/stages.rs:3075-3430`

---

### Step 8.1: Initialize Embedding Service

**Libraries**: 
- `reqwest` for HTTP calls to Ollama and Qdrant
- Custom clients in `embedding/ollama_client.rs` and `embedding/qdrant_client.rs`

```rust
// embedding/mod.rs:83-96
pub fn new(config: EmbeddingConfig) -> Result<Self> {
    let ollama = OllamaClient::new(config.ollama.clone())?;
    let qdrant = QdrantClient::new(config.qdrant.clone())?;
    
    Ok(Self {
        ollama: Arc::new(ollama),
        qdrant: Arc::new(qdrant),
        config,
    })
}
```

---

### Step 8.2: Generate Text Representation

**Function**: `generate_text_representation(item)`

```rust
// embedding/text_representation.rs:39-61
pub fn generate_text_representation(item: &ParsedItem) -> TextRepresentation {
    let text = match &item.item_type {
        ItemType::Function => generate_function_text(item),
        ItemType::Struct => generate_struct_text(item),
        ItemType::Enum => generate_enum_text(item),
        ItemType::Trait => generate_trait_text(item),
        ItemType::Impl => generate_impl_text(item),
        // ... other types
    };
    
    TextRepresentation { text, item_type: item.item_type.as_str().to_string(), ... }
}
```

**Function Text Example** (text_representation.rs:161-191):
```
Input:
    ParsedItem {
        fqn: "router::payments::process_payment",
        name: "process_payment",
        visibility: Public,
        signature: "fn process_payment<T: PaymentMethod>(payment: T) -> Result<()>",
        generic_params: vec![GenericParam { name: "T", bounds: vec!["PaymentMethod"] }],
        doc_comment: "Process a payment through the configured gateway",
        body_source: "{ ... }",
    }

Output Text:
    pub fn process_payment<T: PaymentMethod>(payment: T) -> Result<()>
    Process a payment through the configured gateway
    
    Module: router::payments
    Crate: router
    Traits used: T: PaymentMethod
    Body preview:
    {
        let gateway = get_gateway()?;
        gateway.process(payment).await
    }
```

**Why**: This text format is optimized for semantic embedding - it includes:
- Signature (syntactic information)
- Documentation (semantic information)
- Context (module, crate)
- Type constraints (generic bounds)

---

### Step 8.3: Generate Embedding via Ollama

**Library**: `reqwest`  
**Endpoint**: `POST http://localhost:11434/api/embeddings`

```rust
// embedding/ollama_client.rs:121-160
pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
    let request = EmbeddingRequest {
        model: self.config.model.clone(),  // "qwen3-embedding:4b"
        prompt: text.to_string(),
    };
    
    let response = self.client
        .post(&format!("{}/api/embeddings", self.config.base_url))
        .json(&request)
        .send()
        .await?;
    
    let result: EmbeddingResponse = response.json().await?;
    Ok(result.embedding)  // Vec<f32> of 2560 dimensions
}
```

**Input**: Text representation (string)

**Output**: Embedding vector (2560 dimensions with qwen3-embedding:4b)

---

### Step 8.4: Store in Qdrant

**Library**: `reqwest`  
**Endpoint**: `PUT http://localhost:6333/collections/code_embeddings/points`

```rust
// embedding/qdrant_client.rs:182-243
pub async fn upsert_point(&self, collection: &str, point: Point) -> Result<()> {
    let request = UpsertRequest {
        points: vec![point],
    };
    
    self.client
        .put(&format!("{}/collections/{}/points", self.config.base_url, collection))
        .json(&request)
        .send()
        .await?;
    
    Ok(())
}
```

**Point Structure**:
```rust
pub struct Point {
    pub id: Uuid,                    // Unique ID
    pub vector: Vec<f32>,            // 2560-dim embedding
    pub payload: HashMap<String, PayloadValue>,  // Metadata
}
```

**Payload Example**:
```json
{
    "fqn": "router::payments::process_payment",
    "name": "process_payment",
    "item_type": "function",
    "visibility": "public",
    "signature": "fn process_payment<T: PaymentMethod>(payment: T) -> Result<()>",
    "doc_comment": "Process a payment through the configured gateway",
    "module_path": "router::payments",
    "crate_name": "router"
}
```

---

## 9. Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────────────────────────┐
│                                  INGESTION PIPELINE                                      │
└─────────────────────────────────────────────────────────────────────────────────────────┘

┌─────────────┐      ┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│   CLI Args  │─────▶│  Pipeline   │─────▶│   Shared    │─────▶│   Stage     │
│             │      │   Runner    │      │   State     │      │   Loop      │
└─────────────┘      └─────────────┘      └─────────────┘      └─────────────┘
                           │                                           │
                           ▼                                           ▼
                    ┌─────────────┐      ┌─────────────────────────────────────────────┐
                    │  Pipeline   │     │                STAGE 1: EXPAND               │
                    │   Context   │      ├─────────────────────────────────────────────┤
                    │  - id       │      │ 1. discover_crates()                        │
                    │  - config   │      │    └─ walkdir → Vec<PathBuf>                │
                    │  - state    │      │                                              │
                    └─────────────┘      │ 2. pre_patch_crates_for_features()          │
                                         │    └─ toml_edit → patched Cargo.toml        │
                                         │                                              │
                                         │ 3. run_cargo_expand()                        │
                                         │    └─ std::process::Command                 │
                                         │    └─ cargo expand --lib -p <crate>         │
                                         │    └─ Output: expanded source string        │
                                         │                                              │
                                         │ 4. Cache to /tmp/rustbrain-expand-cache/    │
                                         │                                              │
                                         │ OUTPUT TO STATE:                            │
                                         │   - state.expanded_sources: HashMap<Path,   │
                                         │     Path> (source -> cache file)            │
                                         │   - state.source_files: Vec<SourceFileInfo> │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌1 5
▶▼
┌───────────────────────└─────────▶
▶️ Continue to Stage 1

I'll continue the pattern
                                         │
                                         ▼
                                         ─v───────────────────────└──────────────────────────────────┘
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │                STAGE 2: PARSE               │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ FOR EACH FILE:                              │
                                         │                                              │
                                         │ 1. Read expanded source from cache          │
                                         │                                              │
                                         │ 2. Phase 1: tree-sitter skeleton            │
                                         │    └─ Parser::parse(source) → Tree          │
                                         │    └─ Tree.root_node().walk()               │
                                         │    └─ collect_items() → Vec<SkeletonItem>   │
                                         │       - item_type, name, start/end byte     │
                                         │                                              │
                                         │ 3. Phase 2: syn deep parse                  │
                                         │    FOR EACH SKELETON:                       │
                                         │    └─ extract source[skeleton.byte_range]   │
                                         │    └─ syn::parse_str::<Item>(source)        │
                                         │    └─ Item::Fn → parse_function()           │
                                         │    └─ Item::Struct → parse_struct()         │
                                         │    └─ Item::Impl → parse_impl()             │
                                         │    └─ Output: ParsedItem                    │
                                         │                                              │
                                         │ 4. ON SYN FAILURE: fallback to tree-sitter  │
                                         │    └─ create_partial_item() from skeleton   │
                                         │                                              │
                                         │ OUTPUT TO DATABASE:                         │
                                         │   - INSERT INTO extracted_items             │
                                         │                                              │
                                         │ OUTPUT TO STATE:                            │
                                         │   - state.parsed_items: HashMap<Path,       │
                                         │     Vec<ParsedItemInfo>>                    │
                                         │   - state.counts.items_parsed               │
                                         │                                              │
                                         │ ⚠️ BUG: Clears state.expanded_sources       │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │             STAGE 3: TYPECHECK              │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ INPUT FROM STATE:                           │
                                         │   - state.expanded_sources ← EMPTY! (BUG)   │
                                         │                                              │
                                         │ IF NOT EMPTY (should be):                   │
                                         │                                              │
                                         │ FOR EACH EXPANDED SOURCE:                   │
                                         │   1. syn::parse_file(expanded_source)       │
                                         │   2. Visit all ExprCall nodes               │
                                         │   3. Extract turbofish: func::<Type>()      │
                                         │   4. Record call site:                      │
                                         │      - caller_fqn                           │
                                         │      - callee_fqn                           │
                                         │      - type_arguments                       │
                                         │      - is_monomorphized                     │
                                         │   5. Analyze impl blocks for quality        │
                                         │                                              │
                                         │ OUTPUT TO DATABASE:                         │
                                         │   - INSERT INTO call_sites                  │
                                         │   - INSERT INTO trait_implementations       │
                                         │                                              │
                                         │ CURRENT STATUS: ⚠️ ALWAYS SKIPPED (BUG)     │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │              STAGE 4: EXTRACT               │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ 1. Store source files in database           │
                                         │    └─ INSERT INTO source_files              │
                                         │       (original_source, expanded_source)    │
                                         │                                              │
                                         │ 2. Link items to source files               │
                                         │    └─ UPDATE extracted_items                │
                                         │       SET source_file_id = ...              │
                                         │                                              │
                                         │ OUTPUT TO DATABASE:                         │
                                         │   - source_files table populated            │
                                         │   - extracted_items.source_file_id set      │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │               STAGE 5: GRAPH                │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ INPUT: state.parsed_items                   │
                                         │                                              │
                                         │ 1. Create nodes                             │
                                         │    └─ FOR EACH ITEM:                        │
                                         │       - NodeData { id, fqn, name, type }    │
                                         │    └─ graph_builder.create_nodes_batch()    │
                                         │    └─ Cypher: MERGE (n:Function {fqn})      │
                                         │                                              │
                                         │ 2. Create relationships                     │
                                         │    └─ CONTAINS: Crate → Module → Item       │
                                         │    └─ IMPLEMENTS: Impl → Trait              │
                                         │    └─ FOR: Impl → Type (self type)          │
                                         │    └─ CALLS: Function → Function            │
                                         │    └─ USES_TYPE: Item → Type                │
                                         │                                              │
                                         │ OUTPUT TO NEO4J:                            │
                                         │   - Nodes: Function, Struct, Enum, etc.     │
                                         │   - Relationships: CONTAINS, IMPLEMENTS...  │
                                         │                                              │
                                         │ OUTPUT TO STATE:                            │
                                         │   - state.graph_nodes: HashMap<FQN, ID>     │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │               STAGE 6: EMBED                │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ INPUT: state.parsed_items                   │
                                         │                                              │
                                         │ FOR EACH ITEM:                              │
                                         │                                              │
                                         │ 1. Generate text representation             │
                                         │    └─ generate_text_representation(item)    │
                                         │    └─ Format: "pub fn name<T>(...) ..."     │
                                         │                                              │
                                         │ 2. Generate embedding via Ollama            │
                                         │    └─ POST /api/embeddings                  │
                                         │    └─ Model: qwen3-embedding:4b             │
                                         │    └─ Output: Vec<f32> (2560 dims)          │
                                         │                                              │
                                         │ 3. Store in Qdrant                          │
                                         │    └─ PUT /collections/code_embeddings/pts  │
                                         │    └─ Point { id, vector, payload }         │
                                         │                                              │
                                         │ OUTPUT TO QDRANT:                           │
                                         │   - code_embeddings collection              │
                                         │   - doc_embeddings collection (doc chunks)  │
                                         └─────────────────────────────────────────────┘
                                                              │
                                                              ▼
                                         ┌─────────────────────────────────────────────┐
                                         │                 COMPLETE                     │
                                         ├─────────────────────────────────────────────┤
                                         │                                              │
                                         │ STORAGE SUMMARY:                            │
                                         │   ┌─────────────┐  ┌─────────────┐          │
                                         │   │  PostgreSQL │  │    Neo4j    │          │
                                         │   ├─────────────┤  ├─────────────┤          │
                                         │   │source_files │  │Function     │          │
                                         │   │extracted_   │  │Struct nodes │          │
                                         │   │  items      │  │CALLS edges  │          │
                                         │   │call_sites ❌│  │IMPLEMENTS   │          │
                                         │   │trait_impls ❌│ │edges        │          │
                                         │   └─────────────┘  └─────────────┘          │
                                         │                                              │
                                         │   ┌─────────────┐                           │
                                         │   │   Qdrant    │                           │
                                         │   ├─────────────┤                           │
                                         │   │code_        │                           │
                                         │   │  embeddings │                           │
                                         │   │doc_         │                           │
                                         │   │  embeddings │                           │
                                         │   └─────────────┘                           │
                                         │                                              │
                                         │ ❌ = Empty due to TypecheckStage bug        │
                                         └─────────────────────────────────────────────┘
```

---

## Libraries Summary

| Stage | Library | Purpose |
|-------|---------|---------|
| CLI | `clap` | Argument parsing |
| Logging | `tracing`, `tracing-subscriber` | Structured logging |
| Expand | `walkdir` | Directory traversal |
| Expand | `toml_edit` | Cargo.toml manipulation |
| Expand | `std::process::Command` | Running `cargo expand` |
| Parse | `tree-sitter`, `tree-sitter-rust` | Fast skeleton parsing |
| Parse | `syn` | Deep semantic parsing |
| Parse | `quote` | Token conversion (used internally by syn) |
| Typecheck | `syn` | AST analysis |
| All | `sqlx` | PostgreSQL queries |
| Graph | `neo4rs` | Neo4j driver |
| Embed | `reqwest` | HTTP client for Ollama/Qdrant |
| All | `serde`, `serde_json` | Serialization |
| All | `tokio` | Async runtime |
| All | `anyhow` | Error handling |

---

## Key Functions Reference

| Function | File | Purpose |
|----------|------|---------|
| `PipelineRunner::run()` | runner.rs:118 | Main execution loop |
| `ExpandStage::run()` | stages.rs:1321 | Expand macros |
| `ExpandStage::run_cargo_expand()` | stages.rs:846 | Execute cargo expand |
| `ParseStage::run()` | stages.rs:1505 | Parse source files |
| `DualParser::parse()` | parsers/mod.rs:146 | Dual parsing strategy |
| `TreeSitterParser::extract_skeletons()` | tree_sitter_parser.rs:33 | Fast skeleton extraction |
| `SynParser::parse_item()` | syn_parser.rs:44 | Deep parsing |
| `SynParser::parse_impl()` | syn_parser.rs:265 | Impl block parsing |
| `TypecheckStage::run()` | stages.rs:1895 | Type analysis (broken) |
| `ExtractStage::run()` | stages.rs:2174 | Store to Postgres |
| `GraphStage::run()` | stages.rs:2539 | Build Neo4j graph |
| `EmbedStage::run()` | stages.rs:3075+ | Generate embeddings |
| `generate_text_representation()` | text_representation.rs:39 | Text for embedding |
| `OllamaClient::embed()` | ollama_client.rs:121 | Generate embedding |
| `QdrantClient::upsert_point()` | qdrant_client.rs:182 | Store vector |
