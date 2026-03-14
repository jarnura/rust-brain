//! Pipeline stage definitions and implementations
//!
//! Each stage implements the `PipelineStage` trait and processes
//! data from the shared `PipelineContext`.

use crate::parsers::{DualParser, ParsedItem, ItemType, Visibility};
use crate::pipeline::{PipelineContext, ParsedItemInfo, SourceFileInfo};
use crate::typecheck::TypeResolutionService;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use walkdir::WalkDir;

/// Result of running a pipeline stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageResult {
    /// Stage name
    pub name: String,
    
    /// Execution status
    pub status: StageStatus,
    
    /// Items processed
    pub items_processed: usize,
    
    /// Items failed
    pub items_failed: usize,
    
    /// Duration in milliseconds
    pub duration_ms: u64,
    
    /// Error message if failed
    pub error: Option<String>,
    
    /// Timestamp
    pub timestamp: chrono::DateTime<Utc>,
}

impl StageResult {
    pub fn success(name: &str, processed: usize, failed: usize, duration: Duration) -> Self {
        Self {
            name: name.to_string(),
            status: StageStatus::Success,
            items_processed: processed,
            items_failed: failed,
            duration_ms: duration.as_millis() as u64,
            error: None,
            timestamp: Utc::now(),
        }
    }
    
    pub fn partial(name: &str, processed: usize, failed: usize, duration: Duration, error: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status: StageStatus::Partial,
            items_processed: processed,
            items_failed: failed,
            duration_ms: duration.as_millis() as u64,
            error: Some(error.into()),
            timestamp: Utc::now(),
        }
    }
    
    pub fn failed(name: &str, error: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status: StageStatus::Failed,
            items_processed: 0,
            items_failed: 0,
            duration_ms: 0,
            error: Some(error.into()),
            timestamp: Utc::now(),
        }
    }
    
    pub fn skipped(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: StageStatus::Skipped,
            items_processed: 0,
            items_failed: 0,
            duration_ms: 0,
            error: None,
            timestamp: Utc::now(),
        }
    }
}

/// Status of a stage execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    Success,
    Partial,
    Failed,
    Skipped,
}

impl std::fmt::Display for StageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Partial => write!(f, "partial"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

/// Error from a stage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageError {
    pub stage: String,
    pub message: String,
    pub context: Option<String>,
    pub is_fatal: bool,
}

impl StageError {
    pub fn new(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
            context: None,
            is_fatal: false,
        }
    }
    
    pub fn fatal(stage: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            message: message.into(),
            context: None,
            is_fatal: true,
        }
    }
    
    pub fn with_context(mut self, ctx: impl Into<String>) -> Self {
        self.context = Some(ctx.into());
        self
    }
}

/// Trait for pipeline stages
#[async_trait::async_trait]
pub trait PipelineStage: Send + Sync {
    /// Stage name for logging and tracking
    fn name(&self) -> &str;
    
    /// Run the stage
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult>;
    
    /// Whether this stage can be skipped
    fn can_skip(&self, _ctx: &PipelineContext) -> bool {
        false
    }
}

// =============================================================================
// EXPAND STAGE
// =============================================================================

/// Stage 1: Macro expansion via cargo expand
pub struct ExpandStage {
    parser: DualParser,
}

impl ExpandStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            parser: DualParser::new()?,
        })
    }
    
    fn get_git_hash(&self, repo_path: &Path) -> Option<String> {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo_path)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    }
    
    fn find_source_files(&self, crate_path: &Path) -> Vec<PathBuf> {
        let src_path = crate_path.join("src");
        
        if !src_path.exists() {
            return Vec::new();
        }
        
        WalkDir::new(&src_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "rs").unwrap_or(false))
            .map(|e| e.path().to_path_buf())
            .collect()
    }
    
    fn expand_library(&self, crate_path: &Path) -> Result<String> {
        debug!("Expanding library for {:?}", crate_path);
        
        let output = Command::new("cargo")
            .args(["expand", "--lib"])
            .current_dir(crate_path)
            .output()
            .context("Failed to run cargo expand --lib")?;
        
        if output.status.success() {
            String::from_utf8(output.stdout)
                .context("Expanded output is not valid UTF-8")
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("cargo expand --lib failed: {}", stderr)
        }
    }
    
    async fn discover_crates(&self, workspace_path: &Path) -> Result<Vec<PathBuf>> {
        let cargo_toml = workspace_path.join("Cargo.toml");
        
        if !cargo_toml.exists() {
            anyhow::bail!("No Cargo.toml found at {:?}", workspace_path);
        }
        
        // Check if it's a workspace
        let content = std::fs::read_to_string(&cargo_toml)?;
        if content.contains("[workspace]") {
            // Find workspace members
            let mut crates = Vec::new();
            
            // Simple glob-based member detection
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
            
            Ok(crates)
        } else {
            // Single crate
            Ok(vec![workspace_path.to_path_buf()])
        }
    }
}

#[async_trait::async_trait]
impl PipelineStage for ExpandStage {
    fn name(&self) -> &str {
        "expand"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        let crate_path = &ctx.config.crate_path;
        
        info!("Starting expand stage for {:?}", crate_path);
        
        if ctx.config.dry_run {
            info!("Dry run - skipping expansion");
            return Ok(StageResult::skipped("expand"));
        }
        
        // Discover crates
        let crates = match self.discover_crates(crate_path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(StageResult::failed("expand", format!("Failed to discover crates: {}", e)));
            }
        };
        
        let mut state = ctx.state.write().await;
        let mut expanded_count = 0;
        let mut failed_count = 0;
        
        for crate_path in &crates {
            // Get git hash
            let git_hash = self.get_git_hash(crate_path);
            
            // Get crate name from Cargo.toml
            let crate_name = crate_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            
            // Find source files
            let source_files = self.find_source_files(crate_path);
            
            // Expand library
            let expanded = match self.expand_library(crate_path) {
                Ok(exp) => {
                    expanded_count += 1;
                    Some(exp)
                }
                Err(e) => {
                    warn!("Failed to expand {:?}: {}", crate_path, e);
                    state.errors.push(StageError::new("expand", e.to_string()));
                    failed_count += 1;
                    None
                }
            };
            
            // Store source files and expanded code
            for file_path in source_files {
                if let Ok(source) = std::fs::read_to_string(&file_path) {
                    let module_path = compute_module_path(crate_path, &file_path, &crate_name);
                    
                    state.source_files.push(SourceFileInfo {
                        path: file_path.clone(),
                        crate_name: crate_name.clone(),
                        module_path,
                        original_source: source,
                        git_hash: git_hash.clone(),
                    });
                    
                    if let Some(ref expanded_source) = expanded {
                        state.expanded_sources.insert(file_path, expanded_source.clone());
                    }
                }
            }
        }
        
        state.counts.files_expanded = expanded_count;
        
        let duration = start.elapsed();
        
        if failed_count > 0 && expanded_count == 0 {
            Ok(StageResult::failed("expand", "All expansion attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("expand", expanded_count, failed_count, duration, 
                format!("{} crates expanded, {} failed", expanded_count, failed_count)))
        } else {
            Ok(StageResult::success("expand", expanded_count, 0, duration))
        }
    }
}

// =============================================================================
// PARSE STAGE
// =============================================================================

/// Stage 2: Dual parsing (tree-sitter + syn) with derive macro detection
pub struct ParseStage {
    parser: DualParser,
    derive_detector: crate::derive_detector::DeriveDetector,
}

impl ParseStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            parser: DualParser::new()?,
            derive_detector: crate::derive_detector::DeriveDetector::new(),
        })
    }
}

