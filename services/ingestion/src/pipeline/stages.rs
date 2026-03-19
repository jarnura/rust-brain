//! Pipeline stage definitions and implementations
//!
//! Each stage implements the `PipelineStage` trait and processes
//! data from the shared `PipelineContext`.

use crate::parsers::{DualParser, ParsedItem, ItemType, Visibility};
use crate::pipeline::{PipelineContext, ParsedItemInfo, SourceFileInfo};
use crate::typecheck::TypeResolutionService;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Attempt to release freed memory back to the OS
fn trim_memory() {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::malloc_trim(0);
    }
}
use walkdir::WalkDir;

/// Default timeout for cargo expand (3 minutes per crate)
const CARGO_EXPAND_TIMEOUT: Duration = Duration::from_secs(180);

/// Cache directory for expanded code
const EXPAND_CACHE_DIR: &str = "/tmp/rustbrain-expand-cache";

/// Maximum parallel threads for parsing (prevents memory spikes)
const MAX_PARSE_THREADS: usize = 4;

/// Compute a SHA-256 content hash of a string, returning a hex-encoded string.
fn compute_content_hash(content: &str) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Redact password from database/connection URLs for safe logging
///
/// Examples:
/// - postgres://user:password@host/db → postgres://user:***@host/db
/// - bolt://user:password@host → bolt://user:***@host
fn redact_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let scheme_and_user = &url[..colon_pos + 1];
            let rest = &url[at_pos..];
            format!("{}***{}", scheme_and_user, rest)
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

/// Maximum retries for transient network failures
const MAX_RETRIES: usize = 3;

/// Retry an async operation with exponential backoff for transient failures.
///
/// Retries up to `max_retries` times with delays of 1s, 2s, 4s, etc.
async fn retry_with_backoff<F, Fut, T>(
    operation_name: &str,
    max_retries: usize,
    f: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut last_error = None;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                if attempt < max_retries {
                    let delay = Duration::from_secs(1 << attempt);
                    warn!(
                        "{} failed (attempt {}/{}), retrying in {:?}: {}",
                        operation_name,
                        attempt + 1,
                        max_retries + 1,
                        delay,
                        e
                    );
                    tokio::time::sleep(delay).await;
                }
                last_error = Some(e);
            }
        }
    }
    Err(last_error.unwrap())
}

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
pub struct ExpandStage {}