#[async_trait::async_trait]
impl PipelineStage for ParseStage {
    fn name(&self) -> &str {
        "parse"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        
        info!("Starting parse stage");
        
        let state = ctx.state.read().await;
        let source_files = state.source_files.clone();
        let expanded_sources = state.expanded_sources.clone();
        drop(state);
        
        if source_files.is_empty() {
            return Ok(StageResult::skipped("parse"));
        }
        
        let mut state = ctx.state.write().await;
        let mut parsed_count = 0;
        let mut items_count = 0;
        let mut failed_count = 0;
        let mut derive_generated_count = 0;
        
        for file_info in &source_files {
            // Use expanded source if available, otherwise original
            let (source_to_parse, has_expanded) = expanded_sources
                .get(&file_info.path)
                .map(|s| (s.as_str(), true))
                .unwrap_or((&file_info.original_source, false));
            
            match self.parser.parse(source_to_parse, &file_info.module_path) {
                Ok(result) => {
                    // Detect derive-generated impl blocks if we have expanded source
                    let generated_by_map = if has_expanded {
                        match self.derive_detector.detect(
                            &file_info.original_source,
                            source_to_parse,
                            &file_info.module_path,
                        ) {
                            Ok(detection) => detection.generated_by,
                            Err(e) => {
                                warn!("Derive detection failed for {:?}: {}", file_info.path, e);
                                HashMap::new()
                            }
                        }
                    } else {
                        HashMap::new()
                    };
                    
                    // Convert ParsedItem to ParsedItemInfo with generated_by annotation
                    let items: Vec<ParsedItemInfo> = result.items
                        .iter()
                        .map(|item| {
                            // Check if this impl was generated by a derive macro
                            let generated_by = if item.item_type == ItemType::Impl {
                                // Try to find the derive source
                                self.find_derive_source(item, &generated_by_map)
                                    .or_else(|| item.generated_by.clone())
                            } else {
                                item.generated_by.clone()
                            };
                            
                            if generated_by.is_some() {
                                derive_generated_count += 1;
                            }
                            
                            ParsedItemInfo {
                                fqn: item.fqn.clone(),
                                item_type: item.item_type.to_string(),
                                name: item.name.clone(),
                                visibility: item.visibility.as_str().to_string(),
                                signature: item.signature.clone(),
                                generic_params: item.generic_params.clone(),
                                where_clauses: item.where_clauses.clone(),
                                attributes: item.attributes.clone(),
                                doc_comment: item.doc_comment.clone(),
                                start_line: item.start_line,
                                end_line: item.end_line,
                                body_source: item.body_source.clone(),
                                generated_by,
                            }
                        })
                        .collect();
                    
                    items_count += items.len();
                    state.parsed_items.insert(file_info.path.clone(), items);
                    parsed_count += 1;
                    
                    // Record parse errors
                    for err in result.errors {
                        state.errors.push(StageError::new("parse", err.message)
                            .with_context(format!("{}:{}", file_info.path.display(), err.line.unwrap_or(0))));
                    }
                }
                Err(e) => {
                    warn!("Failed to parse {:?}: {}", file_info.path, e);
                    state.errors.push(StageError::new("parse", e.to_string())
                        .with_context(file_info.path.display().to_string()));
                    failed_count += 1;
                }
            }
        }
        
        state.counts.files_parsed = parsed_count;
        state.counts.items_parsed = items_count;
        
        let duration = start.elapsed();
        
        info!(
            "Parse stage completed: {} files, {} items ({} derive-generated)",
            parsed_count, items_count, derive_generated_count
        );
        
        if failed_count > 0 && parsed_count == 0 {
            Ok(StageResult::failed("parse", "All parsing attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("parse", parsed_count, failed_count, duration,
                format!("{} files parsed, {} failed, {} items ({} derive-generated)", 
                    parsed_count, failed_count, items_count, derive_generated_count)))
        } else {
            Ok(StageResult::success("parse", items_count, 0, duration))
        }
    }
}

impl ParseStage {
    /// Find the derive source for an impl block
    fn find_derive_source(
        &self,
        item: &ParsedItem,
        generated_by_map: &HashMap<String, String>,
    ) -> Option<String> {
        // Extract trait name from attributes (format: impl_for=TraitName)
        for attr in &item.attributes {
            if attr.starts_with("impl_for=") {
                let trait_name = &attr[9..];
                // Extract self type from name (format: "TraitName_TypeName")
                if let Some(underscore_pos) = item.name.find('_') {
                    let self_type = &item.name[underscore_pos + 1..];
                    let key = format!("{} for {}", trait_name, self_type);
                    return generated_by_map.get(&key).cloned();
                }
            }
        }
        None
    }
}

// =============================================================================
// TYPECHECK STAGE
// =============================================================================

/// Stage 3: Type resolution and inference
/// 
/// This stage:
/// - Analyzes expanded source code for type information
/// - Extracts trait implementations (impl Trait for Type)
/// - Extracts call sites with concrete type arguments
/// - Stores results in the database for later queries
pub struct TypecheckStage;

impl TypecheckStage {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl PipelineStage for TypecheckStage {
    fn name(&self) -> &str {
        "typecheck"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        
        info!("Starting typecheck stage");
        
        if ctx.config.dry_run {
            info!("Dry run - skipping typecheck");
            return Ok(StageResult::skipped("typecheck"));
        }
        
        let state = ctx.state.read().await;
        let source_files = state.source_files.clone();
        let expanded_sources = state.expanded_sources.clone();
        let parsed_items = state.parsed_items.clone();
        drop(state);
        
        if expanded_sources.is_empty() {
            info!("No expanded sources to typecheck");
            return Ok(StageResult::skipped("typecheck"));
        }
        
        // Connect to database for TypeResolutionService
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&ctx.config.database_url)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                return Ok(StageResult::failed("typecheck", 
                    format!("Database connection failed: {}", e)));
            }
        };
        
        let type_resolution_service = TypeResolutionService::new(pool);
        
        let mut typechecked_count = 0;
        let mut trait_impls_count = 0;
        let mut call_sites_count = 0;
        let mut failed_count = 0;
        
        let mut state = ctx.state.write().await;
        
        // Build a mapping from file path to caller FQNs
        // (functions/methods defined in each file that could be callers)
        let mut file_to_caller_fqns: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for (path, items) in &parsed_items {
            let fqns: Vec<String> = items
                .iter()
                .filter(|i| i.item_type == "function" || i.item_type == "impl")
                .map(|i| i.fqn.clone())
                .collect();
            file_to_caller_fqns.insert(path.clone(), fqns);
        }
        
        // Process each source file with expanded source
        for file_info in &source_files {
            let expanded_source = match expanded_sources.get(&file_info.path) {
                Some(src) => src,
                None => continue, // Skip files without expanded source
            };
            
            // Get caller FQNs for this file
            let caller_fqns = file_to_caller_fqns
                .get(&file_info.path)
                .cloned()
                .unwrap_or_default();
            
            // Run type resolution analysis
            match type_resolution_service.analyze_expanded_source(
                &file_info.crate_name,
                &file_info.module_path,
                &file_info.path.to_string_lossy(),
                expanded_source,
                &caller_fqns,
            ).await {
                Ok(result) => {
                    typechecked_count += 1;
                    trait_impls_count += result.trait_impls.len();
                    call_sites_count += result.call_sites.len();
                    
                    // Log any resolution errors
                    for err in result.errors {
                        debug!("Type resolution warning for {:?}: {}", file_info.path, err);
                    }
                }
                Err(e) => {
                    warn!("Type resolution failed for {:?}: {}", file_info.path, e);
                    state.errors.push(StageError::new("typecheck", e.to_string())
                        .with_context(file_info.path.display().to_string()));
                    failed_count += 1;
                }
            }
        }
        
        state.counts.items_typechecked = typechecked_count;
        
        let duration = start.elapsed();
        
        info!(
            "Typecheck stage completed: {} files analyzed, {} trait impls, {} call sites",
            typechecked_count, trait_impls_count, call_sites_count
        );
        
        if failed_count > 0 && typechecked_count == 0 {
            Ok(StageResult::failed("typecheck", "All type checks failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("typecheck", typechecked_count, failed_count, duration,
                format!("{} files typechecked, {} failed, {} trait impls, {} call sites", 
                    typechecked_count, failed_count, trait_impls_count, call_sites_count)))
        } else {
            Ok(StageResult::success("typecheck", typechecked_count, 0, duration))
        }
    }
}

// =============================================================================
// EXTRACT STAGE
// =============================================================================

/// Stage 4: Extract items to Postgres
pub struct ExtractStage {
    pool: Option<PgPool>,
}

impl ExtractStage {
    pub fn new() -> Self {
        Self { pool: None }
    }
    
    pub async fn connect(&mut self, database_url: &str) -> Result<()> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("Failed to connect to database")?;
        