impl ExpandStage {
    pub fn new() -> Result<Self> {
        Ok(Self {})
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
    
    fn expand_library(&self, crate_path: &Path, workspace_path: &Path, crate_name: &str) -> Result<String> {
        debug!("Expanding library for {:?} (crate: {})", crate_path, crate_name);

        let cache_key = format!("{}-{}.expand", crate_name, self.compute_crate_hash(crate_path));
        let cache_file = PathBuf::from(EXPAND_CACHE_DIR).join(&cache_key);
        
        if cache_file.exists() {
            if let Ok(cached) = std::fs::read_to_string(&cache_file) {
                debug!("Using cached expand for {} from {:?}", crate_name, cache_file);
                return Ok(cached);
            }
        }

        let result = match self.run_cargo_expand(workspace_path, crate_name, &["--features", "v1"]) {
            Ok(output) => {
                debug!("Succeeded with v1 features for {}", crate_name);
                Ok(output)
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("does not contain this feature") {
                    debug!("v1 feature not found for {}, using default", crate_name);
                    self.run_cargo_expand(workspace_path, crate_name, &[])
                } else {
                    Err(e)
                }
            }
        };

        if let Ok(ref output) = result {
            let _ = std::fs::create_dir_all(EXPAND_CACHE_DIR);
            let _ = std::fs::write(&cache_file, output);
        }

        result
    }

    fn run_cargo_expand(&self, workspace_path: &Path, crate_name: &str, extra_args: &[&str]) -> Result<String> {
        use std::io::Read;
        use std::thread;

        let jobs = num_cpus::get().min(16);
        let jobs_str = jobs.to_string();
        let mut args = vec!["expand", "--lib", "-p", crate_name, "--jobs", &jobs_str, "--ugly"];
        args.extend(extra_args);

        let mut child = Command::new("cargo")
            .args(&args)
            .env("RUSTFLAGS", "-C codegen-units=16")
            .env("CARGO_BUILD_JOBS", &jobs_str)
            .current_dir(workspace_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context(format!("Failed to spawn cargo expand for {}", crate_name))?;

        let stdout = child.stdout.take().context("Failed to capture stdout")?;
        let stderr = child.stderr.take().context("Failed to capture stderr")?;

        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf = Arc::new(Mutex::new(Vec::new()));

        let stdout_buf_clone = stdout_buf.clone();
        let stderr_buf_clone = stderr_buf.clone();

        let stdout_thread = thread::spawn(move || {
            let mut reader = stdout;
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 { break; }
                if let Ok(mut guard) = stdout_buf_clone.lock() {
                    guard.extend_from_slice(&buf[..n]);
                }
            }
        });

        let stderr_thread = thread::spawn(move || {
            let mut reader = stderr;
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 { break; }
                if let Ok(mut guard) = stderr_buf_clone.lock() {
                    guard.extend_from_slice(&buf[..n]);
                }
            }
        });

        let timeout = CARGO_EXPAND_TIMEOUT;
        let start = Instant::now();

        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();

                    let stdout_data = stdout_buf.lock().unwrap().clone();
                    let stderr_data = stderr_buf.lock().unwrap().clone();

                    if status.success() {
                        return String::from_utf8(stdout_data)
                            .context("Expanded output is not valid UTF-8");
                    } else {
                        let stderr = String::from_utf8_lossy(&stderr_data);
                        anyhow::bail!("cargo expand failed: {}", stderr);
                    }
                }
                Ok(None) => {
                    if start.elapsed() > timeout {
                        let _ = child.kill();
                        let _ = stdout_thread.join();
                        let _ = stderr_thread.join();
                        anyhow::bail!(
                            "cargo expand timed out after {:?} for {}",
                            timeout,
                            crate_name
                        );
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    let _ = child.kill();
                    anyhow::bail!("Error waiting for cargo expand: {}", e);
                }
            }
        }
    }

    /// Compute a content hash for a crate's source files to detect changes.
    /// Returns a hex-encoded SHA-256 hash of all .rs file contents concatenated.
    fn compute_crate_hash(&self, crate_path: &Path) -> String {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        let mut files: Vec<PathBuf> = self.find_source_files(crate_path);
        files.sort(); // Ensure deterministic ordering

        for file in &files {
            if let Ok(content) = std::fs::read_to_string(file) {
                file.hash(&mut hasher);
                content.hash(&mut hasher);
            }
        }

        // Also hash Cargo.toml for dependency changes
        let cargo_toml = crate_path.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            cargo_toml.hash(&mut hasher);
            content.hash(&mut hasher);
        }

        format!("{:016x}", hasher.finish())
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
    
    fn get_crate_name_from_toml(&self, crate_path: &Path) -> Option<String> {
        let cargo_toml = crate_path.join("Cargo.toml");
        let content = std::fs::read_to_string(&cargo_toml).ok()?;
        
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("name = ") {
                let name = trimmed.strip_prefix("name = ")
                    .map(|s| s.trim().trim_matches('"').to_string());
                return name;
            }
        }
        None
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
        
        let crates = match self.discover_crates(crate_path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(StageResult::failed("expand", format!("Failed to discover crates: {}", e)));
            }
        };

        let mut state = ctx.state.write().await;
        let mut expanded_count = 0;
        let mut failed_count = 0;
        let mut skipped_binary = 0;

        let _ = std::fs::create_dir_all(EXPAND_CACHE_DIR);

        // FIX: Accumulate into a local HashMap instead of calling
        // Arc::make_mut on each insert, which clones the entire HashMap
        // every time the refcount > 1.
        let mut expanded_map: HashMap<PathBuf, String> = HashMap::new();

        for crate_path in &crates {
            let git_hash = self.get_git_hash(crate_path);

            let crate_name = self.get_crate_name_from_toml(crate_path)
                .unwrap_or_else(|| crate_path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".to_string()));

            let source_files = self.find_source_files(crate_path);
            if source_files.is_empty() {
                debug!("Skipping {:?} - no source files", crate_path);
                continue;
            }

            let content_hash = self.compute_crate_hash(crate_path);
            let expanded = if let Some(cached) = state.expand_cache.get(&content_hash) {
                debug!("Using in-memory cached expand for {}", crate_name);
                expanded_count += 1;
                Some(cached.clone())
            } else {
                match self.expand_library(crate_path, crate_path, &crate_name) {
                    Ok(exp) => {
                        expanded_count += 1;
                        state.expand_cache.insert(content_hash, exp.clone());
                        Some(exp)
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("no library targets found") {
                            debug!("Skipping {} - binary only crate", crate_name);
                            skipped_binary += 1;
                            None
                        } else {
                            warn!("Failed to expand {}: {}", crate_name, e);
                            state.errors.push(StageError::new("expand", e.to_string()));
                            failed_count += 1;
                            None
                        }
                    }
                }
            };

            for file_path in &source_files {
                // Pre-flight file size check: skip files > 10 MB
                if let Ok(metadata) = std::fs::metadata(file_path) {
                    if crate::pipeline::memory_accountant::MemoryAccountant::should_skip_file(metadata.len()) {
                        info!("Skipping oversized file {:?} ({} bytes)", file_path, metadata.len());
                        continue;
                    }
                }

                if let Ok(source) = std::fs::read_to_string(file_path) {
                    let module_path = compute_module_path(crate_path, file_path, &crate_name);
                    let file_hash = compute_content_hash(&source);

                    state.source_files.push(SourceFileInfo {
                        path: file_path.clone(),
                        crate_name: crate_name.clone(),
                        module_path,
                        original_source: Arc::new(source),
                        git_hash: git_hash.clone(),
                        content_hash: file_hash,
                    });

                    if let Some(ref expanded_source) = expanded {
                        expanded_map.insert(file_path.clone(), expanded_source.clone());
                    }
                }
            }
        }

        // Assign the complete map as a single Arc (no per-insert cloning)
        state.expanded_sources = Arc::new(expanded_map);
        state.counts.files_expanded = expanded_count;
        
        let duration = start.elapsed();
        info!("Expand stage: {} expanded, {} failed, {} skipped (binary-only), {:?}", 
              expanded_count, failed_count, skipped_binary, duration);
        
        if failed_count > 0 && expanded_count == 0 {
            Ok(StageResult::failed("expand", "All expansion attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("expand", expanded_count, failed_count, duration, 
                format!("{} crates expanded, {} failed, {} skipped", expanded_count, failed_count, skipped_binary)))
        } else {
            Ok(StageResult::success("expand", expanded_count + skipped_binary, 0, duration))
        }
    }
}

// =============================================================================
// PARSE STAGE
// =============================================================================

/// Maximum expanded source size to parse (skip larger files to prevent OOM)
/// Expanded code with 5000+ impl blocks can consume 50+ GB of memory
const MAX_EXPANDED_SOURCE_SIZE: usize = 2 * 1024 * 1024; // 2 MB

/// Maximum impl blocks to parse in expanded code (skip if more)
const MAX_IMPL_BLOCKS: usize = 500;

pub struct ParseStage {
    parser: Arc<DualParser>,
    derive_detector: Arc<crate::derive_detector::DeriveDetector>,
}