        self.pool = Some(pool);
        Ok(())
    }
    
    /// Store a source file in the database and return its ID
    async fn store_source_file(&self, file_info: &SourceFileInfo, expanded_source: Option<&str>) -> Result<Uuid> {
        let pool = self.pool.as_ref()
            .ok_or_else(|| anyhow!("Database not connected"))?;
        
        let id = Uuid::new_v4();
        
        sqlx::query(
            r#"
            INSERT INTO source_files 
                (id, crate_name, module_path, file_path, original_source, expanded_source, git_hash)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (crate_name, module_path, file_path) DO UPDATE SET
                original_source = EXCLUDED.original_source,
                expanded_source = EXCLUDED.expanded_source,
                git_hash = EXCLUDED.git_hash,
                last_indexed_at = NOW(),
                updated_at = NOW()
            RETURNING id
            "#
        )
        .bind(id)
        .bind(&file_info.crate_name)
        .bind(&file_info.module_path)
        .bind(file_info.path.to_string_lossy().to_string())
        .bind(&file_info.original_source)
        .bind(expanded_source)
        .bind(&file_info.git_hash)
        .fetch_one(pool)
        .await?;
        
        Ok(id)
    }
    
    async fn extract_item(&self, item: &ParsedItemInfo, source_file_id: Option<Uuid>) -> Result<Uuid> {
        let pool = self.pool.as_ref()
            .ok_or_else(|| anyhow!("Database not connected"))?;
        
        let id = Uuid::new_v4();
        
        // Serialize generic_params, where_clauses, and attributes to JSON
        let generic_params_json = serde_json::to_value(&item.generic_params)
            .unwrap_or(serde_json::json!([]));
        let where_clauses_json = serde_json::to_value(&item.where_clauses)
            .unwrap_or(serde_json::json!([]));
        let attributes_json = serde_json::to_value(&item.attributes)
            .unwrap_or(serde_json::json!([]));
        
        sqlx::query(
            r#"
            INSERT INTO extracted_items 
                (id, source_file_id, item_type, fqn, name, visibility, signature, 
                 doc_comment, start_line, end_line, body_source, 
                 generic_params, where_clauses, attributes, generated_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (fqn) DO UPDATE SET
                signature = EXCLUDED.signature,
                doc_comment = EXCLUDED.doc_comment,
                start_line = EXCLUDED.start_line,
                end_line = EXCLUDED.end_line,
                body_source = EXCLUDED.body_source,
                generic_params = EXCLUDED.generic_params,
                where_clauses = EXCLUDED.where_clauses,
                attributes = EXCLUDED.attributes,
                visibility = EXCLUDED.visibility,
                source_file_id = EXCLUDED.source_file_id,
                generated_by = EXCLUDED.generated_by,
                updated_at = NOW()
            RETURNING id
            "#
        )
        .bind(id)
        .bind(source_file_id)
        .bind(&item.item_type)
        .bind(&item.fqn)
        .bind(&item.name)
        .bind(&item.visibility)
        .bind(&item.signature)
        .bind(&item.doc_comment)
        .bind(item.start_line as i32)
        .bind(item.end_line as i32)
        .bind(&item.body_source)
        .bind(&generic_params_json)
        .bind(&where_clauses_json)
        .bind(&attributes_json)
        .bind(&item.generated_by)
        .fetch_one(pool)
        .await?;
        
        Ok(id)
    }
}

#[async_trait::async_trait]
impl PipelineStage for ExtractStage {
    fn name(&self) -> &str {
        "extract"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        
        info!("Starting extract stage");
        
        if ctx.config.dry_run {
            info!("Dry run - skipping extraction");
            return Ok(StageResult::skipped("extract"));
        }
        
        let state = ctx.state.read().await;
        let parsed_items = state.parsed_items.clone();
        let source_files = state.source_files.clone();
        let expanded_sources = state.expanded_sources.clone();
        drop(state);
        
        if parsed_items.is_empty() {
            return Ok(StageResult::skipped("extract"));
        }
        
        // Connect to database
        let mut stage = ExtractStage::new();
        if let Err(e) = stage.connect(&ctx.config.database_url).await {
            return Ok(StageResult::failed("extract", format!("Database connection failed: {}", e)));
        }
        
        let mut state = ctx.state.write().await;
        let mut extracted_count = 0;
        let mut failed_count = 0;
        
        // Step 1: Store source files in database and build path -> ID mapping
        let mut source_file_ids: HashMap<PathBuf, Uuid> = HashMap::new();
        
        for file_info in &source_files {
            let expanded = expanded_sources.get(&file_info.path);
            match stage.store_source_file(file_info, expanded.map(|s| s.as_str())).await {
                Ok(id) => {
                    source_file_ids.insert(file_info.path.clone(), id);
                    debug!("Stored source file {:?} with ID {}", file_info.path, id);
                }
                Err(e) => {
                    warn!("Failed to store source file {:?}: {}", file_info.path, e);
                    state.errors.push(StageError::new("extract", e.to_string())
                        .with_context(file_info.path.display().to_string()));
                }
            }
        }
        
        // Step 2: Extract parsed items with source_file_id
        for (path, items) in &parsed_items {
            let source_file_id = source_file_ids.get(path).copied();
            
            for item in items {
                match stage.extract_item(item, source_file_id).await {
                    Ok(id) => {
                        state.extracted_items.insert(item.fqn.clone(), id);
                        extracted_count += 1;
                    }
                    Err(e) => {
                        warn!("Failed to extract item {}: {}", item.fqn, e);
                        state.errors.push(StageError::new("extract", e.to_string())
                            .with_context(item.fqn.clone()));
                        failed_count += 1;
                    }
                }
            }
        }
        
        state.counts.items_extracted = extracted_count;
        
        let duration = start.elapsed();
        
        if failed_count > 0 && extracted_count == 0 {
            Ok(StageResult::failed("extract", "All extraction attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("extract", extracted_count, failed_count, duration,
                format!("{} items extracted, {} failed", extracted_count, failed_count)))
        } else {
            Ok(StageResult::success("extract", extracted_count, 0, duration))
        }
    }
}

// =============================================================================
// GRAPH STAGE
// =============================================================================

use crate::graph::{GraphBuilder, GraphConfig, NodeData, NodeType, PropertyValue, RelationshipBuilder, RelationshipData};

/// Stage 5: Build Neo4j relationship graph
pub struct GraphStage {
    neo4j_url: Option<String>,
}

impl GraphStage {
    pub fn new() -> Self {
        Self { neo4j_url: None }
    }
    
    /// Convert a ParsedItemInfo to NodeData for Neo4j
    fn item_to_node(item: &ParsedItemInfo) -> NodeData {
        let node_type = match item.item_type.as_str() {
            "function" => NodeType::Function,
            "struct" => NodeType::Struct,
            "enum" => NodeType::Enum,
            "trait" => NodeType::Trait,
            "impl" => NodeType::Impl,
            "type_alias" => NodeType::TypeAlias,
            "const" => NodeType::Const,
            "static" => NodeType::Static,
            "macro" => NodeType::Macro,
            "module" => NodeType::Module,
            _ => NodeType::Type,
        };
        
        let mut properties = HashMap::new();
        if !item.signature.is_empty() {
            properties.insert("signature".to_string(), PropertyValue::from(item.signature.as_str()));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(item.start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(item.end_line));
        
        // Add visibility property
        properties.insert("visibility".to_string(), PropertyValue::from(item.visibility.as_str()));
        
        // Add generated_by property if present
        if let Some(ref generated_by) = item.generated_by {
            properties.insert("generated_by".to_string(), PropertyValue::from(generated_by.as_str()));
        }
        
        // Add function-specific properties
        if item.item_type == "function" {
            let sig_lower = item.signature.to_lowercase();
            properties.insert("is_async".to_string(), PropertyValue::from(sig_lower.contains("async")));
            properties.insert("is_unsafe".to_string(), PropertyValue::from(sig_lower.contains("unsafe")));
            properties.insert("is_generic".to_string(), PropertyValue::from(!item.generic_params.is_empty()));
        }
        
        // Add generic_params as JSON if present
        if !item.generic_params.is_empty() {
            if let Ok(json) = serde_json::to_string(&item.generic_params) {
                properties.insert("generic_params".to_string(), PropertyValue::from(json));
            }
        }
        
        // Add doc_comment if present
        if !item.doc_comment.is_empty() {
            properties.insert("doc_comment".to_string(), PropertyValue::from(item.doc_comment.as_str()));
        }
        
        NodeData {
            id: item.fqn.clone(),
            fqn: item.fqn.clone(),
            name: item.name.clone(),
            node_type,
            properties,
        }
    }
    
    /// Create a Crate node
    fn create_crate_node(crate_name: &str) -> NodeData {
        let mut properties = HashMap::new();
        properties.insert("name".to_string(), PropertyValue::from(crate_name));
        
        NodeData {
            id: crate_name.to_string(),
            fqn: crate_name.to_string(),
            name: crate_name.to_string(),
            node_type: NodeType::Crate,
            properties,
        }
    }
    
    /// Extract the parent module FQN from an item FQN
    fn get_parent_fqn(fqn: &str) -> Option<String> {
        fqn.rfind("::").map(|pos| fqn[..pos].to_string())
    }
    
    /// Extract trait name from impl attributes (impl_for=TraitName)
    fn extract_trait_from_impl(attributes: &[String]) -> Option<String> {
        for attr in attributes {
            if attr.starts_with("impl_for=") {
                return Some(attr[9..].to_string());
            }
        }
        None
    }
    
    /// Extract the self type from an impl FQN
    /// Format: "module::TraitName_TypeName" for trait impls, or "module::TypeName" for inherent impls
    fn extract_impl_self_type(_fqn: &str, name: &str) -> Option<String> {
        // For inherent impls, the name is just the type name
        // For trait impls like "TraitName_TypeName", extract TypeName
        if let Some(underscore_pos) = name.find('_') {
            Some(name[underscore_pos + 1..].to_string())
        } else {
            Some(name.to_string())
        }
    }
    
    /// Extract fields from struct body source
    fn extract_struct_fields(body_source: &str, _struct_fqn: &str) -> Vec<(String, String, usize)> {
        let mut fields = Vec::new();
        
        // Simple regex-like extraction for struct fields
        // Pattern: field_name: Type,
        for (pos, line) in body_source.lines().enumerate() {
            let line = line.trim();
            // Skip comments, attributes, and non-field lines
            if line.starts_with("//") || line.starts_with("#") || line.starts_with("pub ") || line.starts_with("}") {
                continue;
            }
            
            // Try to parse field: Type pattern
            if let Some(colon_pos) = line.find(':') {
                let field_name = line[..colon_pos].trim().to_string();
                let rest = &line[colon_pos + 1..];
                // Get type until comma or end
                let type_str = rest.split(',').next().unwrap_or("").trim();
                if !field_name.is_empty() && !type_str.is_empty() {
                    fields.push((field_name, type_str.to_string(), pos));
                }
            }
        }
        
        fields
    }
    
    /// Extract variants from enum body source
    fn extract_enum_variants(body_source: &str, _enum_fqn: &str) -> Vec<(String, Option<String>, usize)> {
        let mut variants = Vec::new();
        
        for (pos, line) in body_source.lines().enumerate() {
            let line = line.trim();
            // Skip comments, attributes, braces
            if line.starts_with("//") || line.starts_with("#") || line.starts_with("{") || line.starts_with("}") {
                continue;
            }
            
            // Try to parse variant pattern: VariantName, or VariantName(Type), or VariantName { fields }
            if let Some(variant_name) = line.split(',').next() {
                let variant_name = variant_name.trim();
                if variant_name.is_empty() || variant_name.starts_with("//") {
                    continue;
                }
                
                // Check if variant has data
                let has_data = variant_name.contains('(') || variant_name.contains('{');
                let name = variant_name.split('(').next().unwrap_or(variant_name)
                    .split('{').next().unwrap_or(variant_name)
                    .trim();
                
                if !name.is_empty() && !name.starts_with("//") {
                    // Extract type if tuple variant
                    let variant_type = if has_data {
                        Some(name.to_string())
                    } else {
                        None
                    };
                    variants.push((name.to_string(), variant_type, pos));
                }
            }
        }
        
        variants
    }
    
    /// Load items from the database when parsed_items is not available in state
    async fn load_items_from_database(&self, database_url: &str) -> Result<Vec<ParsedItemInfo>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .context("Failed to connect to database for loading items")?;
        
        let rows = sqlx::query(
            r#"
            SELECT item_type, fqn, name, visibility, signature, doc_comment,
                   start_line, end_line, body_source, generic_params, where_clauses, attributes, generated_by
            FROM extracted_items
            ORDER BY fqn
            "#
        )
        .fetch_all(&pool)
        .await
        .context("Failed to query extracted_items")?;
        
        let items: Vec<ParsedItemInfo> = rows
            .into_iter()
            .map(|row| {
                let generic_params_json: serde_json::Value = row.get("generic_params");
                let where_clauses_json: serde_json::Value = row.get("where_clauses");
                let attributes_json: serde_json::Value = row.get("attributes");
                
                ParsedItemInfo {
                    fqn: row.get("fqn"),
                    item_type: row.get("item_type"),
                    name: row.get("name"),
                    visibility: row.get("visibility"),
                    signature: row.get("signature"),
                    generic_params: serde_json::from_value(generic_params_json).unwrap_or_default(),
                    where_clauses: serde_json::from_value(where_clauses_json).unwrap_or_default(),
                    attributes: serde_json::from_value(attributes_json).unwrap_or_default(),
                    doc_comment: row.get("doc_comment"),
                    start_line: row.get::<i32, _>("start_line") as usize,
                    end_line: row.get::<i32, _>("end_line") as usize,
                    body_source: row.get("body_source"),
                    generated_by: row.get("generated_by"),
                }
            })
            .collect();
        
        Ok(items)
    }
}