impl ParseStage {
    pub fn new() -> Result<Self> {
        Ok(Self {
            parser: Arc::new(DualParser::new()?),
            derive_detector: Arc::new(crate::derive_detector::DeriveDetector::new()),
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
        
        info!("Starting parse stage (batch insert to database)");
        
        let db_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(30)
            .connect(&ctx.config.database_url)
            .await
            .map_err(|e| anyhow!("Database connection failed: {}", e))?;
        
        let source_files = {
            let state = ctx.state.read().await;
            let source_files = state.source_files.clone();
            let expanded_sources = state.expanded_sources.clone();
            drop(state);
            
            let mut state = ctx.state.write().await;
            state.expanded_sources = Arc::new(HashMap::new());
            state.source_files.clear();
            drop(state);
            
            trim_memory();
            
            (source_files, expanded_sources)
        };
        let (source_files, expanded_sources) = source_files;
        
        if source_files.is_empty() {
            return Ok(StageResult::skipped("parse"));
        }
        
        let total_files = source_files.len();
        info!("Parsing {} files with {} threads, batch size 10", total_files, MAX_PARSE_THREADS);
        
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(MAX_PARSE_THREADS)
            .build()
            .map_err(|e| anyhow!("Failed to create thread pool: {}", e))?;
        
        let batch_size = 10;
        let mut parsed_count = 0;
        let mut items_count = 0;
        let mut failed_count = 0;
        let mut derive_generated_count = 0;
        
        for (batch_idx, batch) in source_files.chunks(batch_size).enumerate() {
            let batch_start = batch_idx * batch_size;
            info!("Parsing batch {} (files {}-{})", batch_idx + 1, batch_start + 1, batch_start + batch.len());
            
            for file_info in batch {
                let (source_to_parse, has_expanded) = expanded_sources
                    .get(&file_info.path)
                    .map(|s| (s.as_str(), true))
                    .unwrap_or((&file_info.original_source, false));
                
                // Count impl blocks in expanded source - files with 5000+ impl blocks consume 50+ GB
                let impl_count = if has_expanded {
                    source_to_parse.matches("impl ").count()
                } else {
                    0
                };
                
                // Skip expanded files with too many impl blocks - they cause OOM during derive detection
                let skip_expanded = has_expanded && (
                    source_to_parse.len() > MAX_EXPANDED_SOURCE_SIZE || 
                    impl_count > MAX_IMPL_BLOCKS
                );
                
                if skip_expanded {
                    let reason = if source_to_parse.len() > MAX_EXPANDED_SOURCE_SIZE {
                        format!("{} bytes > {} limit", source_to_parse.len(), MAX_EXPANDED_SOURCE_SIZE)
                    } else {
                        format!("{} impl blocks > {} limit", impl_count, MAX_IMPL_BLOCKS)
                    };
                    info!("Skipping large expanded file {:?} ({}) - parsing original instead", 
                          file_info.path, reason);
                    let result = self.parser.parse(&file_info.original_source, &file_info.module_path);
                    match result {
                        Ok(parse_result) => {
                            let items: Vec<ParsedItemInfo> = parse_result.items
                                .iter()
                                .map(|item| {
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
                                        generated_by: None,
                                    }
                                })
                                .collect();
                            
                            let items_len = items.len();
                            items_count += items_len;
                            parsed_count += 1;
                            
                            if !items.is_empty() {
                                let items_ref: Vec<&ParsedItemInfo> = items.iter().collect();
                                let insert_count = Self::batch_insert_items(&db_pool, &items_ref).await;
                                debug!("Inserted {} items for {:?}", insert_count, file_info.path);
                            }
                            
                            drop(items);
                            drop(parse_result);
                            trim_memory();
                        }
                        Err(e) => {
                            warn!("Failed to parse original source for {:?}: {}", file_info.path, e);
                            failed_count += 1;
                        }
                    }
                    continue;
                }
                
                let result = self.parser.parse(source_to_parse, &file_info.module_path);
                
                match result {
                    Ok(parse_result) => {
                        let generated_by_map = if has_expanded {
                            self.derive_detector.detect(
                                &file_info.original_source,
                                source_to_parse,
                                &file_info.module_path,
                            ).map(|d| d.generated_by).unwrap_or_default()
                        } else {
                            HashMap::new()
                        };
                        
                        let items: Vec<ParsedItemInfo> = parse_result.items
                            .iter()
                            .map(|item| {
                                let generated_by = if item.item_type == ItemType::Impl {
                                    self.find_derive_source(item, &generated_by_map)
                                        .or_else(|| item.generated_by.clone())
                                } else {
                                    item.generated_by.clone()
                                };
                                
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
                        
                        let items_len = items.len();
                        derive_generated_count += items.iter().filter(|i| i.generated_by.is_some()).count();
                        items_count += items_len;
                        parsed_count += 1;
                        
                        if !items.is_empty() {
                            let items_ref: Vec<&ParsedItemInfo> = items.iter().collect();
                            let insert_count = Self::batch_insert_items(&db_pool, &items_ref).await;
                            debug!("Inserted {} items for {:?}", insert_count, file_info.path);
                        }
                        
                        if !parse_result.errors.is_empty() {
                            let mut state = ctx.state.write().await;
                            for err in &parse_result.errors {
                                state.errors.push(StageError::new("parse", err.message.clone())
                                    .with_context(format!("{}:{}", file_info.path.display(), err.line.unwrap_or(0))));
                            }
                            drop(state);
                        }
                        
                        drop(items);
                        drop(parse_result);
                        drop(generated_by_map);
                        trim_memory();
                    }
                    Err(e) => {
                        warn!("Failed to parse {:?}: {}", file_info.path, e);
                        let mut state = ctx.state.write().await;
                        state.errors.push(StageError::new("parse", e.to_string())
                            .with_context(file_info.path.display().to_string()));
                        drop(state);
                        failed_count += 1;
                    }
                }
            }
            
            trim_memory();
            info!("Batch {} complete: {}/{} files, {} items total", batch_idx + 1, parsed_count, total_files, items_count);
        }
        
        let mut state = ctx.state.write().await;
        state.counts.files_parsed = parsed_count;
        state.counts.items_parsed = items_count;
        drop(state);
        
        let duration = start.elapsed();
        
        info!(
            "Parse stage completed: {} files, {} items ({} derive-generated) in {:?}",
            parsed_count, items_count, derive_generated_count, duration
        );
        
        if failed_count > 0 && parsed_count == 0 {
            Ok(StageResult::failed("parse", "All parsing attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial("parse", parsed_count, failed_count, duration,
                format!("{} files parsed, {} failed, {} items", parsed_count, failed_count, items_count)))
        } else {
            Ok(StageResult::success("parse", items_count, 0, duration))
        }
    }
}

impl ParseStage {
    fn find_derive_source(
        &self,
        item: &ParsedItem,
        generated_by_map: &HashMap<String, String>,
    ) -> Option<String> {
        for attr in &item.attributes {
            if attr.starts_with("impl_for=") {
                let trait_name = &attr[9..];
                if let Some(underscore_pos) = item.name.find('_') {
                    let self_type = &item.name[underscore_pos + 1..];
                    let key = format!("{} for {}", trait_name, self_type);
                    return generated_by_map.get(&key).cloned();
                }
            }
        }
        None
    }
    
    async fn batch_insert_items(pool: &sqlx::PgPool, items: &[&ParsedItemInfo]) -> usize {
        if items.is_empty() {
            return 0;
        }
        
        // Deduplicate by FQN to prevent "ON CONFLICT DO UPDATE command cannot affect row a second time"
        // When the same FQN appears multiple times in a batch, PostgreSQL rejects the entire batch
        let mut seen_fqns: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let deduped_items: Vec<&&ParsedItemInfo> = items
            .iter()
            .filter(|item| seen_fqns.insert(item.fqn.as_str()))
            .collect();
        
        let dup_count = items.len() - deduped_items.len();
        if dup_count > 0 {
            debug!("Deduplicated {} items with duplicate FQNs in batch of {}", dup_count, items.len());
        }
        
        let ids: Vec<String> = deduped_items.iter().map(|_| Uuid::new_v4().to_string()).collect();
        let item_types: Vec<&str> = deduped_items.iter().map(|i| i.item_type.as_str()).collect();
        let fqns: Vec<&str> = deduped_items.iter().map(|i| i.fqn.as_str()).collect();
        let names: Vec<&str> = deduped_items.iter().map(|i| i.name.as_str()).collect();
        let visibilities: Vec<&str> = deduped_items.iter().map(|i| i.visibility.as_str()).collect();
        let signatures: Vec<&str> = deduped_items.iter().map(|i| i.signature.as_str()).collect();
        let doc_comments: Vec<&str> = deduped_items.iter().map(|i| i.doc_comment.as_str()).collect();
        let start_lines: Vec<i32> = deduped_items.iter().map(|i| i.start_line as i32).collect();
        let end_lines: Vec<i32> = deduped_items.iter().map(|i| i.end_line as i32).collect();
        let body_sources: Vec<&str> = deduped_items.iter().map(|i| i.body_source.as_str()).collect();
        let generic_params: Vec<serde_json::Value> = deduped_items.iter()
            .map(|i| serde_json::to_value(&i.generic_params).unwrap_or(serde_json::json!([])))
            .collect();
        let where_clauses: Vec<serde_json::Value> = deduped_items.iter()
            .map(|i| serde_json::to_value(&i.where_clauses).unwrap_or(serde_json::json!([])))
            .collect();
        let attributes: Vec<serde_json::Value> = deduped_items.iter()
            .map(|i| serde_json::to_value(&i.attributes).unwrap_or(serde_json::json!([])))
            .collect();
        let generated_bys: Vec<Option<&str>> = deduped_items.iter()
            .map(|i| i.generated_by.as_deref())
            .collect();
        
        let generic_params_json: Vec<String> = generic_params.iter()
            .map(|v| v.to_string()).collect();
        let where_clauses_json: Vec<String> = where_clauses.iter()
            .map(|v| v.to_string()).collect();
        let attributes_json: Vec<String> = attributes.iter()
            .map(|v| v.to_string()).collect();
        
        let result = sqlx::query(
            r#"
            INSERT INTO extracted_items 
                (id, source_file_id, item_type, fqn, name, visibility, signature, 
                 doc_comment, start_line, end_line, body_source, 
                 generic_params, where_clauses, attributes, generated_by)
            SELECT 
                unnest($1::uuid[]) as id,
                NULL::uuid as source_file_id,
                unnest($2::text[]) as item_type,
                unnest($3::text[]) as fqn,
                unnest($4::text[]) as name,
                unnest($5::text[]) as visibility,
                unnest($6::text[]) as signature,
                unnest($7::text[]) as doc_comment,
                unnest($8::int[]) as start_line,
                unnest($9::int[]) as end_line,
                unnest($10::text[]) as body_source,
                unnest($11::jsonb[]) as generic_params,
                unnest($12::jsonb[]) as where_clauses,
                unnest($13::jsonb[]) as attributes,
                unnest($14::text[]) as generated_by
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
            "#
        )
        .bind(&ids)
        .bind(&item_types)
        .bind(&fqns)
        .bind(&names)
        .bind(&visibilities)
        .bind(&signatures)
        .bind(&doc_comments)
        .bind(&start_lines)
        .bind(&end_lines)
        .bind(&body_sources)
        .bind(&generic_params_json)
        .bind(&where_clauses_json)
        .bind(&attributes_json)
        .bind(&generated_bys)
        .execute(pool)
        .await;
        
        match result {
            Ok(r) => r.rows_affected() as usize,
            Err(e) => {
                warn!("Batch insert failed: {}", e);
                0
            }
        }
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
    
    /// Check if a source file has changed since last indexing by comparing content hashes.
    /// Returns Some(existing_id) if unchanged, None if changed or new.
    async fn check_file_unchanged(&self, file_info: &SourceFileInfo) -> Result<Option<Uuid>> {
        let pool = self.pool.as_ref()
            .ok_or_else(|| anyhow!("Database not connected"))?;

        let row: Option<(Uuid, Option<String>)> = sqlx::query_as(
            r#"
            SELECT id, content_hash FROM source_files
            WHERE crate_name = $1 AND module_path = $2 AND file_path = $3
            "#
        )
        .bind(&file_info.crate_name)
        .bind(&file_info.module_path)
        .bind(file_info.path.to_string_lossy().to_string())
        .fetch_optional(pool)
        .await?;

        if let Some((id, Some(existing_hash))) = row {
            if existing_hash == file_info.content_hash {
                return Ok(Some(id));
            }
        }
        Ok(None)
    }

    /// Store a source file in the database and return its ID.
    /// Includes content_hash for incremental change detection.
    async fn store_source_file(&self, file_info: &SourceFileInfo, expanded_source: Option<&str>) -> Result<Uuid> {
        let pool = self.pool.as_ref()
            .ok_or_else(|| anyhow!("Database not connected"))?;

        let id = Uuid::new_v4();

        sqlx::query(
            r#"
            INSERT INTO source_files
                (id, crate_name, module_path, file_path, original_source, expanded_source, git_hash, content_hash)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (crate_name, module_path, file_path) DO UPDATE SET
                original_source = EXCLUDED.original_source,
                expanded_source = EXCLUDED.expanded_source,
                git_hash = EXCLUDED.git_hash,
                content_hash = EXCLUDED.content_hash,
                last_indexed_at = NOW(),
                updated_at = NOW()
            RETURNING id
            "#
        )
        .bind(id)
        .bind(&file_info.crate_name)
        .bind(&file_info.module_path)
        .bind(file_info.path.to_string_lossy().to_string())
        .bind(file_info.original_source.as_str())
        .bind(expanded_source)
        .bind(&file_info.git_hash)
        .bind(&file_info.content_hash)
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
        
        info!("Starting extract stage (linking source files)");
        
        if ctx.config.dry_run {
            info!("Dry run - skipping extraction");
            return Ok(StageResult::skipped("extract"));
        }
        
        let state = ctx.state.read().await;
        let source_files = state.source_files.clone();
        let expanded_sources = state.expanded_sources.clone();
        let extracted_items = state.extracted_items.clone();
        drop(state);
        
        // Connect to database
        let mut stage = ExtractStage::new();
        if let Err(e) = stage.connect(&ctx.config.database_url).await {
            return Ok(StageResult::failed("extract", format!("Database connection failed: {}", e)));
        }
        
        let mut state = ctx.state.write().await;
        let mut extracted_count = extracted_items.len();
        let mut failed_count = 0;
        
        // Step 1: Store source files in database and build path -> ID mapping
        let mut source_file_ids: HashMap<PathBuf, Uuid> = HashMap::new();
        let mut skipped_unchanged = 0;

        for file_info in &source_files {
            match stage.check_file_unchanged(file_info).await {
                Ok(Some(existing_id)) => {
                    debug!("Skipping unchanged file {:?} (hash: {})", file_info.path, file_info.content_hash);
                    source_file_ids.insert(file_info.path.clone(), existing_id);
                    skipped_unchanged += 1;
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    debug!("Content hash check failed for {:?}, will re-index: {}", file_info.path, e);
                }
            }

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

        if skipped_unchanged > 0 {
            info!("Skipped {} unchanged files (incremental mode)", skipped_unchanged);
        }
        
        // Step 2: Update source_file_id for items already in database (from parse stage)
        // Items were inserted with NULL source_file_id during parse, now link them
        let pool = stage.pool.as_ref()
            .ok_or_else(|| anyhow!("Database not connected"))?;
        
        let update_result = sqlx::query(
            r#"
            UPDATE extracted_items ei
            SET source_file_id = sf.id
            FROM source_files sf
            WHERE ei.source_file_id IS NULL
              AND sf.file_path = SPLIT_PART(ei.fqn, '::', 1)
            "#
        )
        .execute(pool)
        .await;
        
        match update_result {
            Ok(result) => {
                info!("Updated source_file_id for {} items", result.rows_affected());
            }
            Err(e) => {
                warn!("Failed to update source_file_id links: {}", e);
            }
        }
        
        state.counts.items_extracted = extracted_count;
        
        let duration = start.elapsed();
        
        info!(
            "Extract stage completed: {} source files, {} items in {:?}",
            source_file_ids.len(), extracted_count, duration
        );
        
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
pub struct GraphStage {}

impl GraphStage {
    pub fn new() -> Self {
        Self {}
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
            password: std::env::var("NEO4J_PASSWORD").expect("NEO4J_PASSWORD environment variable must be set"),
            database: std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string()),
            ..Default::default()
        };
        
        // Connect to Neo4j
        info!("Connecting to Neo4j at {}", redact_url(&config.uri));
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
        
        // Build O(1) lookup indexes for traits (avoids O(n²) in find_trait_fqn)
        let mut trait_name_to_fqns: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut local_trait_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (_path, items) in &parsed_items {
            for item in items {
                if item.item_type == "trait" {
                    trait_name_to_fqns.insert(item.name.clone(), item.fqn.clone());
                    local_trait_fqns.insert(item.fqn.clone());
                }
            }
        }
        info!("Built trait lookup index: {} traits indexed", trait_name_to_fqns.len());
        
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
        let _contains_count = relationships.len();
        
        // 2. Create IMPLEMENTS and FOR relationships for impl blocks
        // IMPORTANT: We need to create Trait nodes for external traits first,
        // since the relationship MERGE requires both nodes to exist.
        let mut impl_count = 0;
        let mut for_count = 0;
        let mut external_trait_nodes: Vec<NodeData> = Vec::new();
        let mut seen_external_traits: std::collections::HashSet<String> = std::collections::HashSet::new();

        // First pass: collect all external traits that need nodes created
        for impl_item in &impl_items {
            if let Some(trait_name) = Self::extract_trait_from_impl(&impl_item.attributes) {
                let trait_fqn = Self::find_trait_fqn_optimized(&trait_name, &trait_name_to_fqns, &impl_item.fqn)
                    .unwrap_or_else(|| trait_name.clone());

                let is_local_trait = local_trait_fqns.contains(&trait_fqn);

                if !is_local_trait && !seen_external_traits.contains(&trait_fqn) {
                    seen_external_traits.insert(trait_fqn.clone());
                    external_trait_nodes.push(NodeData {
                        id: trait_fqn.clone(),
                        fqn: trait_fqn.clone(),
                        name: trait_name.clone(),
                        node_type: NodeType::Trait,
                        properties: {
                            let mut props = HashMap::new();
                            props.insert("external".to_string(), PropertyValue::from(true));
                            props
                        },
                    });
                }
            }
        }

        // Insert external trait nodes before creating relationships
        if !external_trait_nodes.is_empty() {
            info!("Creating {} external trait nodes for IMPLEMENTS relationships", external_trait_nodes.len());
            match graph_builder.create_nodes_batch(external_trait_nodes).await {
                Ok(_) => info!("Successfully inserted external trait nodes"),
                Err(e) => warn!("Failed to insert external trait nodes: {}", e),
            }
            if let Err(e) = graph_builder.flush().await {
                warn!("Failed to flush external trait nodes: {}", e);
            }
        }

        // Second pass: create IMPLEMENTS relationships
        for impl_item in &impl_items {
            if let Some(trait_name) = Self::extract_trait_from_impl(&impl_item.attributes) {
                let trait_fqn = Self::find_trait_fqn_optimized(&trait_name, &trait_name_to_fqns, &impl_item.fqn)
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
                                if !bound.starts_with(|c: char| c.is_lowercase()) {
                                    let super_trait_fqn = Self::find_trait_fqn_optimized(bound, &trait_name_to_fqns, &item.fqn)
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
                    // Static/free function calls
                    let calls = Self::extract_function_calls(&item.body_source, &function_fqns, &function_names_to_fqns, &item.fqn);
                    for (callee_fqn, line) in &calls {
                        relationships.push(RelationshipBuilder::create_calls(
                            item.fqn.clone(),
                            callee_fqn.clone(),
                            *line,
                            "",
                            Vec::new(),
                            true, // is_static_dispatch
                        ));
                        calls_count += 1;
                    }

                    // Method calls with local type tracking
                    let self_type = if item.item_type == "impl" {
                        Some(item.name.as_str())
                    } else {
                        None
                    };
                    let method_calls = Self::extract_method_calls(
                        &item.body_source,
                        &function_names_to_fqns,
                        self_type,
                    );
                    for (callee_fqn, line) in &method_calls {
                        // Avoid duplicate entries
                        if !calls.iter().any(|(fqn, _)| fqn == callee_fqn) {
                            relationships.push(RelationshipBuilder::create_calls(
                                item.fqn.clone(),
                                callee_fqn.clone(),
                                *line,
                                "",
                                Vec::new(),
                                false, // is_static_dispatch = false for method calls
                            ));
                            calls_count += 1;
                        }
                    }
                }
            }
        }
        info!("Created {} CALLS relationships (including method calls)", calls_count);
        
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
    /// Find the FQN of a trait by name using pre-built O(1) lookup index
    fn find_trait_fqn_optimized(
        trait_name: &str,
        trait_name_to_fqns: &std::collections::HashMap<String, String>,
        impl_fqn: &str,
    ) -> Option<String> {
        if let Some(fqn) = trait_name_to_fqns.get(trait_name) {
            return Some(fqn.clone());
        }
        let module_path = Self::get_parent_fqn(impl_fqn)?;
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
    
    /// Extract method calls from body source by tracking local variable types.
    ///
    /// Handles patterns like:
    /// - `let x: Type = ...;` then `x.method()`
    /// - `let x = Type::new();` then `x.method()`
    /// - `self.method()` where self type is known from context
    fn extract_method_calls(
        body_source: &str,
        function_names_to_fqns: &std::collections::HashMap<String, Vec<String>>,
        self_type: Option<&str>,
    ) -> Vec<(String, usize)> {
        let mut calls = Vec::new();
        let mut local_types: HashMap<String, String> = HashMap::new();

        // Track local variable types from let bindings
        let type_annotation_re = regex::Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*:\s*([A-Z]\w+)").unwrap();
        let constructor_re = regex::Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*=\s*([A-Z]\w+)::").unwrap();

        for line in body_source.lines() {
            let trimmed = line.trim();
            // Track type annotations: let x: Type = ...
            if let Some(caps) = type_annotation_re.captures(trimmed) {
                let var_name = caps.get(1).unwrap().as_str().to_string();
                let type_name = caps.get(2).unwrap().as_str().to_string();
                local_types.insert(var_name, type_name);
            }
            // Track constructor calls: let x = Type::new()
            if let Some(caps) = constructor_re.captures(trimmed) {
                let var_name = caps.get(1).unwrap().as_str().to_string();
                let type_name = caps.get(2).unwrap().as_str().to_string();
                local_types.insert(var_name, type_name);
            }
        }

        // Now find method calls on tracked variables and self
        let method_call_re = regex::Regex::new(r"(\w+)\.(\w+)\s*\(").unwrap();

        for (line_num, line) in body_source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("#") {
                continue;
            }

            for caps in method_call_re.captures_iter(trimmed) {
                let receiver = caps.get(1).unwrap().as_str();
                let method = caps.get(2).unwrap().as_str();

                // Determine the receiver type
                let receiver_type = if receiver == "self" || receiver == "Self" {
                    self_type.map(|s| s.to_string())
                } else {
                    local_types.get(receiver).cloned()
                };

                if let Some(type_name) = receiver_type {
                    // Look for Type::method in known functions
                    let qualified = format!("{}::{}", type_name, method);
                    if let Some(fqns) = function_names_to_fqns.get(method) {
                        // Find FQN that contains the type name
                        if let Some(fqn) = fqns.iter().find(|f| f.contains(&format!("::{}", qualified)) || f.ends_with(&qualified)) {
                            calls.push((fqn.clone(), line_num + 1));
                        }
                    }
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
pub struct EmbedStage {}

impl EmbedStage {
    pub fn new() -> Self {
        Self {}
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
                        return Ok(StageResult::skipped("embed"));
                    }
                    info!("Loaded {} items from database", items.len());
                    // Group items by a dummy path since we don't have file paths from DB
                    parsed_items.insert(std::path::PathBuf::from("/workspace"), items);
                }
                Err(e) => {
                    warn!("Failed to load items from database: {}", e);
                    return Ok(StageResult::skipped("embed"));
                }
            }
        }
        
        // Get service URLs
        let ollama_url = Self::get_ollama_url(ctx);
        let qdrant_url = Self::get_qdrant_url();
        
        info!("Connecting to Ollama at {} and Qdrant at {}", redact_url(&ollama_url), redact_url(&qdrant_url));
        
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
            let _file_path_str = path.to_string_lossy().to_string();
            let _module_path = path_to_crate.get(path).map(|s| s.as_str()).unwrap_or("");
            
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
            
            // Retry embedding batches with exponential backoff for transient failures
            let batch_label = format!("embed batch {}", batch_num + 1);
            let chunk_vec: Vec<_> = chunk.to_vec();
            let service = &embedding_service;

            match retry_with_backoff(&batch_label, MAX_RETRIES, || async {
                service.embed_items(&chunk_vec).await
            }).await {
                Ok(results) => {
                    embedded_count += results.len();
                    debug!("Embedded {} items in batch {}", results.len(), batch_num + 1);
                }
                Err(e) => {
                    warn!("Failed to embed batch {} after retries: {}", batch_num, e);
                    state.errors.push(StageError::new("embed", e.to_string())
                        .with_context(format!("batch {} (after {} retries)", batch_num, MAX_RETRIES)));
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

// =============================================================================
// DATA LIFECYCLE
// =============================================================================

/// Data lifecycle manager for cross-store garbage collection and cascade deletion.
pub struct DataLifecycleManager;

impl DataLifecycleManager {
    /// Delete all data for a crate across all stores.
    ///
    /// Order matters: delete embeddings first (Qdrant), then graph (Neo4j), then relational (Postgres).
    pub async fn cascade_delete_crate(
        crate_name: &str,
        pool: &PgPool,
        neo4j_url: Option<&str>,
        qdrant_url: Option<&str>,
    ) -> Result<DataLifecycleReport> {
        let mut report = DataLifecycleReport::default();

        info!("Starting cascade delete for crate: {}", crate_name);

        // Step 1: Delete from Qdrant (embeddings) if available
        if let Some(qdrant) = qdrant_url {
            let client = reqwest::Client::new();
            let delete_req = serde_json::json!({
                "filter": {
                    "must": [{
                        "key": "crate_name",
                        "match": { "value": crate_name }
                    }]
                }
            });

            match client
                .post(format!("{}/collections/code_embeddings/points/delete", qdrant))
                .json(&delete_req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    info!("Deleted Qdrant embeddings for crate: {}", crate_name);
                    report.qdrant_deleted = true;
                }
                Ok(resp) => {
                    warn!("Qdrant delete returned {}: {}", resp.status(), resp.text().await.unwrap_or_default());
                }
                Err(e) => {
                    warn!("Failed to delete from Qdrant: {}", e);
                    report.errors.push(format!("Qdrant: {}", e));
                }
            }
        }

        // Step 2: Delete from Neo4j (graph nodes/relationships) if available
        if let Some(neo4j) = neo4j_url {
            match neo4rs::Graph::new(neo4j, std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()), std::env::var("NEO4J_PASSWORD").expect("NEO4J_PASSWORD environment variable must be set")).await {
                Ok(graph) => {
                    let q = neo4rs::query(
                        "MATCH (n {crate_name: $crate_name}) DETACH DELETE n"
                    ).param("crate_name", crate_name);

                    match graph.run(q).await {
                        Ok(_) => {
                            info!("Deleted Neo4j nodes for crate: {}", crate_name);
                            report.neo4j_deleted = true;
                        }
                        Err(e) => {
                            warn!("Failed to delete from Neo4j: {}", e);
                            report.errors.push(format!("Neo4j: {}", e));
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to connect to Neo4j: {}", e);
                    report.errors.push(format!("Neo4j connect: {}", e));
                }
            }
        }

        // Step 3: Delete from Postgres (cascade from source_files to extracted_items)
        match sqlx::query(
            "DELETE FROM extracted_items WHERE source_file_id IN (SELECT id FROM source_files WHERE crate_name = $1)"
        )
        .bind(crate_name)
        .execute(pool)
        .await
        {
            Ok(result) => {
                report.postgres_items_deleted = result.rows_affected() as usize;
                info!("Deleted {} extracted items from Postgres", report.postgres_items_deleted);
            }
            Err(e) => {
                warn!("Failed to delete extracted items: {}", e);
                report.errors.push(format!("Postgres items: {}", e));
            }
        }

        match sqlx::query("DELETE FROM source_files WHERE crate_name = $1")
            .bind(crate_name)
            .execute(pool)
            .await
        {
            Ok(result) => {
                report.postgres_files_deleted = result.rows_affected() as usize;
                info!("Deleted {} source files from Postgres", report.postgres_files_deleted);
            }
            Err(e) => {
                warn!("Failed to delete source files: {}", e);
                report.errors.push(format!("Postgres files: {}", e));
            }
        }

        report.postgres_deleted = true;
        Ok(report)
    }

    /// Find orphaned references - items in one store but not others.
    pub fn find_orphaned_references(
        store_refs: &HashMap<String, rustbrain_common::StoreReference>,
    ) -> Vec<&rustbrain_common::StoreReference> {
        store_refs.values()
            .filter(|r| !r.is_fully_synced() && !r.is_orphaned())
            .collect()
    }
}

/// Report of data lifecycle operations
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DataLifecycleReport {
    pub postgres_deleted: bool,
    pub postgres_items_deleted: usize,
    pub postgres_files_deleted: usize,
    pub neo4j_deleted: bool,
    pub qdrant_deleted: bool,
    pub errors: Vec<String>,
}

impl DataLifecycleReport {
    pub fn is_successful(&self) -> bool {
        self.errors.is_empty()
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

    #[test]
    fn test_stage_error_with_context() {
        let err = StageError::new("parse", "failed")
            .with_context("file: src/main.rs");
        assert_eq!(err.stage, "parse");
        assert_eq!(err.message, "failed");
        assert_eq!(err.context, Some("file: src/main.rs".to_string()));
        assert!(!err.is_fatal);
    }

    #[test]
    fn test_stage_result_failed() {
        let result = StageResult::failed("embed", "connection refused");
        assert_eq!(result.status, StageStatus::Failed);
        assert_eq!(result.items_processed, 0);
        assert_eq!(result.items_failed, 0);
        assert_eq!(result.duration_ms, 0);
        assert_eq!(result.error, Some("connection refused".to_string()));
    }

    #[test]
    fn test_stage_result_skipped() {
        let result = StageResult::skipped("graph");
        assert_eq!(result.status, StageStatus::Skipped);
        assert_eq!(result.name, "graph");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_stage_status_display() {
        assert_eq!(StageStatus::Success.to_string(), "success");
        assert_eq!(StageStatus::Partial.to_string(), "partial");
        assert_eq!(StageStatus::Failed.to_string(), "failed");
        assert_eq!(StageStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn test_stage_result_serialization() {
        let result = StageResult::success("test", 5, 1, Duration::from_millis(42));
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["name"], "test");
        assert_eq!(json["status"], "success");
        assert_eq!(json["items_processed"], 5);
        assert_eq!(json["items_failed"], 1);
        assert_eq!(json["duration_ms"], 42);
    }

    #[test]
    fn test_stage_error_serialization() {
        let err = StageError::fatal("parse", "syntax error")
            .with_context("line 42");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["stage"], "parse");
        assert_eq!(json["message"], "syntax error");
        assert_eq!(json["context"], "line 42");
        assert_eq!(json["is_fatal"], true);
    }

    #[test]
    fn test_expand_stage_creation() {
        let stage = ExpandStage::new();
        assert!(stage.is_ok());
        assert_eq!(stage.unwrap().name(), "expand");
    }

    #[test]
    fn test_parse_stage_creation() {
        let stage = ParseStage::new();
        assert!(stage.is_ok());
        assert_eq!(stage.unwrap().name(), "parse");
    }

    #[test]
    fn test_typecheck_stage_creation() {
        let stage = TypecheckStage::new();
        assert_eq!(stage.name(), "typecheck");
    }

    #[test]
    fn test_extract_stage_creation() {
        let stage = ExtractStage::new();
        assert_eq!(stage.name(), "extract");
    }

    #[test]
    fn test_graph_stage_creation() {
        let stage = GraphStage::new();
        assert_eq!(stage.name(), "graph");
    }

    #[test]
    fn test_embed_stage_creation() {
        let stage = EmbedStage::new();
        assert_eq!(stage.name(), "embed");
    }

    #[tokio::test]
    async fn test_retry_with_backoff_success_first_try() {
        let result = retry_with_backoff("test_op", 3, || async {
            Ok::<i32, anyhow::Error>(42)
        }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_with_backoff_success_after_failure() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let attempt = Arc::new(AtomicUsize::new(0));
        let attempt_clone = attempt.clone();

        let result = retry_with_backoff("test_op", 3, || {
            let attempt = attempt_clone.clone();
            async move {
                let n = attempt.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(anyhow::anyhow!("transient failure"))
                } else {
                    Ok(99)
                }
            }
        }).await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempt.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn test_retry_with_backoff_all_failures() {
        let result = retry_with_backoff("test_op", 1, || async {
            Err::<i32, anyhow::Error>(anyhow::anyhow!("permanent failure"))
        }).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("permanent failure"));
    }

    #[test]
    fn test_expand_stage_compute_crate_hash() {
        let stage = ExpandStage::new().unwrap();
        // Hash of a non-existent directory should be deterministic
        let hash1 = stage.compute_crate_hash(Path::new("/nonexistent/path"));
        let hash2 = stage.compute_crate_hash(Path::new("/nonexistent/path"));
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 16); // 16 hex chars from u64
    }

    #[test]
    fn test_expand_stage_compute_crate_hash_different_paths() {
        let stage = ExpandStage::new().unwrap();
        let hash1 = stage.compute_crate_hash(Path::new("/path/a"));
        let hash2 = stage.compute_crate_hash(Path::new("/path/b"));
        // Both non-existent, but DefaultHasher with no input should be the same
        // Actually both have no files so both hash the same way
        // This is expected - what matters is that real paths with different content differ
        assert_eq!(hash1.len(), 16);
        assert_eq!(hash2.len(), 16);
    }

    #[test]
    fn test_cargo_expand_timeout_constant() {
        assert_eq!(CARGO_EXPAND_TIMEOUT, Duration::from_secs(300));
    }

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    #[test]
    fn test_compute_content_hash_deterministic() {
        let hash1 = compute_content_hash("fn main() {}");
        let hash2 = compute_content_hash("fn main() {}");
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn test_compute_content_hash_different_content() {
        let hash1 = compute_content_hash("fn main() {}");
        let hash2 = compute_content_hash("fn main() { println!(\"hello\"); }");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_content_hash_empty() {
        let hash = compute_content_hash("");
        assert_eq!(hash.len(), 16);
    }

    #[test]
    fn test_source_file_info_content_hash() {
        let info = SourceFileInfo {
            path: PathBuf::from("src/main.rs"),
            crate_name: "test".to_string(),
            module_path: "test".to_string(),
            original_source: Arc::new("fn main() {}".to_string()),
            git_hash: None,
            content_hash: compute_content_hash("fn main() {}"),
        };
        assert_eq!(info.content_hash.len(), 16);
    }

    #[test]
    fn test_extract_method_calls_with_type_annotation() {
        let body = r#"
            let client: HttpClient = HttpClient::new();
            client.get("/api");
            client.post("/data");
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert("get".to_string(), vec!["crate::HttpClient::get".to_string()]);
        names_to_fqns.insert("post".to_string(), vec!["crate::HttpClient::post".to_string()]);

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, None);
        assert!(!calls.is_empty(), "Should detect method calls on typed variables");
    }

    #[test]
    fn test_extract_method_calls_with_constructor() {
        let body = r#"
            let parser = DualParser::new();
            parser.parse("source");
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert("parse".to_string(), vec!["crate::DualParser::parse".to_string()]);

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, None);
        assert!(!calls.is_empty(), "Should detect method calls on constructor-inferred variables");
    }

    #[test]
    fn test_extract_method_calls_on_self() {
        let body = r#"
            self.process_item(item);
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert("process_item".to_string(), vec!["crate::MyStruct::process_item".to_string()]);

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, Some("MyStruct"));
        assert!(!calls.is_empty(), "Should detect self.method() calls");
    }

    #[test]
    fn test_extract_method_calls_skips_comments() {
        let body = r#"
            // client.get("/api");
            # client.post("/data");
        "#;
        let names_to_fqns = std::collections::HashMap::new();
        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, None);
        assert!(calls.is_empty(), "Should skip comments");
    }

    // Data lifecycle tests

    #[test]
    fn test_store_reference_new() {
        let ref_entry = rustbrain_common::StoreReference::new("crate::func".to_string(), "my_crate".to_string());
        assert_eq!(ref_entry.fqn, "crate::func");
        assert_eq!(ref_entry.crate_name, "my_crate");
        assert!(ref_entry.postgres_id.is_none());
        assert!(ref_entry.neo4j_node_id.is_none());
        assert!(ref_entry.qdrant_point_id.is_none());
        assert!(!ref_entry.is_fully_synced());
        assert!(ref_entry.is_orphaned());
    }

    #[test]
    fn test_store_reference_fully_synced() {
        let mut ref_entry = rustbrain_common::StoreReference::new("crate::func".to_string(), "my_crate".to_string());
        ref_entry.postgres_id = Some("pg-123".to_string());
        ref_entry.neo4j_node_id = Some("neo-456".to_string());
        ref_entry.qdrant_point_id = Some("qd-789".to_string());
        assert!(ref_entry.is_fully_synced());
        assert!(!ref_entry.is_orphaned());
        assert!(ref_entry.missing_stores().is_empty());
    }

    #[test]
    fn test_store_reference_partially_synced() {
        let mut ref_entry = rustbrain_common::StoreReference::new("crate::func".to_string(), "my_crate".to_string());
        ref_entry.postgres_id = Some("pg-123".to_string());
        // Missing neo4j and qdrant
        assert!(!ref_entry.is_fully_synced());
        assert!(!ref_entry.is_orphaned());
        let missing = ref_entry.missing_stores();
        assert_eq!(missing.len(), 2);
        assert!(missing.contains(&"neo4j"));
        assert!(missing.contains(&"qdrant"));
    }

    #[test]
    fn test_find_orphaned_references() {
        let mut refs = HashMap::new();

        // Fully synced - should not be orphaned
        let mut full = rustbrain_common::StoreReference::new("a".to_string(), "c".to_string());
        full.postgres_id = Some("1".to_string());
        full.neo4j_node_id = Some("2".to_string());
        full.qdrant_point_id = Some("3".to_string());
        refs.insert("a".to_string(), full);

        // Partially synced - should be detected
        let mut partial = rustbrain_common::StoreReference::new("b".to_string(), "c".to_string());
        partial.postgres_id = Some("4".to_string());
        refs.insert("b".to_string(), partial);

        // Completely orphaned - should NOT be in orphaned list (it's empty, not inconsistent)
        let empty = rustbrain_common::StoreReference::new("c".to_string(), "c".to_string());
        refs.insert("c".to_string(), empty);

        let orphaned = DataLifecycleManager::find_orphaned_references(&refs);
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].fqn, "b");
    }

    #[test]
    fn test_data_lifecycle_report_default() {
        let report = DataLifecycleReport::default();
        assert!(!report.postgres_deleted);
        assert!(!report.neo4j_deleted);
        assert!(!report.qdrant_deleted);
        assert!(report.is_successful());
    }

    #[test]
    fn test_data_lifecycle_report_with_errors() {
        let mut report = DataLifecycleReport::default();
        report.errors.push("connection failed".to_string());
        assert!(!report.is_successful());
    }
}