#[async_trait::async_trait]
impl PipelineStage for GraphStage {
    fn name(&self) -> &str {
        "graph"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        
        info!("Starting graph stage");
        
        if ctx.config.dry_run {
            info!("Dry run - skipping graph building");
            return Ok(StageResult::skipped("graph"));
        }
        
        // Check if Neo4j is configured
        let neo4j_url = match &ctx.config.neo4j_url {
            Some(url) => url.clone(),
            None => {
                info!("Neo4j not configured - skipping graph stage");
                return Ok(StageResult::skipped("graph"));
            }
        };
        
        let state = ctx.state.read().await;
        let mut parsed_items = state.parsed_items.clone();
        let source_files = state.source_files.clone();
        drop(state);
        
        // If no parsed_items in state, try to load from database
        if parsed_items.is_empty() {
            info!("No parsed items in state, loading from database...");
            match self.load_items_from_database(&ctx.config.database_url).await {
                Ok(items) => {
                    if items.is_empty() {
                        info!("No items found in database");
                        return Ok(StageResult::skipped("graph"));
                    }
                    // Group items by a dummy path since they came from DB
                    parsed_items.insert(PathBuf::from("__database__"), items);
                    info!("Loaded {} items from database", parsed_items.values().map(|v| v.len()).sum::<usize>());
                }
                Err(e) => {
                    warn!("Failed to load items from database: {}", e);
                    return Ok(StageResult::skipped("graph"));
                }
            }
        }
        
        if parsed_items.is_empty() {
            info!("No parsed items to insert into graph");
            return Ok(StageResult::skipped("graph"));
        }
        
        // Build Neo4j configuration
        let config = GraphConfig {
            uri: neo4j_url,
            username: std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
            password: std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "rustbrain_dev_2024".to_string()),
            database: std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string()),
            ..Default::default()
        };
        
        // Connect to Neo4j
        info!("Connecting to Neo4j at {}", config.uri);
        let graph_builder = match GraphBuilder::with_config(config).await {
            Ok(gb) => gb,
            Err(e) => {
                error!("Failed to connect to Neo4j: {}", e);
                return Ok(StageResult::failed("graph", format!("Neo4j connection failed: {}", e)));
            }
        };
        
        // Test connection
        match graph_builder.test_connection().await {
            Ok(true) => info!("Neo4j connection test successful"),
            Ok(false) => {
                error!("Neo4j connection test returned false");
                return Ok(StageResult::failed("graph", "Neo4j connection test failed"));
            }
            Err(e) => {
                error!("Neo4j connection test error: {}", e);
                return Ok(StageResult::failed("graph", format!("Neo4j connection test error: {}", e)));
            }
        }
        
        // Create indexes for better performance
        if let Err(e) = graph_builder.create_indexes().await {
            warn!("Failed to create indexes (may already exist): {}", e);
        }
        
        // Collect unique crate names
        let mut crate_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for sf in &source_files {
            crate_names.insert(sf.crate_name.clone());
        }
        
        // Create Crate nodes
        let mut all_nodes: Vec<NodeData> = Vec::new();
        for crate_name in &crate_names {
            all_nodes.push(Self::create_crate_node(crate_name));
        }
        
        // Collect all items as nodes
        let mut item_fqns: Vec<String> = Vec::new();
        let mut impl_items: Vec<&ParsedItemInfo> = Vec::new();
        
        for (_path, items) in &parsed_items {
            for item in items {
                let node = Self::item_to_node(item);
                all_nodes.push(node);
                item_fqns.push(item.fqn.clone());
                
                if item.item_type == "impl" {
                    impl_items.push(item);
                }
            }
        }
        
        info!("Inserting {} nodes into Neo4j", all_nodes.len());
        
        // Batch insert nodes
        let node_count = all_nodes.len();
        match graph_builder.create_nodes_batch(all_nodes).await {
            Ok(_) => info!("Successfully inserted {} nodes", node_count),
            Err(e) => {
                error!("Failed to insert nodes batch: {}", e);
                return Ok(StageResult::failed("graph", format!("Failed to insert nodes: {}", e)));
            }
        }
        
        // Flush to ensure all nodes are committed
        if let Err(e) = graph_builder.flush().await {
            warn!("Failed to flush graph batch: {}", e);
        }
        
        // ====================================================================
        // CREATE RELATIONSHIPS
        // ====================================================================
        
        info!("Creating relationships...");
        let mut relationships: Vec<RelationshipData> = Vec::new();
        
        // 1. Create CONTAINS relationships: Crate → Module, Crate → Items at crate root
        for (_path, items) in &parsed_items {
            for item in items {
                // Get parent FQN (module path)
                if let Some(parent_fqn) = Self::get_parent_fqn(&item.fqn) {
                    // Check if parent is a crate root (just the crate name)
                    let is_root_item = !parent_fqn.contains("::");
                    
                    if is_root_item {
                        // Crate → Item relationship
                        if crate_names.contains(&parent_fqn) {
                            relationships.push(RelationshipBuilder::create_contains(
                                parent_fqn.clone(),
                                item.fqn.clone(),
                            ));
                        }
                    } else {
                        // Module → Item relationship (parent module contains this item)
                        relationships.push(RelationshipBuilder::create_contains(
                            parent_fqn.clone(),
                            item.fqn.clone(),
                        ));
                    }
                }
            }
        }
        
        info!("Created {} CONTAINS relationships", relationships.len());
        let contains_count = relationships.len();
        
        // 2. Create IMPLEMENTS and FOR relationships for impl blocks
        let mut impl_count = 0;
        let mut for_count = 0;
        
        for impl_item in &impl_items {
            // Extract trait name from attributes (format: impl_for=TraitName)
            if let Some(trait_name) = Self::extract_trait_from_impl(&impl_item.attributes) {
                // Create IMPLEMENTS relationship: Impl → Trait
                // The trait FQN might be in the same crate or external
                // Try to find it in parsed items first, otherwise use the trait name directly
                let trait_fqn = Self::find_trait_fqn(&parsed_items, &trait_name, &impl_item.fqn)
                    .unwrap_or_else(|| trait_name.clone());
                
                relationships.push(RelationshipBuilder::create_implements(
                    impl_item.fqn.clone(),
                    trait_fqn,
                ));
                impl_count += 1;
            }
            
            // Create FOR relationship: Impl → Type (the type being implemented for)
            if let Some(self_type) = Self::extract_impl_self_type(&impl_item.fqn, &impl_item.name) {
                // The type FQN should be constructible from the module path + type name
                let type_fqn = if let Some(parent) = Self::get_parent_fqn(&impl_item.fqn) {
                    format!("{}::{}", parent, self_type)
                } else {
                    self_type.clone()
                };
                
                relationships.push(RelationshipBuilder::create_for(
                    impl_item.fqn.clone(),
                    type_fqn,
                ));
                for_count += 1;
            }
        }
        
        info!("Created {} IMPLEMENTS and {} FOR relationships", impl_count, for_count);
        
        // 3. Create HAS_FIELD relationships for structs
        let mut field_count = 0;
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "struct" && !item.body_source.is_empty() {
                    let fields = Self::extract_struct_fields(&item.body_source, &item.fqn);
                    for (field_name, field_type, position) in fields {
                        // Try to resolve the field type FQN
                        let type_fqn = Self::resolve_type_fqn(&item.fqn, &field_type);
                        
                        relationships.push(RelationshipBuilder::create_has_field(
                            item.fqn.clone(),
                            type_fqn,
                            field_name,
                            position,
                            item.visibility == "pub",
                            false, // has_default - would need more parsing
                        ));
                        field_count += 1;
                    }
                }
            }
        }
        info!("Created {} HAS_FIELD relationships", field_count);
        
        // 4. Create HAS_VARIANT relationships for enums
        let mut variant_count = 0;
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "enum" && !item.body_source.is_empty() {
                    let variants = Self::extract_enum_variants(&item.body_source, &item.fqn);
                    for (variant_name, variant_type, position) in variants {
                        let has_data = variant_type.is_some();
                        
                        relationships.push(RelationshipBuilder::create_has_variant(
                            item.fqn.clone(),
                            item.fqn.clone(), // The variant belongs to the enum
                            variant_name,
                            position,
                            has_data,
                        ));
                        variant_count += 1;
                    }
                }
            }
        }
        info!("Created {} HAS_VARIANT relationships", variant_count);
        
        // 5. Create EXTENDS relationships for trait inheritance
        let mut extends_count = 0;
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "trait" {
                    // Check generic_params for supertrait bounds
                    for generic_param in &item.generic_params {
                        if generic_param.kind == "type" {
                            for bound in &generic_param.bounds {
                                // These might be supertrait bounds
                                if !bound.starts_with(|c: char| c.is_lowercase()) {
                                    let super_trait_fqn = Self::find_trait_fqn(&parsed_items, bound, &item.fqn)
                                        .unwrap_or_else(|| bound.clone());
                                    
                                    relationships.push(RelationshipBuilder::create_extends(
                                        item.fqn.clone(),
                                        super_trait_fqn,
                                    ));
                                    extends_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        info!("Created {} EXTENDS relationships", extends_count);
        
        // 6. Create CALLS relationships for function calls
        // Build a set of all known function FQNs for fast lookup
        let mut function_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut function_names_to_fqns: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "function" {
                    function_fqns.insert(item.fqn.clone());
                    function_names_to_fqns
                        .entry(item.name.clone())
                        .or_default()
                        .push(item.fqn.clone());
                }
            }
        }
        
        let mut calls_count = 0;
        for (_path, items) in &parsed_items {
            for item in items {
                if (item.item_type == "function" || item.item_type == "impl") && !item.body_source.is_empty() {
                    let calls = Self::extract_function_calls(&item.body_source, &function_fqns, &function_names_to_fqns, &item.fqn);
                    for (callee_fqn, line) in calls {
                        relationships.push(RelationshipBuilder::create_calls(
                            item.fqn.clone(),
                            callee_fqn,
                            line,
                            "", // file path not needed here
                            Vec::new(), // concrete_types - would need type inference
                            true, // is_static_dispatch - simplified assumption
                        ));
                        calls_count += 1;
                    }
                }
            }
        }
        info!("Created {} CALLS relationships", calls_count);
        
        // 7. Create USES_TYPE relationships for type usage in functions/methods
        // Build a set of all known type FQNs (structs, enums, traits, type aliases)
        let mut type_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut type_names_to_fqns: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "struct" || item.item_type == "enum" || 
                   item.item_type == "trait" || item.item_type == "type_alias" {
                    type_fqns.insert(item.fqn.clone());
                    type_names_to_fqns
                        .entry(item.name.clone())
                        .or_default()
                        .push(item.fqn.clone());
                }
            }
        }
        
        let mut uses_type_count = 0;
        for (_path, items) in &parsed_items {
            for item in items {
                // Extract type usages from functions and impl methods
                if item.item_type == "function" {
                    // Extract types from signature
                    let sig_types = Self::extract_types_from_signature(&item.signature, &item.fqn);
                    for (type_name, context) in sig_types {
                        let type_fqn = Self::resolve_type_fqn_with_lookup(
                            &item.fqn, 
                            &type_name, 
                            &type_fqns, 
                            &type_names_to_fqns
                        );
                        
                        // Only create relationship if it's a known type or looks like a user type
                        if type_fqns.contains(&type_fqn) || !Self::is_primitive_type(&type_name) {
                            relationships.push(RelationshipBuilder::create_uses_type(
                                item.fqn.clone(),
                                type_fqn.clone(),
                                context,
                                Some(item.start_line),
                            ));
                            uses_type_count += 1;
                        }
                    }
                    
                    // Extract types from body
                    if !item.body_source.is_empty() {
                        let body_types = Self::extract_types_from_body(&item.body_source, &item.fqn);
                        for (type_name, line) in body_types {
                            let type_fqn = Self::resolve_type_fqn_with_lookup(
                                &item.fqn, 
                                &type_name, 
                                &type_fqns, 
                                &type_names_to_fqns
                            );
                            
                            if type_fqns.contains(&type_fqn) || !Self::is_primitive_type(&type_name) {
                                relationships.push(RelationshipBuilder::create_uses_type(
                                    item.fqn.clone(),
                                    type_fqn.clone(),
                                    "body",
                                    Some(line),
                                ));
                                uses_type_count += 1;
                            }
                        }
                    }
                }
                
                // Extract type usages from impl blocks
                if item.item_type == "impl" && !item.body_source.is_empty() {
                    let body_types = Self::extract_types_from_body(&item.body_source, &item.fqn);
                    for (type_name, line) in body_types {
                        let type_fqn = Self::resolve_type_fqn_with_lookup(
                            &item.fqn, 
                            &type_name, 
                            &type_fqns, 
                            &type_names_to_fqns
                        );
                        
                        if type_fqns.contains(&type_fqn) || !Self::is_primitive_type(&type_name) {
                            relationships.push(RelationshipBuilder::create_uses_type(
                                item.fqn.clone(),
                                type_fqn.clone(),
                                "body",
                                Some(line),
                            ));
                            uses_type_count += 1;
                        }
                    }
                }
            }
        }
        info!("Created {} USES_TYPE relationships", uses_type_count);
        
        // Batch insert relationships
        let relationship_count = relationships.len();
        if relationship_count > 0 {
            info!("Inserting {} relationships into Neo4j", relationship_count);
            
            match graph_builder.create_relationships_batch(relationships).await {
                Ok(_) => info!("Successfully inserted {} relationships", relationship_count),
                Err(e) => {
                    warn!("Failed to insert relationships batch: {}", e);
                    // Continue anyway - nodes were inserted successfully
                }
            }
            
            // Flush to ensure all relationships are committed
            if let Err(e) = graph_builder.flush().await {
                warn!("Failed to flush graph batch: {}", e);
            }
        }
        
        // Update state with graph node IDs
        let mut state = ctx.state.write().await;
        for fqn in &item_fqns {
            state.graph_nodes.insert(fqn.clone(), fqn.clone());
        }
        
        state.counts.graph_nodes = node_count;
        state.counts.graph_edges = relationship_count;
        
        let duration = start.elapsed();
        
        info!("Graph stage completed: {} nodes, {} edges inserted in {}ms", 
            node_count, relationship_count, duration.as_millis());
        
        Ok(StageResult::success("graph", node_count + relationship_count, 0, duration))
    }
}

impl GraphStage {
    /// Find the FQN of a trait by name, searching in the same module first
    fn find_trait_fqn(
        parsed_items: &HashMap<PathBuf, Vec<ParsedItemInfo>>,
        trait_name: &str,
        impl_fqn: &str,
    ) -> Option<String> {
        // Get the module path from the impl FQN
        let module_path = Self::get_parent_fqn(impl_fqn)?;
        
        // Search for the trait in the same module first
        for (_path, items) in parsed_items {
            for item in items {
                if item.item_type == "trait" && item.name == trait_name {
                    return Some(item.fqn.clone());
                }
            }
        }
        
        // If not found, try to construct the FQN from the module path
        // This handles cases where the trait is in the same module
        Some(format!("{}::{}", module_path, trait_name))
    }
    
    /// Resolve a type string to an FQN
    fn resolve_type_fqn(context_fqn: &str, type_str: &str) -> String {
        // Get the module path from the context FQN
        let module_path = Self::get_parent_fqn(context_fqn);
        
        // Handle common Rust types
        let primitive_types = [
            "i8", "i16", "i32", "i64", "i128", "isize",
            "u8", "u16", "u32", "u64", "u128", "usize",
            "f32", "f64", "bool", "char", "str",
            "String", "Vec", "Option", "Result", "Box",
            "Rc", "Arc", "Cow", "Cell", "RefCell",
        ];
        
        // Check if it's a primitive or standard library type
        if primitive_types.contains(&type_str) || type_str.starts_with(|c: char| c.is_lowercase()) {
            return type_str.to_string();
        }
        
        // If type contains ::, it's already an FQN or path
        if type_str.contains("::") {
            return type_str.to_string();
        }
        
        // Construct FQN from module path and type name
        if let Some(module) = module_path {
            format!("{}::{}", module, type_str)
        } else {
            type_str.to_string()
        }
    }
    
    /// Extract function calls from body source
    /// Returns a list of (callee_fqn, line_number) tuples
    fn extract_function_calls(
        body_source: &str,
        function_fqns: &std::collections::HashSet<String>,
        function_names_to_fqns: &std::collections::HashMap<String, Vec<String>>,
        caller_fqn: &str,
    ) -> Vec<(String, usize)> {
        let mut calls = Vec::new();
        let caller_module = Self::get_parent_fqn(caller_fqn);
        
        // Patterns to match:
        // 1. function_name(args) - simple function call
        // 2. Type::method(args) - static method call
        // 3. self.method(args) - instance method call (skip, needs type info)
        // 4. obj.method(args) - method call on object (skip, needs type info)
        
        for (line_num, line) in body_source.lines().enumerate() {
            let line = line.trim();
            
            // Skip comments and attributes
            if line.starts_with("//") || line.starts_with("#") || line.starts_with("///") {
                continue;
            }
            
            // Pattern 1: Simple function call - identifier(args)
            // Match word followed by open paren, but not keywords
            let keywords = [
                "if", "while", "for", "match", "fn", "let", "const", "static",
                "pub", "mod", "use", "struct", "enum", "trait", "impl", "type",
                "async", "unsafe", "extern", "crate", "self", "super", "where",
                "return", "break", "continue", "yield", "await", "move", "ref",
                "mut", "as", "in", "else", "loop", "dyn", "box",
            ];
            
            // Find all potential function calls using a simple pattern
            // Look for identifier( or path::identifier(
            let mut pos = 0;
            let chars: Vec<char> = line.chars().collect();
            
            while pos < chars.len() {
                // Look for identifier followed by (
                if chars[pos].is_alphabetic() || chars[pos] == '_' || chars[pos] == ':' {
                    let start = pos;
                    
                    // Collect the identifier/path
                    while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_' || chars[pos] == ':') {
                        pos += 1;
                    }
                    
                    // Skip whitespace
                    while pos < chars.len() && chars[pos].is_whitespace() {
                        pos += 1;
                    }
                    
                    // Check if followed by ( or <
                    if pos < chars.len() && (chars[pos] == '(' || chars[pos] == '<') {
                        let identifier: String = chars[start..pos].iter().collect();
                        let identifier = identifier.trim();
                        
                        // Skip keywords
                        if keywords.contains(&identifier) {
                            pos += 1;
                            continue;
                        }
                        
                        // Skip self/super/crate prefixes (these are relative paths)
                        if identifier.starts_with("self::") || identifier.starts_with("super::") || identifier.starts_with("crate::") {
                            // Try to resolve the full path
                            if let Some(callee) = Self::resolve_call_target(identifier, function_fqns, caller_module.as_deref()) {
                                if callee != caller_fqn { // Don't create self-calls
                                    calls.push((callee, line_num + 1));
                                }
                            }
                        } else if identifier.contains("::") {
                            // Path like Type::method or module::function
                            // Check if it's a known function FQN or can be resolved
                            if function_fqns.contains(identifier) {
                                if identifier != caller_fqn {
                                    calls.push((identifier.to_string(), line_num + 1));
                                }
                            } else if let Some(callee) = Self::resolve_call_target(identifier, function_fqns, caller_module.as_deref()) {
                                if callee != caller_fqn {
                                    calls.push((callee, line_num + 1));
                                }
                            }
                        } else {
                            // Simple identifier - look up in function names
                            if let Some(fqns) = function_names_to_fqns.get(identifier) {
                                // Prefer functions in the same module
                                let callee = if let Some(ref module) = caller_module {
                                    fqns.iter()
                                        .find(|fqn| fqn.starts_with(&format!("{}::", module)))
                                        .or_else(|| fqns.first())
                                } else {
                                    fqns.first()
                                };
                                
                                if let Some(callee_fqn) = callee {
                                    if callee_fqn != caller_fqn {
                                        calls.push((callee_fqn.clone(), line_num + 1));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    pos += 1;
                }
            }
        }
        
        calls
    }
    
    /// Resolve a call target identifier to an FQN
    fn resolve_call_target(
        identifier: &str,
        function_fqns: &std::collections::HashSet<String>,
        caller_module: Option<&str>,
    ) -> Option<String> {
        // If it's already a full FQN, check if it exists
        if function_fqns.contains(identifier) {
            return Some(identifier.to_string());
        }
        
        // Try prepending the caller's module
        if let Some(module) = caller_module {
            let full_fqn = format!("{}::{}", module, identifier);
            if function_fqns.contains(&full_fqn) {
                return Some(full_fqn);
            }
        }
        
        // Try to find by the last part of the path
        // For Type::method, check if there's a function with that name
        if let Some(last_sep) = identifier.rfind("::") {
            let method_name = &identifier[last_sep + 2..];
            
            // Look for any function ending with ::method_name
            for fqn in function_fqns {
                if fqn.ends_with(&format!("::{}", method_name)) {
                    // Check if the type/path prefix matches
                    return Some(fqn.clone());
                }
            }
        }
        
        None
    }
    
    /// Check if a type name is a primitive or standard library type
    fn is_primitive_type(type_name: &str) -> bool {
        let primitive_types = [
            "i8", "i16", "i32", "i64", "i128", "isize",
            "u8", "u16", "u32", "u64", "u128", "usize",
            "f32", "f64", "bool", "char", "str",
            "String", "Vec", "Option", "Result", "Box",
            "Rc", "Arc", "Cow", "Cell", "RefCell",
            "Mutex", "RwLock", "Arc", "Weak",
            "HashMap", "HashSet", "BTreeMap", "BTreeSet",
            "VecDeque", "LinkedList", "BinaryHeap",
            "Cow", "PhantomData", "PhantomPinned",
            "Duration", "Instant", "SystemTime",
            "Path", "PathBuf", "OsStr", "OsString",
            "IpAddr", "Ipv4Addr", "Ipv6Addr", "SocketAddr",
            "Error", "BoxError", "io", "fmt", "Debug", "Display",
            "Clone", "Copy", "Default", "Eq", "Hash", "Ord", "PartialEq", "PartialOrd",
            "Send", "Sync", "Sized", "Unpin", "From", "Into", "TryFrom", "TryInto",
            "AsRef", "AsMut", "Deref", "DerefMut", "Index", "IndexMut",
            "Add", "Sub", "Mul", "Div", "Rem", "Neg", "Not", "BitAnd", "BitOr", "BitXor",
            "Fn", "FnMut", "FnOnce", "Future", "Stream", "Iterator",
            "Self", "self", "static", "dyn",
        ];
        
        // Check if it's a primitive type or starts with lowercase (type parameter)
        primitive_types.contains(&type_name) || 
            type_name.starts_with(|c: char| c.is_lowercase() && c != '_') ||
            type_name.len() == 1 // Single letter types like T, U, V are usually generic params
    }
    
    /// Resolve a type name to an FQN using the type lookup maps
    fn resolve_type_fqn_with_lookup(
        context_fqn: &str,
        type_name: &str,
        type_fqns: &std::collections::HashSet<String>,
        type_names_to_fqns: &std::collections::HashMap<String, Vec<String>>,
    ) -> String {
        // If it's already a full FQN, check if it exists
        if type_fqns.contains(type_name) {
            return type_name.to_string();
        }
        
        // If it contains ::, it might be a path
        if type_name.contains("::") {
            return type_name.to_string();
        }
        
        // Get the module path from the context FQN
        let module_path = Self::get_parent_fqn(context_fqn);
        
        // Try to find in type_names_to_fqns
        if let Some(fqns) = type_names_to_fqns.get(type_name) {
            // Prefer types in the same module
            if let Some(ref module) = module_path {
                if let Some(fqn) = fqns.iter().find(|fqn| fqn.starts_with(&format!("{}::", module))) {
                    return fqn.clone();
                }
            }
            // Fall back to first match
            if let Some(fqn) = fqns.first() {
                return fqn.clone();
            }
        }
        
        // Construct FQN from module path and type name
        if let Some(module) = module_path {
            format!("{}::{}", module, type_name)
        } else {
            type_name.to_string()
        }
    }
    
    /// Extract type names from a function signature
    /// Returns a list of (type_name, context) tuples
    fn extract_types_from_signature(signature: &str, _context_fqn: &str) -> Vec<(String, String)> {
        let mut types = Vec::new();
        
        // Common patterns:
        // - fn name(param: Type) -> ReturnType
        // - param: &Type, param: &mut Type, param: Type
        // - -> Type, -> Option<Type>, -> Result<Type, Error>
        
        // Extract types from parameters (after ':')
        if let Some(params_start) = signature.find('(') {
            if let Some(params_end) = signature.find(')') {
                let params = &signature[params_start + 1..params_end];
                
                // Split by comma and extract types
                for param in params.split(',') {
                    let param = param.trim();
                    if param.is_empty() {
                        continue;
                    }
                    
                    // Find the colon that separates name from type
                    if let Some(colon_pos) = param.find(':') {
                        let type_part = param[colon_pos + 1..].trim();
                        
                        // Extract type names from the type part
                        let extracted = Self::extract_type_names(type_part);
                        for type_name in extracted {
                            types.push((type_name, "parameter".to_string()));
                        }
                    }
                }
            }
        }
        
        // Extract return type (after '->')
        if let Some(ret_pos) = signature.find("->") {
            let ret_type = signature[ret_pos + 2..].trim();
            let ret_type = ret_type.trim_end_matches(';').trim();
            
            // Handle where clause - stop at 'where'
            let ret_type = if let Some(where_pos) = ret_type.find(" where") {
                &ret_type[..where_pos]
            } else {
                ret_type
            };
            
            let extracted = Self::extract_type_names(ret_type);
            for type_name in extracted {
                types.push((type_name, "return".to_string()));
            }
        }
        
        types
    }
    
    /// Extract type names from a type expression
    fn extract_type_names(type_str: &str) -> Vec<String> {
        let mut types = Vec::new();
        let type_str = type_str.trim();
        
        if type_str.is_empty() {
            return types;
        }
        
        // Remove lifetime annotations
        let type_str = type_str.replace("'static", "").replace("'a", "").replace("'_", "");
        
        // Handle common patterns:
        // - Option<Type> -> extract Type
        // - Result<Type, Error> -> extract Type, Error
        // - Vec<Type> -> extract Type
        // - &Type, &mut Type -> extract Type
        // - Box<Type> -> extract Type
        // - Type<A, B> -> extract Type, A, B
        
        // Remove references
        let type_str = type_str.trim_start_matches('&').trim();
        let type_str = type_str.trim_start_matches("mut ").trim();
        
        // Handle generic types
        if let Some(angle_start) = type_str.find('<') {
            // Get the outer type name
            let outer_type = type_str[..angle_start].trim();
            if !outer_type.is_empty() && !Self::is_primitive_type(outer_type) {
                types.push(outer_type.to_string());
            }
            
            // Extract inner types (handle nested generics by counting brackets)
            let inner_start = angle_start + 1;
            let mut depth = 1;
            let mut current_start = inner_start;
            
            for (i, c) in type_str[inner_start..].char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            // End of generic params
                            let inner = &type_str[current_start..inner_start + i];
                            for part in inner.split(',') {
                                let part = part.trim();
                                if !part.is_empty() {
                                    // Recursively extract types
                                    let inner_types = Self::extract_type_names(part);
                                    types.extend(inner_types);
                                }
                            }
                            break;
                        }
                    }
                    ',' if depth == 1 => {
                        let inner = &type_str[current_start..inner_start + i];
                        let inner = inner.trim();
                        if !inner.is_empty() {
                            let inner_types = Self::extract_type_names(inner);
                            types.extend(inner_types);
                        }
                        current_start = inner_start + i + 1;
                    }
                    _ => {}
                }
            }
        } else {
            // Simple type
            // Handle arrays: [Type; N]
            if type_str.starts_with('[') {
                if let Some(semi_pos) = type_str.find(';') {
                    let inner = &type_str[1..semi_pos].trim();
                    let inner_types = Self::extract_type_names(inner);
                    types.extend(inner_types);
                }
            } else if !type_str.is_empty() && !Self::is_primitive_type(type_str) {
                types.push(type_str.to_string());
            }
        }
        
        types
    }
    
    /// Extract type names from function body
    /// Returns a list of (type_name, line_number) tuples
    fn extract_types_from_body(body_source: &str, _context_fqn: &str) -> Vec<(String, usize)> {
        let mut types = Vec::new();
        
        // Patterns to match:
        // - Type::method() - static method calls on a type
        // - Type { ... } - struct instantiation
        // - Type::Variant - enum variant access
        // - let x: Type = ...
        // - as Type - type casting
        // - <Type as Trait>::method - fully qualified syntax
        
        for (line_num, line) in body_source.lines().enumerate() {
            let line = line.trim();
            
            // Skip comments and attributes
            if line.starts_with("//") || line.starts_with("#") || line.starts_with("///") {
                continue;
            }
            
            // Pattern 1: Type::method() - static method call
            // Look for identifier:: followed by method call
            if let Some(pos) = line.find("::") {
                let before = &line[..pos];
                // Get the type name (last identifier before ::)
                if let Some(type_name) = before.split(|c: char| !c.is_alphanumeric() && c != '_').last() {
                    if !type_name.is_empty() && !Self::is_primitive_type(type_name) {
                        // Check it's not a keyword or self/super/crate
                        let keywords = ["self", "super", "crate", "Self"];
                        if !keywords.contains(&type_name) {
                            types.push((type_name.to_string(), line_num + 1));
                        }
                    }
                }
            }
            
            // Pattern 2: let x: Type = ...
            if line.contains("let") && line.contains(':') {
                // Find the type annotation
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() >= 2 {
                    let type_part = parts[1].split('=').next().unwrap_or("").trim();
                    let extracted = Self::extract_type_names(type_part);
                    for type_name in extracted {
                        types.push((type_name, line_num + 1));
                    }
                }
            }
            
            // Pattern 3: as Type - type casting
            if line.contains(" as ") {
                if let Some(as_pos) = line.find(" as ") {
                    let after_as = &line[as_pos + 4..];
                    // Get the type (until next non-type char)
                    let type_name: String = after_as
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':' || *c == '<' || *c == '>')
                        .collect();
                    if !type_name.is_empty() && !Self::is_primitive_type(&type_name) {
                        types.push((type_name, line_num + 1));
                    }
                }
            }
            
            // Pattern 4: Type { ... } - struct instantiation
            // Look for pattern: identifier { (not preceded by keywords)
            let struct_pattern = regex::Regex::new(r"\b([A-Z][a-zA-Z0-9_]*)\s*\{").ok();
            if let Some(re) = struct_pattern {
                for cap in re.captures_iter(line) {
                    if let Some(type_name) = cap.get(1) {
                        let type_name = type_name.as_str();
                        if !Self::is_primitive_type(type_name) {
                            types.push((type_name.to_string(), line_num + 1));
                        }
                    }
                }
            }
        }
        
        // Deduplicate while preserving order (keep first occurrence)
        let mut seen = std::collections::HashSet::new();
        types.retain(|(type_name, _)| {
            let type_lower = type_name.to_lowercase();
            if seen.contains(&type_lower) {
                false
            } else {
                seen.insert(type_lower);
                true
            }
        });
        
        types
    }
}

// =============================================================================
// EMBED STAGE
// =============================================================================

/// Stage 6: Create vector embeddings
pub struct EmbedStage {
    embedding_url: Option<String>,
}

impl EmbedStage {
    pub fn new() -> Self {
        Self { embedding_url: None }
    }
    
    /// Get Ollama URL from environment or config
    fn get_ollama_url(ctx: &PipelineContext) -> String {
        ctx.config.embedding_url.clone()
            .or_else(|| std::env::var("OLLAMA_HOST").ok())
            .unwrap_or_else(|| "http://ollama:11434".to_string())
    }
    
    /// Get Qdrant URL from environment
    fn get_qdrant_url() -> String {
        std::env::var("QDRANT_HOST")
            .unwrap_or_else(|_| "http://qdrant:6333".to_string())
    }
}

#[async_trait::async_trait]
impl PipelineStage for EmbedStage {
    fn name(&self) -> &str {
        "embed"
    }
    
    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        
        info!("Starting embed stage");
        
        if ctx.config.dry_run {
            info!("Dry run - skipping embedding");
            return Ok(StageResult::skipped("embed"));
        }
        
        let state = ctx.state.read().await;
        let parsed_items = state.parsed_items.clone();
        let source_files = state.source_files.clone();
        drop(state);
        
        if parsed_items.is_empty() {
            info!("No parsed items to embed");
            return Ok(StageResult::skipped("embed"));
        }
        
        // Get service URLs
        let ollama_url = Self::get_ollama_url(ctx);
        let qdrant_url = Self::get_qdrant_url();
        
        info!("Connecting to Ollama at {} and Qdrant at {}", ollama_url, qdrant_url);
        
        // Create embedding service
        let embedding_service = match crate::embedding::EmbeddingService::with_urls(ollama_url, qdrant_url) {
            Ok(s) => s,
            Err(e) => {
                return Ok(StageResult::failed("embed", format!("Failed to create embedding service: {}", e)));
            }
        };
        
        // Initialize (ensure collections exist, check model)
        if let Err(e) = embedding_service.initialize().await {
            warn!("Embedding service initialization warning: {}", e);
            // Continue anyway - the service might still work
        }
        
        // Collect all items for embedding
        let mut all_items: Vec<ParsedItem> = Vec::new();
        let mut path_to_crate: HashMap<std::path::PathBuf, String> = HashMap::new();
        
        for sf in &source_files {
            path_to_crate.insert(sf.path.clone(), sf.crate_name.clone());
        }
        
        for (path, items) in &parsed_items {
            // Get module_path and crate_name from source_files for file_path
            let file_path_str = path.to_string_lossy().to_string();
            let module_path = path_to_crate.get(path).map(|s| s.as_str()).unwrap_or("");
            
            for item_info in items {
                // Reconstruct ParsedItem from ParsedItemInfo with ALL fields preserved
                let parsed_item = ParsedItem {
                    fqn: item_info.fqn.clone(),
                    item_type: parse_item_type(&item_info.item_type),
                    name: item_info.name.clone(),
                    visibility: parse_visibility(&item_info.visibility),
                    signature: item_info.signature.clone(),
                    generic_params: item_info.generic_params.clone(),
                    where_clauses: item_info.where_clauses.clone(),
                    attributes: item_info.attributes.clone(),
                    doc_comment: item_info.doc_comment.clone(),
                    start_line: item_info.start_line,
                    end_line: item_info.end_line,
                    body_source: item_info.body_source.clone(),
                    generated_by: item_info.generated_by.clone(),
                };
                all_items.push(parsed_item);
            }
        }
        
        info!("Embedding {} items...", all_items.len());
        
        // Embed all items in batches
        let mut state = ctx.state.write().await;
        let mut embedded_count = 0;
        let mut failed_count = 0;
        
        // Process in batches of 100 items
        const BATCH_SIZE: usize = 100;
        
        for (batch_num, chunk) in all_items.chunks(BATCH_SIZE).enumerate() {
            debug!("Processing embedding batch {}/{}", 
                batch_num + 1, 
                (all_items.len() + BATCH_SIZE - 1) / BATCH_SIZE
            );
            
            match embedding_service.embed_items(chunk).await {
                Ok(results) => {
                    embedded_count += results.len();
                    debug!("Embedded {} items in batch {}", results.len(), batch_num + 1);
                }
                Err(e) => {
                    warn!("Failed to embed batch {}: {}", batch_num, e);
                    state.errors.push(StageError::new("embed", e.to_string())
                        .with_context(format!("batch {}", batch_num)));
                    failed_count += chunk.len();
                }
            }
        }
        
        state.counts.embeddings_created = embedded_count;
        
        let duration = start.elapsed();
        
        if failed_count > 0 && embedded_count == 0 {
            Ok(StageResult::failed("embed", "All embedding attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("embed", embedded_count, failed_count, duration,
                format!("{} items embedded, {} failed", embedded_count, failed_count)))
        } else {
            Ok(StageResult::success("embed", embedded_count, 0, duration))
        }
    }
}

/// Parse item type string to ItemType enum
fn parse_item_type(s: &str) -> ItemType {
    match s {
        "function" => ItemType::Function,
        "struct" => ItemType::Struct,
        "enum" => ItemType::Enum,
        "trait" => ItemType::Trait,
        "impl" => ItemType::Impl,
        "type_alias" => ItemType::TypeAlias,
        "const" => ItemType::Const,
        "static" => ItemType::Static,
        "macro" => ItemType::Macro,
        "module" => ItemType::Module,
        "use" => ItemType::Use,
        _ => ItemType::Unknown(s.to_string()),
    }
}

/// Parse visibility string to Visibility enum
fn parse_visibility(s: &str) -> Visibility {
    match s {
        "pub" => Visibility::Public,
        "pub_crate" => Visibility::PubCrate,
        "pub_super" => Visibility::PubSuper,
        "private" | "" => Visibility::Private,
        other if other.starts_with("pub(in ") => Visibility::PubIn(other.to_string()),
        other if other.starts_with("pub(") => Visibility::PubIn(other.to_string()),
        _ => Visibility::Private,
    }
}

// =============================================================================
// HELPER FUNCTIONS
// =============================================================================

/// Compute module path from file path
fn compute_module_path(crate_path: &Path, file_path: &Path, crate_name: &str) -> String {
    let src_path = crate_path.join("src");
    
    if let Ok(relative) = file_path.strip_prefix(&src_path) {
        let module = relative
            .to_string_lossy()
            .trim_end_matches(".rs")
            .replace("/", "::")
            .replace("\\", "::");
        
        // Handle special cases
        if module == "lib" || module == "main" {
            crate_name.to_string()
        } else if module.starts_with("bin::") {
            module
        } else {
            format!("{}::{}", crate_name, module)
        }
    } else {
        crate_name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_stage_result_success() {
        let result = StageResult::success("test", 10, 0, Duration::from_millis(100));
        assert_eq!(result.name, "test");
        assert_eq!(result.status, StageStatus::Success);
        assert_eq!(result.items_processed, 10);
    }
    
    #[test]
    fn test_stage_result_partial() {
        let result = StageResult::partial("test", 8, 2, Duration::from_millis(100), "some failed");
        assert_eq!(result.status, StageStatus::Partial);
        assert!(result.error.is_some());
    }
    
    #[test]
    fn test_stage_error() {
        let err = StageError::new("expand", "test error");
        assert!(!err.is_fatal);
        
        let fatal = StageError::fatal("expand", "fatal error");
        assert!(fatal.is_fatal);
    }
}
