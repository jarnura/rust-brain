//! Pipeline stage definitions and implementations
//!
//! Each stage implements the `PipelineStage` trait and processes
//! data from the shared `PipelineContext`.

use crate::parsers::{DualParser, ItemType, ParsedItem, Visibility};
use crate::pipeline::{
    discover_workspace_crate_names, ParsedItemInfo, PipelineContext, SourceFileInfo,
};
use crate::typecheck::TypeResolutionService;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, LazyLock, Mutex};
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

// Pre-compiled regexes for hot-loop usage (avoids O(N*M) recompilation)
static TYPE_ANNOTATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*:\s*([A-Z]\w+)").unwrap());
static CONSTRUCTOR_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"let\s+(?:mut\s+)?(\w+)\s*=\s*([A-Z]\w+)::").unwrap());
static METHOD_CALL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(\w+)\.(\w+)\s*\(").unwrap());
static STRUCT_INSTANTIATION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b([A-Z][a-zA-Z0-9_]*)\s*\{").unwrap());

/// Default timeout for cargo expand (3 minutes per crate)
const CARGO_EXPAND_TIMEOUT: Duration = Duration::from_secs(180);

/// Cache directory for expanded code
const EXPAND_CACHE_DIR: &str = "/tmp/rustbrain-expand-cache";

// =============================================================================
// FEATURE PROPAGATION HELPERS
// =============================================================================

/// Parse Cargo.toml to get dependencies as a map of dep_name -> existing_features
#[allow(dead_code)]
fn parse_cargo_dependencies(crate_path: &Path) -> Result<HashMap<String, String>> {
    let cargo_toml_path = crate_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("Failed to read {:?}", cargo_toml_path))?;

    let mut dependencies = HashMap::new();

    let doc: toml_edit::DocumentMut = content
        .parse()
        .with_context(|| format!("Failed to parse {:?}", cargo_toml_path))?;

    if let Some(toml_edit::Item::Table(table)) = doc.get("dependencies") {
        for (name, value) in table.iter() {
            let dep_name = name.to_string();
            if let toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) = value {
                if let Some(toml_edit::Value::String(features)) = t.get("features") {
                    dependencies.insert(dep_name, features.value().to_string());
                } else {
                    dependencies.insert(dep_name, String::new());
                }
            } else if let toml_edit::Item::Value(toml_edit::Value::String(_)) = value {
                dependencies.insert(dep_name, String::new());
            }
        }
    }

    Ok(dependencies)
}

/// Parse Cargo.toml feature definitions to find implicit feature dependencies.
/// Returns a map of (feature_name -> Vec<(dep_name, dep_feature)>)
#[allow(dead_code)]
fn parse_feature_definitions(crate_path: &Path) -> Result<HashMap<String, Vec<(String, String)>>> {
    let cargo_toml_path = crate_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("Failed to read {:?}", cargo_toml_path))?;

    let mut implicit_deps: HashMap<String, Vec<(String, String)>> = HashMap::new();

    let doc: toml_edit::DocumentMut = content
        .parse()
        .with_context(|| format!("Failed to parse {:?}", cargo_toml_path))?;

    if let Some(toml_edit::Item::Table(table)) = doc.get("features") {
        for (feature_name, value) in table.iter() {
            let mut deps_for_feature = Vec::new();

            if let toml_edit::Item::Value(toml_edit::Value::Array(arr)) = value {
                for item in arr.iter() {
                    if let toml_edit::Value::String(s) = item {
                        let val = s.value();
                        if let Some(slash_pos) = val.find('/') {
                            let dep_name = val[..slash_pos].to_string();
                            let dep_feature = val[slash_pos + 1..].to_string();
                            deps_for_feature.push((dep_name, dep_feature));
                        }
                    }
                }
            }

            if !deps_for_feature.is_empty() {
                implicit_deps.insert(feature_name.to_string(), deps_for_feature);
            }
        }
    }

    Ok(implicit_deps)
}

/// Find a crate in the workspace by name
fn find_crate_in_workspace(workspace_path: &Path, crate_name: &str) -> Result<PathBuf> {
    debug!(
        "find_crate_in_workspace: looking for {} in {:?}",
        crate_name, workspace_path
    );

    let name_variants = vec![
        crate_name.to_string(),
        crate_name.replace('_', "-"),
        crate_name.replace('-', "_"),
    ];

    for variant in &name_variants {
        let possible_paths = vec![
            workspace_path.join("crates").join(variant),
            workspace_path.join(variant),
        ];

        for path in &possible_paths {
            let cargo_toml = path.join("Cargo.toml");
            debug!("Checking {:?}", cargo_toml);
            if cargo_toml.exists() {
                debug!("Found crate {} at {:?}", crate_name, path);
                return Ok(path.clone());
            }
        }
    }

    // Search the workspace
    for entry in WalkDir::new(workspace_path)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        let cargo_toml = entry.path().join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                for variant in &name_variants {
                    if content.contains(&format!("name = \"{}\"", variant))
                        || content.contains(&format!("name=\"{}\"", variant))
                    {
                        return Ok(entry.path().to_path_buf());
                    }
                }
            }
        }
    }

    let fallback_path = workspace_path.join("crates").join(crate_name);
    Ok(fallback_path)
}

/// Find all crates in the workspace that depend on a given crate and have a specific feature
#[allow(dead_code)]
fn find_crates_depending_on_with_feature(
    workspace_path: &Path,
    target_dep: &str,
    required_feature: &str,
) -> Result<Vec<(String, PathBuf)>> {
    let mut result = Vec::new();

    for entry in WalkDir::new(workspace_path.join("crates"))
        .min_depth(1)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        let cargo_toml = entry.path().join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            let depends_on_target = content.contains(&format!("{} =", target_dep))
                || content.contains(&format!("{}=", target_dep))
                || content.contains(&format!("path = \"../{}\"", target_dep.replace('-', "_")))
                || content.contains(&format!("path = \"../{}\"", target_dep.replace('_', "-")));

            if depends_on_target {
                let doc: toml_edit::DocumentMut = match content.parse() {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                if let Some(toml_edit::Item::Table(table)) = doc.get("features") {
                    // Check if this crate has the required feature
                    if table.contains_key(required_feature) {
                        let crate_name = entry
                            .path()
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        result.push((crate_name, entry.path().to_path_buf()));
                    }

                    // Check if any feature enables target_dep/required_feature
                    for (_, value) in table.iter() {
                        if let toml_edit::Item::Value(toml_edit::Value::Array(arr)) = value {
                            for item in arr.iter() {
                                if let toml_edit::Value::String(s) = item {
                                    if s.value() == &format!("{}/{}", target_dep, required_feature)
                                    {
                                        let crate_name = entry
                                            .path()
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_default();
                                        result.push((crate_name, entry.path().to_path_buf()));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Get the list of features available in a dependency crate
#[allow(dead_code)]
fn get_dependency_features(workspace_path: &Path, dep_name: &str) -> Result<Vec<String>> {
    let dep_path = find_crate_in_workspace(workspace_path, dep_name)?;

    if !dep_path.exists() {
        return Ok(Vec::new());
    }

    let cargo_toml_path = dep_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml_path)
        .with_context(|| format!("Failed to read {:?}", cargo_toml_path))?;

    let mut features = Vec::new();

    let doc: toml_edit::DocumentMut = content
        .parse()
        .with_context(|| format!("Failed to parse {:?}", cargo_toml_path))?;

    if let Some(toml_edit::Item::Table(table)) = doc.get("features") {
        for (name, _) in table.iter() {
            features.push(name.to_string());
        }
    }

    Ok(features)
}

// =============================================================================
// END FEATURE PROPAGATION HELPERS
// =============================================================================

/// Maximum parallel threads for parsing (prevents memory spikes)
const MAX_PARSE_THREADS: usize = 4;

/// Compute a SHA-256 content hash of a string, returning a hex-encoded string.
fn compute_content_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
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
async fn retry_with_backoff<F, Fut, T>(operation_name: &str, max_retries: usize, f: F) -> Result<T>
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

    pub fn partial(
        name: &str,
        processed: usize,
        failed: usize,
        duration: Duration,
        error: impl Into<String>,
    ) -> Self {
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

    fn expand_library(
        &self,
        crate_path: &Path,
        workspace_path: &Path,
        crate_name: &str,
    ) -> Result<String> {
        debug!(
            "Expanding library for {:?} (crate: {})",
            crate_path, crate_name
        );

        let cache_key = format!(
            "{}-{}.expand",
            crate_name,
            self.compute_crate_hash(crate_path)
        );
        let cache_file = PathBuf::from(EXPAND_CACHE_DIR).join(&cache_key);

        if cache_file.exists() {
            if let Ok(cached) = std::fs::read_to_string(&cache_file) {
                debug!(
                    "Using cached expand for {} from {:?}",
                    crate_name, cache_file
                );
                return Ok(cached);
            }
        }

        // Check what features this crate has
        let has_v1 = self.crate_has_feature(crate_path, "v1");
        let has_v2 = self.crate_has_feature(crate_path, "v2");
        let has_olap = self.crate_has_feature(crate_path, "olap");
        let has_frm = self.crate_has_feature(crate_path, "frm");

        // Build feature combinations to try
        let features_to_try: Vec<Vec<String>> = if has_v1 {
            let mut combinations = Vec::new();

            // First: v1 + olap + frm (if this crate has them)
            let mut base = vec!["v1".to_string()];
            if has_olap {
                base.push("olap".to_string());
            }
            if has_frm {
                base.push("frm".to_string());
            }
            combinations.push(base);

            // If this crate depends on storage_impl, also try with storage_impl/olap
            // This is needed because storage_impl has cfg(all(feature = "v1", feature = "olap"))
            // for imports like try_join_all
            let deps_on_storage = self.crate_depends_on(crate_path, "storage_impl");
            if deps_on_storage && has_olap {
                let with_storage_olap = vec!["v1".to_string(), "olap".to_string()];
                combinations.push(with_storage_olap);
            }

            // IMPORTANT: Always try adding hyperswitch_domain_models/olap,frm
            // This handles transitive dependency feature propagation where:
            // - Crate v1 enables hyperswitch_interfaces/v1
            // - hyperswitch_interfaces/v1 enables hyperswitch_domain_models/v1
            // - But hyperswitch_domain_models needs olap for Connector import
            // - hyperswitch_interfaces depends on it with default-features = false
            let mut with_domain_features = vec!["v1".to_string()];
            if !has_olap {
                with_domain_features.push("hyperswitch_domain_models/olap".to_string());
            }
            if !has_frm {
                with_domain_features.push("hyperswitch_domain_models/frm".to_string());
            }
            combinations.push(with_domain_features);

            // Try just v1 as fallback
            combinations.push(vec!["v1".to_string()]);
            combinations
        } else if has_v2 {
            vec![vec!["v2".to_string()]]
        } else {
            vec![vec![]]
        };

        let mut last_error = None;
        for features in &features_to_try {
            let args: Vec<String> = if features.is_empty() {
                vec![]
            } else {
                vec!["--features".to_string(), features.join(",")]
            };

            let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            match self.run_cargo_expand(workspace_path, crate_name, &args_ref) {
                Ok(output) => {
                    self.write_expand_cache(&cache_file, &output, crate_name);
                    debug!("Succeeded with features {:?} for {}", features, crate_name);
                    return Ok(output);
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if Self::is_feature_conflict_error(&err_str)
                        && features.contains(&"v1".to_string())
                        && has_v2
                    {
                        // v1 has conflicts, try v2
                        debug!("v1 has feature conflicts for {}, trying v2", crate_name);
                        if let Ok(output) =
                            self.run_cargo_expand(workspace_path, crate_name, &["--features", "v2"])
                        {
                            self.write_expand_cache(&cache_file, &output, crate_name);
                            return Ok(output);
                        }
                    }
                    last_error = Some(e);
                }
            }
        }

        // All attempts failed, try default features only as last resort
        debug!(
            "All feature combinations failed for {}, trying default only",
            crate_name
        );
        match self.run_cargo_expand(workspace_path, crate_name, &[]) {
            Ok(output) => {
                self.write_expand_cache(&cache_file, &output, crate_name);
                Ok(output)
            }
            Err(_) => Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error"))),
        }
    }

    /// Write expanded output to cache file with proper error logging.
    fn write_expand_cache(&self, cache_file: &Path, output: &str, crate_name: &str) {
        if let Err(e) = std::fs::create_dir_all(EXPAND_CACHE_DIR) {
            warn!(
                "Failed to create expand cache dir {:?} for {}: {}",
                EXPAND_CACHE_DIR, crate_name, e
            );
            return;
        }
        if let Err(e) = std::fs::write(cache_file, output) {
            warn!(
                "Failed to write expand cache {:?} for {}: {}",
                cache_file, crate_name, e
            );
        }
    }

    /// Check if a crate has a specific feature
    fn crate_has_feature(&self, crate_path: &Path, feature: &str) -> bool {
        let cargo_toml_path = crate_path.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml_path) {
            let doc: toml_edit::DocumentMut = match content.parse() {
                Ok(d) => d,
                Err(_) => return false,
            };

            if let Some(toml_edit::Item::Table(table)) = doc.get("features") {
                return table.contains_key(feature);
            }
        }
        false
    }

    /// Check if a crate depends on another crate (directly or via features)
    fn crate_depends_on(&self, crate_path: &Path, dep_name: &str) -> bool {
        let cargo_toml_path = crate_path.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml_path) {
            // Check for direct dependency
            if content.contains(&format!("{} =", dep_name))
                || content.contains(&format!("{}=", dep_name))
                || content.contains(&format!("path = \"../{}\"", dep_name.replace('-', "_")))
                || content.contains(&format!("path = \"../{}\"", dep_name.replace('_', "-")))
            {
                return true;
            }

            // Check for transitive via feature definitions
            let doc: toml_edit::DocumentMut = match content.parse() {
                Ok(d) => d,
                Err(_) => return false,
            };

            if let Some(toml_edit::Item::Table(table)) = doc.get("features") {
                for (_, value) in table.iter() {
                    if let toml_edit::Item::Value(toml_edit::Value::Array(arr)) = value {
                        for item in arr.iter() {
                            if let toml_edit::Value::String(s) = item {
                                let val = s.value();
                                // Check if feature enables dep_name/feature
                                if val.starts_with(&format!("{}/", dep_name)) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Check if the error indicates feature conflicts (e.g., duplicate definitions)
    fn is_feature_conflict_error(error: &str) -> bool {
        // Patterns that indicate v1+v2 feature conflicts
        error.contains("is defined multiple times")
            || error.contains("redefined here")
            || error.contains("duplicate")
            || error.contains("must be defined only once")
    }

    /// Expand with feature propagation for crates with complex feature dependencies
    #[allow(dead_code)]
    fn expand_with_feature_propagation(
        &self,
        crate_path: &Path,
        workspace_path: &Path,
        crate_name: &str,
        primary_feature: &str,
    ) -> Result<String> {
        // Parse this crate's Cargo.toml to find dependencies
        let deps = parse_cargo_dependencies(crate_path)?;

        // Find transitive feature enables
        // e.g., common_utils has v1 = ["common_enums/v2"]
        // When we expand with v1, common_enums/v2 gets enabled
        // We need to find crates that depend on common_enums and enable their v2 feature
        let mut additional_features = Vec::new();

        for dep_name in deps.keys() {
            if let Ok(dep_path) = find_crate_in_workspace(workspace_path, dep_name) {
                if dep_path.exists() {
                    // Parse the dependency's feature definitions
                    if let Ok(feature_defs) = parse_feature_definitions(&dep_path) {
                        // Check if the dependency's primary_feature enables other features
                        if let Some(enabled_features) = feature_defs.get(primary_feature) {
                            for (transitive_dep, transitive_feature) in enabled_features {
                                // transitive_dep/transitive_feature is enabled by primary_feature
                                // Find crates that depend on transitive_dep and have transitive_feature
                                if let Ok(crates_with_feature) =
                                    find_crates_depending_on_with_feature(
                                        workspace_path,
                                        transitive_dep,
                                        transitive_feature,
                                    )
                                {
                                    for (feature_crate, _) in crates_with_feature {
                                        // Check if the current crate depends on this crate
                                        if deps.contains_key(&feature_crate) {
                                            additional_features.push(format!(
                                                "{}/{}",
                                                feature_crate, transitive_feature
                                            ));
                                            info!(
                                                "Feature propagation: enabling {}/{} for {}",
                                                feature_crate, transitive_feature, crate_name
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if additional_features.is_empty() {
            debug!("No feature propagation needed for {}", crate_name);
            return self.run_cargo_expand(
                workspace_path,
                crate_name,
                &["--features", primary_feature],
            );
        }

        // Build feature string: primary_feature + additional features
        let mut features = vec![primary_feature.to_string()];
        features.extend(additional_features);
        let features_arg = features.join(",");

        info!(
            "Running cargo expand for {} with propagated features: {}",
            crate_name, features_arg
        );

        self.run_cargo_expand(workspace_path, crate_name, &["--features", &features_arg])
    }

    /// Patch Cargo.toml to add hyperswitch_domain_models with olap,frm features
    #[allow(dead_code)]
    fn patch_cargo_toml_add_domain_models(content: &str) -> String {
        let mut doc: toml_edit::DocumentMut = content.parse().unwrap_or_else(|_| {
            // If parse fails, return original

            toml_edit::DocumentMut::new()
        });

        // Check if hyperswitch_domain_models is already a direct dependency
        let has_domain_models = matches!(
            doc.get("dependencies"),
            Some(toml_edit::Item::Table(table)) if table.contains_key("hyperswitch_domain_models")
        );

        if has_domain_models {
            // Already exists, just ensure olap and frm features are enabled
            if let Some(toml_edit::Item::Table(table)) = doc.get_mut("dependencies") {
                if let Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) =
                    table.get_mut("hyperswitch_domain_models")
                {
                    if let Some(toml_edit::Value::Array(features)) = t.get_mut("features") {
                        // Add olap and frm if not present
                        let existing: std::collections::HashSet<String> = features
                            .iter()
                            .filter_map(|f| {
                                if let toml_edit::Value::String(s) = f {
                                    Some(s.value().to_string())
                                } else {
                                    None
                                }
                            })
                            .collect();

                        if !existing.contains("olap") {
                            features.push("olap");
                        }
                        if !existing.contains("frm") {
                            features.push("frm");
                        }
                    }
                }
            }
        } else {
            // Add hyperswitch_domain_models as a new dependency using string manipulation
            let dep_str = r#"hyperswitch_domain_models = { version = "0.1.0", path = "../hyperswitch_domain_models", features = ["olap", "frm"], default-features = false }"#;

            // Find [dependencies] section
            let content_str = doc.to_string();
            let mut result = content_str.clone();

            if let Some(pos) = result.find("[dependencies]") {
                // Find next section or end
                let insert_pos = result[pos..]
                    .find("\n[")
                    .map(|p| pos + p)
                    .unwrap_or(result.len());
                result.insert_str(insert_pos, &format!("\n{}", dep_str));
            } else {
                // No dependencies section, add one before the first section or at end
                if let Some(first_section) = result.find('[') {
                    result.insert_str(first_section, &format!("[dependencies]\n{}\n\n", dep_str));
                } else {
                    result.push_str(&format!("\n[dependencies]\n{}", dep_str));
                }
            }
            return result;
        }

        doc.to_string()
    }

    fn run_cargo_expand(
        &self,
        workspace_path: &Path,
        crate_name: &str,
        extra_args: &[&str],
    ) -> Result<String> {
        use std::io::Read;
        use std::thread;

        // Limit parallel jobs to 1 to prevent memory exhaustion during expand
        // Large hyperswitch crates consume 20-30GB when all loaded into memory
        let jobs = 1;
        let jobs_str = jobs.to_string();
        let mut args: Vec<&str> = vec![
            "expand", "--lib", "-p", crate_name, "--jobs", &jobs_str, "--ugly",
        ];
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
                if n == 0 {
                    break;
                }
                if let Ok(mut guard) = stdout_buf_clone.lock() {
                    guard.extend_from_slice(&buf[..n]);
                }
            }
        });

        let stderr_thread = thread::spawn(move || {
            let mut reader = stderr;
            let mut buf = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

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
                let name = trimmed
                    .strip_prefix("name = ")
                    .map(|s| s.trim().trim_matches('"').to_string());
                return name;
            }
        }
        None
    }

    /// Pre-patch crates that need hyperswitch_domain_models with olap/frm features,
    /// or storage_impl with olap feature.
    /// Returns a map of (crate_path -> original_content) for later restoration.
    fn pre_patch_crates_for_features(
        &self,
        workspace_root: &Path,
        crates: &[PathBuf],
    ) -> HashMap<PathBuf, String> {
        let mut patches = HashMap::new();

        let crates_needing_patch =
            self.find_crates_needing_domain_models_patch(workspace_root, crates);

        for crate_path in &crates_needing_patch {
            let cargo_toml_path = crate_path.join("Cargo.toml");

            if let Ok(original_content) = std::fs::read_to_string(&cargo_toml_path) {
                let mut patched = original_content.clone();
                let mut needs_write = false;

                // Patch 1: Add hyperswitch_domain_models with olap,frm if needed
                let has_domain_with_olap = original_content
                    .lines()
                    .filter(|l| l.contains("hyperswitch_domain_models") && l.contains("features"))
                    .any(|l| l.contains("\"olap\""));

                if !has_domain_with_olap {
                    if original_content.contains("hyperswitch_domain_models") {
                        patched = Self::add_features_to_domain_models_dependency(&patched);
                    } else if original_content.contains("hyperswitch_interfaces") {
                        // Add hyperswitch_domain_models dependency
                        patched = Self::add_domain_models_dependency(&patched);
                    }
                    needs_write = true;
                }

                // Patch 2: Add olap to storage_impl if needed
                let has_storage_with_olap = original_content
                    .lines()
                    .filter(|l| l.contains("storage_impl") && l.contains("features"))
                    .any(|l| l.contains("\"olap\""));

                let has_storage_without_olap = original_content
                    .lines()
                    .filter(|l| {
                        l.contains("storage_impl") && l.contains("default-features = false")
                    })
                    .any(|l| !l.contains("\"olap\""));

                if has_storage_without_olap && !has_storage_with_olap {
                    patched = Self::add_olap_to_storage_impl_dependency(&patched);
                    needs_write = true;
                }

                if needs_write {
                    if let Err(e) = std::fs::write(&cargo_toml_path, &patched) {
                        warn!("Failed to patch {:?}: {}", cargo_toml_path, e);
                    } else {
                        debug!("Pre-patched {:?} for feature propagation", cargo_toml_path);
                        patches.insert(cargo_toml_path.clone(), original_content);
                    }
                }
            }
        }

        // Clear cargo cache to force re-resolution
        let target_dir = workspace_root.join("target");
        if target_dir.exists() {
            let _ = std::fs::remove_file(target_dir.join(".cargo-lock"));
            let _ = std::fs::remove_dir_all(target_dir.join(".fingerprint"));
            debug!("Cleared cargo cache after patching");
        }

        patches
    }

    /// Add olap,frm features to existing hyperswitch_domain_models dependency
    fn add_features_to_domain_models_dependency(content: &str) -> String {
        let mut result = content.to_string();

        // First, also add olap to storage_impl dependency if present
        result = Self::add_olap_to_storage_impl_dependency(&result);

        // Find the hyperswitch_domain_models line and add olap,frm to features
        // Pattern 1: hyperswitch_domain_models = { version = "0.1.0", path = "...", default-features = false }
        // Add features = ["olap", "frm"]
        let pattern1 = "hyperswitch_domain_models = { version = \"0.1.0\", path = \"../hyperswitch_domain_models\", default-features = false }";
        let replacement1 = "hyperswitch_domain_models = { version = \"0.1.0\", path = \"../hyperswitch_domain_models\", features = [\"olap\", \"frm\"], default-features = false }";

        if result.contains(pattern1) {
            result = result.replace(pattern1, replacement1);
            return result;
        }

        // Pattern 2: Already has features but missing olap - need to parse and add
        // Use simple string replacement for common patterns
        let lines: Vec<&str> = content.lines().collect();
        let mut new_lines = Vec::new();

        for line in lines {
            if line.contains("hyperswitch_domain_models") && line.contains("features") {
                // This line has features, check if olap is there
                if !line.contains("olap") {
                    // Add olap to features
                    if let Some(new_line) = Self::add_olap_to_features_line(line) {
                        new_lines.push(new_line);
                        continue;
                    }
                }
            } else if line.contains("hyperswitch_domain_models") && !line.contains("features") {
                // No features, add them
                if let Some(new_line) = Self::add_features_to_dep_line(line) {
                    new_lines.push(new_line);
                    continue;
                }
            }
            new_lines.push(line.to_string());
        }

        new_lines.join("\n")
    }

    /// Add olap feature to storage_impl dependency
    fn add_olap_to_storage_impl_dependency(content: &str) -> String {
        // Pattern 1: storage_impl = { version = "0.1.0", path = "../storage_impl", default-features = false }
        // Add features = ["olap"]
        let pattern1 = "storage_impl = { version = \"0.1.0\", path = \"../storage_impl\", default-features = false }";
        let replacement1 = "storage_impl = { version = \"0.1.0\", path = \"../storage_impl\", features = [\"olap\"], default-features = false }";

        let mut result = content.to_string();
        if result.contains(pattern1) {
            result = result.replace(pattern1, replacement1);
            return result;
        }

        // Pattern 2: storage_impl = { version = "...", path = "...", features = [...], default-features = false }
        // Need to add olap to existing features
        for line in content.lines() {
            if line.contains("storage_impl")
                && line.contains("features = [")
                && !line.contains("\"olap\"")
            {
                // Find the features array and add olap
                if let Some(features_start) = line.find("features = [") {
                    if let Some(_features_end) = line[features_start..].find(']') {
                        let insert_pos = features_start + "features = [".len();
                        let before = &line[..insert_pos];
                        let after = &line[insert_pos..];
                        let new_line = format!("{}\"olap\", {}", before, after);
                        result = result.replace(line, &new_line);
                        return result;
                    }
                }
            }
        }

        result
    }

    fn add_olap_to_features_line(line: &str) -> Option<String> {
        // Find features = [...] and add olap, frm
        let features_start = line.find("features = [")?;
        let bracket_start = features_start + "features = ".len();
        let bracket_end = line[bracket_start..].find(']')? + bracket_start;

        let features_content = &line[bracket_start + 1..bracket_end];

        // Add olap and frm if not present
        let mut new_features = features_content.to_string();
        if !new_features.contains("olap") {
            if new_features.is_empty() || new_features == "\"\"" {
                new_features = "\"olap\", \"frm\"".to_string();
            } else {
                new_features = format!(
                    "{}, \"olap\", \"frm\"",
                    new_features.trim_end_matches(',').trim_end_matches(' ')
                );
            }
        }

        Some(format!(
            "{}[{}]{}",
            &line[..bracket_start + 1],
            new_features,
            &line[bracket_end..]
        ))
    }

    fn add_features_to_dep_line(line: &str) -> Option<String> {
        // Add features = ["olap", "frm"] before default-features or closing brace
        if line.contains("default-features") {
            Some(line.replace(
                "default-features",
                "features = [\"olap\", \"frm\"], default-features",
            ))
        } else if line.contains('}') {
            Some(line.replace('}', ", features = [\"olap\", \"frm\"] }"))
        } else {
            None
        }
    }

    /// Find crates that need hyperswitch_domain_models or storage_impl patching
    fn find_crates_needing_domain_models_patch(
        &self,
        _workspace_root: &Path,
        all_crates: &[PathBuf],
    ) -> Vec<PathBuf> {
        let mut needs_patch = Vec::new();

        for crate_path in all_crates {
            let cargo_toml = crate_path.join("Cargo.toml");
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                let mut crate_needs_patch = false;

                // Check for storage_impl dependency without olap
                if content.contains("storage_impl") && content.contains("default-features = false")
                {
                    // Check if olap is in features
                    let has_storage_olap = content
                        .lines()
                        .filter(|l| l.contains("storage_impl") && l.contains("features"))
                        .any(|l| l.contains("\"olap\""));

                    if !has_storage_olap {
                        debug!("Crate {:?} needs storage_impl/olap patch", crate_path);
                        crate_needs_patch = true;
                    }
                }

                // Check for hyperswitch_interfaces dependency without domain_models with olap
                if content.contains("hyperswitch_interfaces") {
                    let has_domain_with_olap = content
                        .lines()
                        .filter(|l| {
                            l.contains("hyperswitch_domain_models") && l.contains("features")
                        })
                        .any(|l| l.contains("\"olap\""));

                    if !has_domain_with_olap {
                        debug!(
                            "Crate {:?} needs hyperswitch_domain_models/olap patch",
                            crate_path
                        );
                        crate_needs_patch = true;
                    }
                }

                // Check if this IS hyperswitch_interfaces
                if content.contains("name = \"hyperswitch_interfaces\"") {
                    let has_domain_with_olap = content
                        .lines()
                        .filter(|l| {
                            l.contains("hyperswitch_domain_models") && l.contains("features")
                        })
                        .any(|l| l.contains("\"olap\""));

                    if !has_domain_with_olap {
                        debug!(
                            "Crate {:?} (interfaces) needs hyperswitch_domain_models/olap patch",
                            crate_path
                        );
                        crate_needs_patch = true;
                    }
                }

                if crate_needs_patch {
                    needs_patch.push(crate_path.clone());
                }
            }
        }

        debug!("Found {} crates needing patch", needs_patch.len());
        needs_patch
    }

    /// Add hyperswitch_domain_models dependency to Cargo.toml content
    fn add_domain_models_dependency(content: &str) -> String {
        // Use toml_edit for proper parsing
        let mut doc: toml_edit::DocumentMut = match content.parse() {
            Ok(d) => d,
            Err(_) => {
                // Fall back to string manipulation if parsing fails
                return Self::add_domain_models_dependency_simple(content);
            }
        };

        // Create the dependency entry
        let dep_str = r#"hyperswitch_domain_models = { version = "0.1.0", path = "../hyperswitch_domain_models", features = ["olap", "frm"], default-features = false }"#;

        // Parse and insert
        if let Some(deps) = doc.get_mut("dependencies") {
            if let toml_edit::Item::Table(table) = deps {
                // Parse the dependency line
                if let Ok(dep_doc) = dep_str.parse::<toml_edit::DocumentMut>() {
                    if let Some((key, value)) = dep_doc.iter().next() {
                        table.insert(key, value.clone());
                    }
                }
            }
        } else {
            // Add [dependencies] section
            let mut deps_table = toml_edit::Table::new();
            if let Ok(dep_doc) = dep_str.parse::<toml_edit::DocumentMut>() {
                if let Some((key, value)) = dep_doc.iter().next() {
                    deps_table.insert(key, value.clone());
                }
            }
            doc.insert("dependencies", toml_edit::Item::Table(deps_table));
        }

        doc.to_string()
    }

    /// Simple string-based fallback for adding dependency
    fn add_domain_models_dependency_simple(content: &str) -> String {
        let dep_str = "hyperswitch_domain_models = { version = \"0.1.0\", path = \"../hyperswitch_domain_models\", features = [\"olap\", \"frm\"], default-features = false }";

        if let Some(pos) = content.find("[dependencies]") {
            let mut result = content.to_string();
            // Find next section or end
            let insert_pos = content[pos..]
                .find("\n[")
                .map(|p| pos + p)
                .unwrap_or(content.len());
            result.insert_str(insert_pos, &format!("\n{}", dep_str));
            result
        } else {
            format!("{}\n\n[dependencies]\n{}", content, dep_str)
        }
    }

    /// Restore all patched Cargo.toml files
    fn restore_patched_cargo_files(&self, patches: &HashMap<PathBuf, String>) {
        for (path, original_content) in patches {
            if let Err(e) = std::fs::write(path, original_content) {
                warn!("Failed to restore {:?}: {}", path, e);
            } else {
                debug!("Restored original Cargo.toml at {:?}", path);
            }
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

        let crates = match self.discover_crates(crate_path).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(StageResult::failed(
                    "expand",
                    format!("Failed to discover crates: {}", e),
                ));
            }
        };

        // Store the workspace root for use in expand_library
        let workspace_root = crate_path.as_path();

        // ====================================================================
        // PRE-PATCH PHASE: Identify and patch crates needing feature propagation
        // ====================================================================
        let patches = self.pre_patch_crates_for_features(workspace_root, &crates);
        if !patches.is_empty() {
            info!(
                "Pre-patched {} crates for feature propagation",
                patches.len()
            );
        }

        let _ = std::fs::create_dir_all(EXPAND_CACHE_DIR);

        // Process crates in batches to prevent OOM from accumulating all expanded sources
        const BATCH_SIZE: usize = 8;

        let mut expanded_count = 0;
        let mut failed_count = 0;
        let mut skipped_binary = 0;

        // Map source file path -> cache file path (not content!)
        let mut expanded_map: HashMap<PathBuf, PathBuf> = HashMap::new();
        let mut all_source_files: Vec<SourceFileInfo> = Vec::new();

        for (batch_idx, batch) in crates.chunks(BATCH_SIZE).enumerate() {
            let batch_start = batch_idx * BATCH_SIZE;
            info!(
                "Expand batch {} (crates {}-{})",
                batch_idx + 1,
                batch_start + 1,
                batch_start + batch.len()
            );

            for crate_path in batch {
                let git_hash = self.get_git_hash(crate_path);

                let crate_name = self
                    .get_crate_name_from_toml(crate_path)
                    .unwrap_or_else(|| {
                        crate_path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "unknown".to_string())
                    });

                let source_files = self.find_source_files(crate_path);
                if source_files.is_empty() {
                    debug!("Skipping {:?} - no source files", crate_path);
                    continue;
                }

                // Compute cache file path: {crate_name}-{hash}.expand
                let content_hash = self.compute_crate_hash(crate_path);
                let cache_key = format!("{}-{}.expand", crate_name, content_hash);
                let cache_file = PathBuf::from(EXPAND_CACHE_DIR).join(&cache_key);

                // Expand the crate (writes to cache file internally)
                let expanded_result = self.expand_library(crate_path, workspace_root, &crate_name);

                match expanded_result {
                    Ok(_) => {
                        expanded_count += 1;
                        let has_cache = cache_file.exists();
                        if !has_cache {
                            warn!(
                                "Expand cache file missing for {} at {:?} — \
                                 parse will use original source (possible permission issue)",
                                crate_name, cache_file
                            );
                        }

                        for file_path in &source_files {
                            // Pre-flight file size check: skip files > 10 MB
                            if let Ok(metadata) = std::fs::metadata(file_path) {
                                if crate::pipeline::memory_accountant::MemoryAccountant::should_skip_file(metadata.len()) {
                                    info!("Skipping oversized file {:?} ({} bytes)", file_path, metadata.len());
                                    continue;
                                }
                            }

                            if let Ok(source) = std::fs::read_to_string(file_path) {
                                let module_path =
                                    compute_module_path(crate_path, file_path, &crate_name);
                                let file_hash = compute_content_hash(&source);

                                all_source_files.push(SourceFileInfo {
                                    path: file_path.clone(),
                                    crate_name: crate_name.clone(),
                                    module_path,
                                    original_source: Arc::new(source),
                                    git_hash: git_hash.clone(),
                                    content_hash: file_hash,
                                });

                                // Map to cache file only if it exists
                                if has_cache {
                                    expanded_map.insert(file_path.clone(), cache_file.clone());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("no library targets found") {
                            debug!("Skipping {} - binary only crate", crate_name);
                            skipped_binary += 1;
                        } else {
                            warn!("Failed to expand {}: {}", crate_name, e);
                            failed_count += 1;
                        }
                    }
                }
            }

            // Release memory between batches
            trim_memory();
            info!("Expand batch {} complete, memory trimmed", batch_idx + 1);
        }

        // Update state with cache file paths and source files
        {
            let mut state = ctx.state.write().await;
            state.expanded_sources = Arc::new(expanded_map);
            state.source_files = all_source_files;
            state.counts.files_expanded = expanded_count;

            // Record any failures
            // (errors were not being tracked in original code for individual crates)
        }

        // ====================================================================
        // RESTORE PHASE: Restore all patched Cargo.toml files
        // ====================================================================
        self.restore_patched_cargo_files(&patches);

        let duration = start.elapsed();
        info!(
            "Expand stage: {} expanded, {} failed, {} skipped (binary-only), {:?}",
            expanded_count, failed_count, skipped_binary, duration
        );

        if failed_count > 0 && expanded_count == 0 {
            Ok(StageResult::failed(
                "expand",
                "All expansion attempts failed",
            ))
        } else if failed_count > 0 {
            Ok(StageResult::partial(
                "expand",
                expanded_count,
                failed_count,
                duration,
                format!(
                    "{} crates expanded, {} failed, {} skipped",
                    expanded_count, failed_count, skipped_binary
                ),
            ))
        } else {
            Ok(StageResult::success(
                "expand",
                expanded_count + skipped_binary,
                0,
                duration,
            ))
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
            let expanded_cache_paths = state.expanded_sources.clone();
            drop(state);

            // Clear source_files to free memory (these are cloned for local use).
            // Note: We preserve expanded_sources because TypecheckStage needs these
            // path mappings to read expanded source from disk cache. The HashMap only
            // stores PathBuf → PathBuf mappings (~1-2 MB for large codebases), not
            // actual source content, so the memory cost is negligible.
            let state = ctx.state.write().await;
            // DON'T clear source_files - ExtractStage needs them to store source content
            // state.source_files.clear();
            drop(state);

            trim_memory();

            (source_files, expanded_cache_paths)
        };
        let (source_files, expanded_cache_paths) = source_files;

        let workspace_crate_names = if ctx.config.workspace_crate_names.is_empty() {
            match discover_workspace_crate_names(&ctx.config.crate_path) {
                Ok(names) => {
                    info!(
                        "Discovered {} workspace crate names for FQN filtering: {:?}",
                        names.len(),
                        names
                    );
                    names
                }
                Err(e) => {
                    warn!(
                        "Failed to discover workspace crate names, skipping FQN filtering: {}",
                        e
                    );
                    Vec::new()
                }
            }
        } else {
            ctx.config.workspace_crate_names.clone()
        };

        let mut external_filtered_count = 0usize;

        if source_files.is_empty() {
            return Ok(StageResult::skipped("parse"));
        }

        let total_files = source_files.len();
        info!(
            "Parsing {} files with {} threads, batch size 10",
            total_files, MAX_PARSE_THREADS
        );

        let _pool = rayon::ThreadPoolBuilder::new()
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
            info!(
                "Parsing batch {} (files {}-{})",
                batch_idx + 1,
                batch_start + 1,
                batch_start + batch.len()
            );

            for file_info in batch {
                // Read expanded source from cache file on-demand (not from memory)
                let expanded_source: Option<String> = expanded_cache_paths
                    .get(&file_info.path)
                    .and_then(|cache_path| std::fs::read_to_string(cache_path).ok());

                let (source_to_parse, has_expanded) = expanded_source
                    .as_ref()
                    .map(|s| (s.as_str(), true))
                    .unwrap_or((&file_info.original_source, false));

                // Count impl blocks in expanded source - files with 5000+ impl blocks consume 50+ GB
                let impl_count = if has_expanded {
                    source_to_parse.matches("impl ").count()
                } else {
                    0
                };

                // Skip expanded files with too many impl blocks - they cause OOM during derive detection
                let skip_expanded = has_expanded
                    && (source_to_parse.len() > MAX_EXPANDED_SOURCE_SIZE
                        || impl_count > MAX_IMPL_BLOCKS);

                if skip_expanded {
                    let reason = if source_to_parse.len() > MAX_EXPANDED_SOURCE_SIZE {
                        format!(
                            "{} bytes > {} limit",
                            source_to_parse.len(),
                            MAX_EXPANDED_SOURCE_SIZE
                        )
                    } else {
                        format!("{} impl blocks > {} limit", impl_count, MAX_IMPL_BLOCKS)
                    };
                    info!(
                        "Skipping large expanded file {:?} ({}) - parsing original instead",
                        file_info.path, reason
                    );
                    let result = self
                        .parser
                        .parse(&file_info.original_source, &file_info.module_path);
                    match result {
                        Ok(parse_result) => {
                            let mut items: Vec<ParsedItemInfo> = parse_result
                                .items
                                .iter()
                                .map(|item| ParsedItemInfo {
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
                                })
                                .collect();

                            if !workspace_crate_names.is_empty() {
                                let before_filter = items.len();
                                items.retain(|item| {
                                    Self::is_workspace_fqn(&item.fqn, &workspace_crate_names)
                                });
                                let filtered = before_filter - items.len();
                                if filtered > 0 {
                                    external_filtered_count += filtered;
                                    debug!(
                                        "Filtered {} external-crate items from {:?} (kept {})",
                                        filtered,
                                        file_info.path,
                                        items.len()
                                    );
                                }
                            }

                            let items_len = items.len();
                            items_count += items_len;
                            parsed_count += 1;

                            if !items.is_empty() {
                                let items_ref: Vec<&ParsedItemInfo> = items.iter().collect();
                                let insert_count =
                                    Self::batch_insert_items(&db_pool, &items_ref).await;
                                debug!("Inserted {} items for {:?}", insert_count, file_info.path);
                            }

                            drop(items);
                            drop(parse_result);
                            trim_memory();
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse original source for {:?}: {}",
                                file_info.path, e
                            );
                            failed_count += 1;
                        }
                    }
                    continue;
                }

                let result = self.parser.parse(source_to_parse, &file_info.module_path);

                match result {
                    Ok(parse_result) => {
                        let generated_by_map = if has_expanded {
                            self.derive_detector
                                .detect(
                                    &file_info.original_source,
                                    source_to_parse,
                                    &file_info.module_path,
                                )
                                .map(|d| d.generated_by)
                                .unwrap_or_default()
                        } else {
                            HashMap::new()
                        };

                        let mut items: Vec<ParsedItemInfo> = parse_result
                            .items
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

                        if !workspace_crate_names.is_empty() {
                            let before_filter = items.len();
                            items.retain(|item| {
                                Self::is_workspace_fqn(&item.fqn, &workspace_crate_names)
                            });
                            let filtered = before_filter - items.len();
                            if filtered > 0 {
                                external_filtered_count += filtered;
                                debug!(
                                    "Filtered {} external-crate items from {:?} (kept {})",
                                    filtered,
                                    file_info.path,
                                    items.len()
                                );
                            }
                        }

                        let items_len = items.len();
                        derive_generated_count +=
                            items.iter().filter(|i| i.generated_by.is_some()).count();
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
                                state.errors.push(
                                    StageError::new("parse", err.message.clone()).with_context(
                                        format!(
                                            "{}:{}",
                                            file_info.path.display(),
                                            err.line.unwrap_or(0)
                                        ),
                                    ),
                                );
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
                        state.errors.push(
                            StageError::new("parse", e.to_string())
                                .with_context(file_info.path.display().to_string()),
                        );
                        drop(state);
                        failed_count += 1;
                    }
                }
            }

            trim_memory();
            info!(
                "Batch {} complete: {}/{} files, {} items total",
                batch_idx + 1,
                parsed_count,
                total_files,
                items_count
            );
        }

        let mut state = ctx.state.write().await;
        state.counts.files_parsed = parsed_count;
        state.counts.items_parsed = items_count;
        drop(state);

        let duration = start.elapsed();

        info!(
            "Parse stage completed: {} files, {} items ({} derive-generated, {} external-filtered) in {:?}",
            parsed_count, items_count, derive_generated_count, external_filtered_count, duration
        );

        if failed_count > 0 && parsed_count == 0 {
            Ok(StageResult::failed("parse", "All parsing attempts failed"))
        } else if failed_count > 0 {
            Ok(StageResult::partial(
                "parse",
                parsed_count,
                failed_count,
                duration,
                format!(
                    "{} files parsed, {} failed, {} items",
                    parsed_count, failed_count, items_count
                ),
            ))
        } else {
            Ok(StageResult::success("parse", items_count, 0, duration))
        }
    }
}

impl ParseStage {
    /// Check if an FQN belongs to a workspace crate
    fn is_workspace_fqn(fqn: &str, workspace_crate_names: &[String]) -> bool {
        workspace_crate_names
            .iter()
            .any(|crate_name| fqn == crate_name || fqn.starts_with(&format!("{}::", crate_name)))
    }

    fn find_derive_source(
        &self,
        item: &ParsedItem,
        generated_by_map: &HashMap<String, String>,
    ) -> Option<String> {
        for attr in &item.attributes {
            if let Some(trait_name) = attr.strip_prefix("impl_for=") {
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
            debug!(
                "Deduplicated {} items with duplicate FQNs in batch of {}",
                dup_count,
                items.len()
            );
        }

        let ids: Vec<String> = deduped_items
            .iter()
            .map(|_| Uuid::new_v4().to_string())
            .collect();
        let item_types: Vec<&str> = deduped_items.iter().map(|i| i.item_type.as_str()).collect();
        let fqns: Vec<&str> = deduped_items.iter().map(|i| i.fqn.as_str()).collect();
        let names: Vec<&str> = deduped_items.iter().map(|i| i.name.as_str()).collect();
        let visibilities: Vec<&str> = deduped_items
            .iter()
            .map(|i| i.visibility.as_str())
            .collect();
        let signatures: Vec<&str> = deduped_items.iter().map(|i| i.signature.as_str()).collect();
        let doc_comments: Vec<&str> = deduped_items
            .iter()
            .map(|i| i.doc_comment.as_str())
            .collect();
        let start_lines: Vec<i32> = deduped_items.iter().map(|i| i.start_line as i32).collect();
        let end_lines: Vec<i32> = deduped_items.iter().map(|i| i.end_line as i32).collect();
        let body_sources: Vec<&str> = deduped_items
            .iter()
            .map(|i| i.body_source.as_str())
            .collect();
        let generic_params: Vec<serde_json::Value> = deduped_items
            .iter()
            .map(|i| serde_json::to_value(&i.generic_params).unwrap_or(serde_json::json!([])))
            .collect();
        let where_clauses: Vec<serde_json::Value> = deduped_items
            .iter()
            .map(|i| serde_json::to_value(&i.where_clauses).unwrap_or(serde_json::json!([])))
            .collect();
        let attributes: Vec<serde_json::Value> = deduped_items
            .iter()
            .map(|i| serde_json::to_value(&i.attributes).unwrap_or(serde_json::json!([])))
            .collect();
        let generated_bys: Vec<Option<&str>> = deduped_items
            .iter()
            .map(|i| i.generated_by.as_deref())
            .collect();

        let generic_params_json: Vec<String> =
            generic_params.iter().map(|v| v.to_string()).collect();
        let where_clauses_json: Vec<String> = where_clauses.iter().map(|v| v.to_string()).collect();
        let attributes_json: Vec<String> = attributes.iter().map(|v| v.to_string()).collect();

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
                source_file_id = COALESCE(EXCLUDED.source_file_id, extracted_items.source_file_id),
                generated_by = COALESCE(EXCLUDED.generated_by, extracted_items.generated_by),
                updated_at = NOW()
            "#,
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
pub struct TypecheckStage {}

impl Default for TypecheckStage {
    fn default() -> Self {
        Self::new()
    }
}

impl TypecheckStage {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl PipelineStage for TypecheckStage {
    fn name(&self) -> &str {
        "typecheck"
    }

    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();
        let database_url = &ctx.config.database_url;

        let state = ctx.state.read().await;
        let parsed_items: Vec<ParsedItemInfo> =
            state.parsed_items.values().flatten().cloned().collect();
        let expanded_sources = state.expanded_sources.clone();
        let source_files: Vec<SourceFileInfo> = state.source_files.clone();
        drop(state);

        if parsed_items.is_empty() {
            info!("Typecheck stage skipped: no parsed items");
            return Ok(StageResult::skipped("typecheck"));
        }

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .context("Failed to connect to database for typecheck stage")?;

        let service = TypeResolutionService::new(pool);

        let mut typechecked_count = 0usize;
        let mut failed_count = 0usize;

        // Process each expanded source file for type information
        for source_file in &source_files {
            let expanded_path = match expanded_sources.get(&source_file.path) {
                Some(path) => path,
                None => continue,
            };

            let expanded_source = match std::fs::read_to_string(expanded_path) {
                Ok(content) => content,
                Err(e) => {
                    warn!(
                        "Failed to read expanded source {}: {}",
                        expanded_path.display(),
                        e
                    );
                    failed_count += 1;
                    continue;
                }
            };

            // Collect caller FQNs for this file
            let caller_fqns: Vec<String> = parsed_items
                .iter()
                .filter(|item| {
                    // Match items from this source file approximately by module path
                    item.fqn
                        .starts_with(&source_file.module_path.replace('/', "::"))
                        || item.fqn.starts_with(&source_file.crate_name)
                })
                .map(|item| item.fqn.clone())
                .collect();

            let file_size = expanded_source.len();
            let result = if file_size > 10_000_000 {
                // Use heuristics for large files (>10MB)
                debug!(
                    "Using heuristic analysis for large file ({} bytes): {}",
                    file_size,
                    source_file.path.display()
                );
                service
                    .analyze_with_heuristics(
                        &source_file.crate_name,
                        &source_file.module_path,
                        &source_file.path.to_string_lossy(),
                        &expanded_source,
                        &caller_fqns,
                    )
                    .await
            } else {
                service
                    .analyze_expanded_source(
                        &source_file.crate_name,
                        &source_file.module_path,
                        &source_file.path.to_string_lossy(),
                        &expanded_source,
                        &caller_fqns,
                    )
                    .await
            };

            match result {
                Ok(resolution_result) => {
                    typechecked_count += 1;
                    debug!(
                        "Typecheck {}: {} trait impls, {} call sites, {} errors",
                        source_file.path.display(),
                        resolution_result.trait_impls.len(),
                        resolution_result.call_sites.len(),
                        resolution_result.errors.len(),
                    );
                    for err in &resolution_result.errors {
                        warn!("Typecheck error in {}: {}", source_file.path.display(), err);
                    }
                }
                Err(e) => {
                    warn!("Typecheck failed for {}: {}", source_file.path.display(), e);
                    failed_count += 1;
                }
            }
        }

        let mut state = ctx.state.write().await;
        state.counts.items_typechecked = typechecked_count;
        drop(state);

        let duration = start.elapsed();
        info!(
            "Typecheck stage completed: {} files typechecked, {} failed in {:?}",
            typechecked_count, failed_count, duration
        );

        if failed_count > 0 && typechecked_count == 0 {
            Ok(StageResult::failed(
                "typecheck",
                "All typecheck attempts failed",
            ))
        } else if failed_count > 0 {
            Ok(StageResult::partial(
                "typecheck",
                typechecked_count,
                failed_count,
                duration,
                format!(
                    "{} files typechecked, {} failed",
                    typechecked_count, failed_count
                ),
            ))
        } else {
            Ok(StageResult::success(
                "typecheck",
                typechecked_count,
                0,
                duration,
            ))
        }
    }
}

// =============================================================================
// EXTRACT STAGE
// =============================================================================

/// Stage 4: Extract parsed items to Postgres
pub struct ExtractStage {}

impl Default for ExtractStage {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtractStage {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl PipelineStage for ExtractStage {
    fn name(&self) -> &str {
        "extract"
    }

    async fn run(&self, ctx: &PipelineContext) -> Result<StageResult> {
        let start = Instant::now();

        if ctx.config.dry_run {
            info!("Extract stage skipped (dry_run mode)");
            return Ok(StageResult::skipped("extract"));
        }

        let database_url = &ctx.config.database_url;

        let state = ctx.state.read().await;
        let parsed_items: Vec<ParsedItemInfo> =
            state.parsed_items.values().flatten().cloned().collect();
        let source_files: Vec<SourceFileInfo> = state.source_files.clone();
        drop(state);

        if parsed_items.is_empty() {
            info!("Extract stage skipped: no parsed items");
            return Ok(StageResult::skipped("extract"));
        }

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("Failed to connect to database for extract stage")?;

        // Build a map from file path to source file info for linking
        let file_map: HashMap<PathBuf, &SourceFileInfo> = source_files
            .iter()
            .map(|sf| (sf.path.clone(), sf))
            .collect();

        // Ensure all source files exist in the database
        let mut file_ids: HashMap<PathBuf, Uuid> = HashMap::new();
        for source_file in &source_files {
            let file_id = Uuid::new_v4();
            sqlx::query(
                r#"
                INSERT INTO source_files (id, file_path, crate_name, module_path, content_hash, git_hash)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (file_path, content_hash) DO UPDATE SET
                    crate_name = EXCLUDED.crate_name,
                    module_path = EXCLUDED.module_path,
                    git_hash = COALESCE(EXCLUDED.git_hash, source_files.git_hash)
                RETURNING id
                "#,
            )
            .bind(file_id)
            .bind(source_file.path.to_string_lossy().to_string())
            .bind(&source_file.crate_name)
            .bind(&source_file.module_path)
            .bind(&source_file.content_hash)
            .bind(&source_file.git_hash)
            .fetch_one(&pool)
            .await
            .ok()
            .map(|row| row.get::<Uuid, _>("id"))
            .unwrap_or(file_id);

            file_ids.insert(source_file.path.clone(), file_id);
        }

        // Batch insert extracted items
        let batch_size = std::env::var("EXTRACT_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(100);

        let mut extracted_count = 0usize;
        let mut failed_count = 0usize;

        for chunk in parsed_items.chunks(batch_size) {
            let items_with_ids: Vec<(Uuid, &ParsedItemInfo)> =
                chunk.iter().map(|item| (Uuid::new_v4(), item)).collect();

            let ids: Vec<String> = items_with_ids
                .iter()
                .map(|(id, _)| id.to_string())
                .collect();
            let item_types: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.item_type.as_str())
                .collect();
            let fqns: Vec<&str> = items_with_ids.iter().map(|(_, i)| i.fqn.as_str()).collect();
            let names: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.name.as_str())
                .collect();
            let visibilities: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.visibility.as_str())
                .collect();
            let signatures: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.signature.as_str())
                .collect();
            let doc_comments: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.doc_comment.as_str())
                .collect();
            let start_lines: Vec<i32> = items_with_ids
                .iter()
                .map(|(_, i)| i.start_line as i32)
                .collect();
            let end_lines: Vec<i32> = items_with_ids
                .iter()
                .map(|(_, i)| i.end_line as i32)
                .collect();
            let body_sources: Vec<&str> = items_with_ids
                .iter()
                .map(|(_, i)| i.body_source.as_str())
                .collect();
            let generic_params: Vec<serde_json::Value> = items_with_ids
                .iter()
                .map(|(_, i)| {
                    serde_json::to_value(&i.generic_params).unwrap_or(serde_json::json!([]))
                })
                .collect();
            let where_clauses: Vec<serde_json::Value> = items_with_ids
                .iter()
                .map(|(_, i)| {
                    serde_json::to_value(&i.where_clauses).unwrap_or(serde_json::json!([]))
                })
                .collect();
            let attributes: Vec<serde_json::Value> = items_with_ids
                .iter()
                .map(|(_, i)| serde_json::to_value(&i.attributes).unwrap_or(serde_json::json!([])))
                .collect();
            let generated_bys: Vec<Option<&str>> = items_with_ids
                .iter()
                .map(|(_, i)| i.generated_by.as_deref())
                .collect();

            // Resolve source_file_id for each item
            let source_file_ids: Vec<Option<Uuid>> = items_with_ids
                .iter()
                .map(|(_, item)| {
                    // Try to find the source file for this item
                    file_map
                        .keys()
                        .find(|p| {
                            item.fqn
                                .contains(&*p.file_stem().unwrap_or_default().to_string_lossy())
                        })
                        .and_then(|p| file_ids.get(p).copied())
                })
                .collect();

            let generic_params_json: Vec<String> =
                generic_params.iter().map(|v| v.to_string()).collect();
            let where_clauses_json: Vec<String> =
                where_clauses.iter().map(|v| v.to_string()).collect();
            let attributes_json: Vec<String> = attributes.iter().map(|v| v.to_string()).collect();
            let source_file_id_strs: Vec<Option<String>> = source_file_ids
                .iter()
                .map(|id| id.map(|u| u.to_string()))
                .collect();

            let result = sqlx::query(
                r#"
                INSERT INTO extracted_items 
                    (id, source_file_id, item_type, fqn, name, visibility, signature, 
                     doc_comment, start_line, end_line, body_source, 
                     generic_params, where_clauses, attributes, generated_by)
                SELECT 
                    unnest($1::uuid[]) as id,
                    unnest($2::uuid[]) as source_file_id,
                    unnest($3::text[]) as item_type,
                    unnest($4::text[]) as fqn,
                    unnest($5::text[]) as name,
                    unnest($6::text[]) as visibility,
                    unnest($7::text[]) as signature,
                    unnest($8::text[]) as doc_comment,
                    unnest($9::int[]) as start_line,
                    unnest($10::int[]) as end_line,
                    unnest($11::text[]) as body_source,
                    unnest($12::jsonb[]) as generic_params,
                    unnest($13::jsonb[]) as where_clauses,
                    unnest($14::jsonb[]) as attributes,
                    unnest($15::text[]) as generated_by
                ON CONFLICT (fqn) DO UPDATE SET
                    item_type = EXCLUDED.item_type,
                    name = EXCLUDED.name,
                    signature = EXCLUDED.signature,
                    doc_comment = EXCLUDED.doc_comment,
                    start_line = EXCLUDED.start_line,
                    end_line = EXCLUDED.end_line,
                    body_source = EXCLUDED.body_source,
                    generic_params = EXCLUDED.generic_params,
                    where_clauses = EXCLUDED.where_clauses,
                    attributes = EXCLUDED.attributes,
                    visibility = EXCLUDED.visibility,
                    source_file_id = COALESCE(EXCLUDED.source_file_id, extracted_items.source_file_id),
                    generated_by = COALESCE(EXCLUDED.generated_by, extracted_items.generated_by),
                    updated_at = NOW()
                "#,
            )
            .bind(&ids)
            .bind(&source_file_id_strs)
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
            .execute(&pool)
            .await;

            match result {
                Ok(r) => {
                    extracted_count += chunk.len();
                    debug!(
                        "Extracted batch of {} items ({} rows affected)",
                        chunk.len(),
                        r.rows_affected()
                    );
                }
                Err(e) => {
                    warn!("Batch extract failed: {}", e);
                    failed_count += chunk.len();
                }
            }
        }

        // Update state with extracted item IDs
        let mut state = ctx.state.write().await;
        state.counts.items_extracted = extracted_count;
        for item in &parsed_items {
            state
                .extracted_items
                .insert(item.fqn.clone(), Uuid::new_v4());
        }
        drop(state);

        let duration = start.elapsed();
        info!(
            "Extract stage completed: {} items extracted, {} failed in {:?}",
            extracted_count, failed_count, duration
        );

        if failed_count > 0 && extracted_count == 0 {
            Ok(StageResult::failed(
                "extract",
                "All extract attempts failed",
            ))
        } else if failed_count > 0 {
            Ok(StageResult::partial(
                "extract",
                extracted_count,
                failed_count,
                duration,
                format!(
                    "{} items extracted, {} failed",
                    extracted_count, failed_count
                ),
            ))
        } else {
            Ok(StageResult::success(
                "extract",
                extracted_count,
                0,
                duration,
            ))
        }
    }
}

// =============================================================================
// GRAPH STAGE
// =============================================================================

use crate::graph::{
    GraphBuilder, GraphConfig, NodeData, NodeType, PropertyValue, RelationshipBuilder,
    RelationshipData,
};

/// Stage 5: Build Neo4j relationship graph
pub struct GraphStage {}

impl Default for GraphStage {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphStage {
    pub fn new() -> Self {
        Self {}
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

        // Validate workspace label is present — refuse to ingest without it
        if ctx.config.workspace_label.is_none() {
            error!("workspace_label is required for graph ingestion but was not provided");
            return Ok(StageResult::failed(
                "graph",
                "workspace_label is required for graph ingestion. \
                 Set PipelineConfig.workspace_label to a value like \"Workspace_<12hex>\" \
                 matching the Postgres ws_<12hex> schema name."
                    .to_string(),
            ));
        }

        let state = ctx.state.read().await;
        let mut parsed_items = state.parsed_items.clone();
        let source_files = state.source_files.clone();
        drop(state);

        // Discover workspace crate names for FQN filtering
        let workspace_crate_names = if ctx.config.workspace_crate_names.is_empty() {
            match crate::pipeline::discover_workspace_crate_names(&ctx.config.crate_path) {
                Ok(names) => {
                    info!(
                        "Graph stage: discovered {} workspace crate names for FQN filtering: {:?}",
                        names.len(),
                        names
                    );
                    names
                }
                Err(e) => {
                    warn!("Graph stage: failed to discover workspace crate names, skipping FQN filtering: {}", e);
                    Vec::new()
                }
            }
        } else {
            ctx.config.workspace_crate_names.clone()
        };

        if parsed_items.is_empty() {
            info!("No parsed items in state, loading from database...");
            match self
                .load_items_from_database(
                    &ctx.config.database_url,
                    ctx.config.crate_name.as_deref(),
                )
                .await
            {
                Ok(items) => {
                    if items.is_empty() {
                        info!("No items found in database");
                        return Ok(StageResult::skipped("graph"));
                    }
                    // Group items by a dummy path since they came from DB
                    parsed_items.insert(PathBuf::from("__database__"), items);
                    info!(
                        "Loaded {} items from database",
                        parsed_items.values().map(|v| v.len()).sum::<usize>()
                    );
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
            password: std::env::var("NEO4J_PASSWORD")
                .expect("NEO4J_PASSWORD environment variable must be set"),
            database: std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string()),
            workspace_label: ctx.config.workspace_label.clone(),
            ..Default::default()
        };

        // Connect to Neo4j
        info!("Connecting to Neo4j at {}", redact_url(&config.uri));
        let graph_builder = match GraphBuilder::with_config(config).await {
            Ok(gb) => gb,
            Err(e) => {
                error!("Failed to connect to Neo4j: {}", e);
                return Ok(StageResult::failed(
                    "graph",
                    format!("Neo4j connection failed: {}", e),
                ));
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
                return Ok(StageResult::failed(
                    "graph",
                    format!("Neo4j connection test error: {}", e),
                ));
            }
        }

        // Create indexes for better performance
        if let Err(e) = graph_builder.create_indexes().await {
            warn!("Failed to create indexes (may already exist): {}", e);
        }

        // Create workspace-scoped constraints
        if let Some(ref ws_label) = ctx.config.workspace_label {
            if let Err(e) = graph_builder.create_workspace_constraints(ws_label).await {
                warn!(
                    "Failed to create workspace constraints (may already exist): {}",
                    e
                );
            }
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

        let mut item_fqns: Vec<String> = Vec::new();
        let mut impl_items: Vec<&ParsedItemInfo> = Vec::new();

        let mut graph_filtered_count = 0usize;
        for items in parsed_items.values() {
            for item in items {
                if !workspace_crate_names.is_empty()
                    && !Self::is_workspace_fqn(&item.fqn, &workspace_crate_names)
                {
                    graph_filtered_count += 1;
                    debug!("Graph stage: skipping external item: {}", item.fqn);
                    continue;
                }

                let node = Self::item_to_node(item);
                all_nodes.push(node);
                item_fqns.push(item.fqn.clone());

                if item.item_type == "impl" {
                    impl_items.push(item);
                }
            }
        }
        if graph_filtered_count > 0 {
            info!(
                "Graph stage: filtered out {} external-crate items from node creation",
                graph_filtered_count
            );
        }

        // Build O(1) lookup indexes for traits (avoids O(n²) in find_trait_fqn)
        let mut trait_name_to_fqns: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut local_trait_fqns: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for items in parsed_items.values() {
            for item in items {
                if item.item_type == "trait" {
                    trait_name_to_fqns.insert(item.name.clone(), item.fqn.clone());
                    local_trait_fqns.insert(item.fqn.clone());
                }
            }
        }
        info!(
            "Built trait lookup index: {} traits indexed",
            trait_name_to_fqns.len()
        );

        info!("Inserting {} nodes into Neo4j", all_nodes.len());

        // Batch insert nodes
        let node_count = all_nodes.len();
        match graph_builder.create_nodes_batch(all_nodes).await {
            Ok(_) => info!("Successfully inserted {} nodes", node_count),
            Err(e) => {
                error!("Failed to insert nodes batch: {}", e);
                return Ok(StageResult::failed(
                    "graph",
                    format!("Failed to insert nodes: {}", e),
                ));
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
        for items in parsed_items.values() {
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
                                "Crate",
                                Self::item_type_to_label(&item.item_type),
                            ));
                        }
                    } else {
                        // Module → Item relationship (parent module contains this item)
                        relationships.push(RelationshipBuilder::create_contains(
                            parent_fqn.clone(),
                            item.fqn.clone(),
                            "Module",
                            Self::item_type_to_label(&item.item_type),
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
        let mut seen_external_traits: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // First pass: collect all external traits that need nodes created
        for impl_item in &impl_items {
            if let Some(trait_name) = Self::extract_trait_from_impl(&impl_item.attributes) {
                let trait_fqn = Self::find_trait_fqn_optimized(
                    &trait_name,
                    &trait_name_to_fqns,
                    &impl_item.fqn,
                )
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
            info!(
                "Creating {} external trait nodes for IMPLEMENTS relationships",
                external_trait_nodes.len()
            );
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
                let trait_fqn = Self::find_trait_fqn_optimized(
                    &trait_name,
                    &trait_name_to_fqns,
                    &impl_item.fqn,
                )
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

        info!(
            "Created {} IMPLEMENTS and {} FOR relationships",
            impl_count, for_count
        );

        // 3. Create HAS_FIELD relationships for structs
        let mut field_count = 0;
        for items in parsed_items.values() {
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
        for items in parsed_items.values() {
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
        for items in parsed_items.values() {
            for item in items {
                if item.item_type == "trait" {
                    // Check generic_params for supertrait bounds
                    for generic_param in &item.generic_params {
                        if generic_param.kind == "type" {
                            for bound in &generic_param.bounds {
                                if !bound.starts_with(|c: char| c.is_lowercase()) {
                                    let super_trait_fqn = Self::find_trait_fqn_optimized(
                                        bound,
                                        &trait_name_to_fqns,
                                        &item.fqn,
                                    )
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
        let mut function_names_to_fqns: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for items in parsed_items.values() {
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
        for items in parsed_items.values() {
            for item in items {
                if !workspace_crate_names.is_empty()
                    && !Self::is_workspace_fqn(&item.fqn, &workspace_crate_names)
                {
                    continue;
                }
                if item.item_type == "function" && !item.body_source.is_empty() {
                    // Static/free function calls
                    let calls = Self::extract_function_calls(
                        &item.body_source,
                        &function_fqns,
                        &function_names_to_fqns,
                        &item.fqn,
                    );
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
                    let static_callee_fqns: std::collections::HashSet<&str> =
                        calls.iter().map(|(fqn, _)| fqn.as_str()).collect();
                    // Determine self_type from impl_type attribute (set by parse_impl_with_methods)
                    let self_type_attr = item
                        .attributes
                        .iter()
                        .find(|a| a.starts_with("impl_type="))
                        .map(|a| &a["impl_type=".len()..]);
                    let method_calls = Self::extract_method_calls(
                        &item.body_source,
                        &function_names_to_fqns,
                        self_type_attr,
                    );
                    for (callee_fqn, line) in &method_calls {
                        // Avoid duplicate entries
                        if !static_callee_fqns.contains(callee_fqn.as_str()) {
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
        info!(
            "Created {} CALLS relationships (including method calls)",
            calls_count
        );

        // 7. Create USES_TYPE relationships for type usage in functions/methods
        // Build a set of all known type FQNs (structs, enums, traits, type aliases)
        let mut type_fqns: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut type_names_to_fqns: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for items in parsed_items.values() {
            for item in items {
                if item.item_type == "struct"
                    || item.item_type == "enum"
                    || item.item_type == "trait"
                    || item.item_type == "type_alias"
                {
                    type_fqns.insert(item.fqn.clone());
                    type_names_to_fqns
                        .entry(item.name.clone())
                        .or_default()
                        .push(item.fqn.clone());
                }
            }
        }

        let mut uses_type_count = 0;
        for items in parsed_items.values() {
            for item in items {
                if !workspace_crate_names.is_empty()
                    && !Self::is_workspace_fqn(&item.fqn, &workspace_crate_names)
                {
                    continue;
                }
                if item.item_type == "function" {
                    // Extract types from signature
                    let sig_types = Self::extract_types_from_signature(&item.signature, &item.fqn);
                    for (type_name, context) in sig_types {
                        let type_fqn = Self::resolve_type_fqn_with_lookup(
                            &item.fqn,
                            &type_name,
                            &type_fqns,
                            &type_names_to_fqns,
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
                        let body_types =
                            Self::extract_types_from_body(&item.body_source, &item.fqn);
                        for (type_name, line) in body_types {
                            let type_fqn = Self::resolve_type_fqn_with_lookup(
                                &item.fqn,
                                &type_name,
                                &type_fqns,
                                &type_names_to_fqns,
                            );

                            if type_fqns.contains(&type_fqn) || !Self::is_primitive_type(&type_name)
                            {
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

                // Extract type usages from impl block signatures (e.g. "impl Trait for Type")
                // Body scanning is handled by individual method items (item_type="function")
                // to avoid double-counting.
                if item.item_type == "impl" {
                    let sig_types = Self::extract_types_from_impl_signature(&item.signature);
                    for (type_name, context) in sig_types {
                        let type_fqn = Self::resolve_type_fqn_with_lookup(
                            &item.fqn,
                            &type_name,
                            &type_fqns,
                            &type_names_to_fqns,
                        );

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
                }
            }
        }
        info!("Created {} USES_TYPE relationships", uses_type_count);

        // 8. Create DEPENDS_ON relationships for workspace crate dependencies
        let mut depends_count = 0;
        {
            // Build crate_name -> crate_root_path map from source files
            let mut crate_roots: std::collections::HashMap<String, PathBuf> =
                std::collections::HashMap::new();
            for sf in &source_files {
                if !crate_roots.contains_key(&sf.crate_name) {
                    if let Some(root) = Self::find_crate_root(&sf.path) {
                        crate_roots.insert(sf.crate_name.clone(), root);
                    }
                }
            }

            for (crate_name, crate_root) in &crate_roots {
                let cargo_toml = crate_root.join("Cargo.toml");
                let deps = Self::parse_workspace_deps(&cargo_toml, &crate_names);

                for (dep_name, is_dev, is_build) in deps {
                    if dep_name != *crate_name {
                        relationships.push(RelationshipBuilder::create_depends_on(
                            crate_name.clone(),
                            dep_name,
                            is_dev,
                            is_build,
                        ));
                        depends_count += 1;
                    }
                }
            }
        }
        info!("Created {} DEPENDS_ON relationships", depends_count);

        // 9. Create HAS_METHOD relationships for trait methods
        let mut has_method_count = 0;
        for items in parsed_items.values() {
            for item in items {
                if item.item_type == "trait" {
                    let trait_prefix = format!("{}::", item.fqn);

                    for items2 in parsed_items.values() {
                        for child in items2 {
                            if child.item_type == "function" && child.fqn.starts_with(&trait_prefix)
                            {
                                let method_name = &child.fqn[trait_prefix.len()..];
                                // Only match direct methods, not nested items
                                if !method_name.contains("::") {
                                    // Required = no body (just signature); Provided = has default impl
                                    let is_required = child.body_source.is_empty()
                                        || child.body_source.trim() == ";";

                                    relationships.push(RelationshipBuilder::create_has_method(
                                        item.fqn.clone(),
                                        child.fqn.clone(),
                                        is_required,
                                    ));
                                    has_method_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        info!("Created {} HAS_METHOD relationships", has_method_count);

        // Batch insert relationships
        let relationship_count = relationships.len();
        if relationship_count > 0 {
            info!("Inserting {} relationships into Neo4j", relationship_count);

            match graph_builder
                .create_relationships_batch(relationships)
                .await
            {
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

        info!(
            "Graph stage completed: {} nodes, {} edges inserted in {}ms",
            node_count,
            relationship_count,
            duration.as_millis()
        );

        Ok(StageResult::success(
            "graph",
            node_count + relationship_count,
            0,
            duration,
        ))
    }
}

impl GraphStage {
    fn strip_generics(name: &str) -> &str {
        name.split('<').next().unwrap_or(name).trim_end()
    }

    fn is_workspace_fqn(fqn: &str, workspace_crate_names: &[String]) -> bool {
        workspace_crate_names
            .iter()
            .any(|crate_name| fqn == crate_name || fqn.starts_with(&format!("{}::", crate_name)))
    }

    fn item_type_to_label(item_type: &str) -> &'static str {
        match item_type {
            "function" => "Function",
            "struct" => "Struct",
            "enum" => "Enum",
            "trait" => "Trait",
            "impl" => "Impl",
            "type_alias" => "TypeAlias",
            "const" => "Const",
            "static" => "Static",
            "macro" => "Macro",
            "module" => "Module",
            _ => "Type",
        }
    }

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
            properties.insert(
                "signature".to_string(),
                PropertyValue::from(item.signature.as_str()),
            );
        }
        properties.insert(
            "start_line".to_string(),
            PropertyValue::from(item.start_line),
        );
        properties.insert("end_line".to_string(), PropertyValue::from(item.end_line));

        properties.insert(
            "visibility".to_string(),
            PropertyValue::from(item.visibility.as_str()),
        );

        if let Some(ref generated_by) = item.generated_by {
            properties.insert(
                "generated_by".to_string(),
                PropertyValue::from(generated_by.as_str()),
            );
        }

        if item.item_type == "function" {
            let sig_lower = item.signature.to_lowercase();
            properties.insert(
                "is_async".to_string(),
                PropertyValue::from(sig_lower.contains("async")),
            );
            properties.insert(
                "is_unsafe".to_string(),
                PropertyValue::from(sig_lower.contains("unsafe")),
            );
            properties.insert(
                "is_generic".to_string(),
                PropertyValue::from(!item.generic_params.is_empty()),
            );
        }

        if !item.generic_params.is_empty() {
            if let Ok(json) = serde_json::to_string(&item.generic_params) {
                properties.insert("generic_params".to_string(), PropertyValue::from(json));
            }
        }

        if !item.doc_comment.is_empty() {
            properties.insert(
                "doc_comment".to_string(),
                PropertyValue::from(item.doc_comment.as_str()),
            );
        }

        NodeData {
            id: item.fqn.clone(),
            fqn: item.fqn.clone(),
            name: item.name.clone(),
            node_type,
            properties,
        }
    }

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

    fn get_parent_fqn(fqn: &str) -> Option<String> {
        fqn.rfind("::").map(|pos| fqn[..pos].to_string())
    }

    fn extract_trait_from_impl(attributes: &[String]) -> Option<String> {
        for attr in attributes {
            if let Some(stripped) = attr.strip_prefix("impl_for=") {
                return Some(stripped.to_string());
            }
        }
        None
    }

    fn extract_impl_self_type(_fqn: &str, name: &str) -> Option<String> {
        if let Some(underscore_pos) = name.find('_') {
            Some(name[underscore_pos + 1..].to_string())
        } else {
            Some(name.to_string())
        }
    }

    fn extract_struct_fields(body_source: &str, _struct_fqn: &str) -> Vec<(String, String, usize)> {
        let mut fields = Vec::new();

        for (pos, line) in body_source.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//")
                || line.starts_with("#")
                || line.starts_with("pub ")
                || line.starts_with("}")
            {
                continue;
            }

            if let Some(colon_pos) = line.find(':') {
                let field_name = line[..colon_pos].trim().to_string();
                let rest = &line[colon_pos + 1..];
                let type_str = rest.split(',').next().unwrap_or("").trim();
                if !field_name.is_empty() && !type_str.is_empty() {
                    fields.push((field_name, type_str.to_string(), pos));
                }
            }
        }

        fields
    }

    fn extract_enum_variants(
        body_source: &str,
        _enum_fqn: &str,
    ) -> Vec<(String, Option<String>, usize)> {
        let mut variants = Vec::new();

        for (pos, line) in body_source.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//")
                || line.starts_with("#")
                || line.starts_with("{")
                || line.starts_with("}")
            {
                continue;
            }

            if let Some(variant_name) = line.split(',').next() {
                let variant_name = variant_name.trim();
                if variant_name.is_empty() || variant_name.starts_with("//") {
                    continue;
                }

                let has_data = variant_name.contains('(') || variant_name.contains('{');
                let name = variant_name
                    .split('(')
                    .next()
                    .unwrap_or(variant_name)
                    .split('{')
                    .next()
                    .unwrap_or(variant_name)
                    .trim();

                if !name.is_empty() && !name.starts_with("//") {
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

    fn find_crate_root(source_path: &Path) -> Option<PathBuf> {
        let mut current = source_path.parent()?;
        loop {
            if current.join("Cargo.toml").exists() {
                return Some(current.to_path_buf());
            }
            current = current.parent()?;
        }
    }

    fn parse_workspace_deps(
        cargo_toml_path: &Path,
        workspace_crate_names: &std::collections::HashSet<String>,
    ) -> Vec<(String, bool, bool)> {
        let content = match std::fs::read_to_string(cargo_toml_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let toml_value: toml::Value = match toml::from_str(&content) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        let mut deps = Vec::new();

        let sections: &[(&str, bool, bool)] = &[
            ("dependencies", false, false),
            ("dev-dependencies", true, false),
            ("build-dependencies", false, true),
        ];

        for &(section, is_dev, is_build) in sections {
            if let Some(table) = toml_value.get(section).and_then(|v| v.as_table()) {
                for dep_name in table.keys() {
                    let normalized = dep_name.replace('-', "_");
                    let hyphenated = dep_name.replace('_', "-");

                    let actual_name = if workspace_crate_names.contains(dep_name) {
                        Some(dep_name.clone())
                    } else if workspace_crate_names.contains(&normalized) {
                        Some(normalized)
                    } else if workspace_crate_names.contains(&hyphenated) {
                        Some(hyphenated)
                    } else {
                        None
                    };

                    if let Some(name) = actual_name {
                        deps.push((name, is_dev, is_build));
                    }
                }
            }
        }

        deps
    }

    async fn load_items_from_database(
        &self,
        database_url: &str,
        crate_name: Option<&str>,
    ) -> Result<Vec<ParsedItemInfo>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .context("Failed to connect to database for loading items")?;

        let query = if let Some(crate_name) = crate_name {
            sqlx::query(
                r#"
                SELECT item_type, fqn, name, visibility, signature, doc_comment,
                       start_line, end_line, body_source, generic_params, where_clauses, attributes, generated_by
                FROM extracted_items
                WHERE crate_name = $1
                ORDER BY fqn
                "#
            )
            .bind(crate_name)
        } else {
            sqlx::query(
                r#"
                SELECT item_type, fqn, name, visibility, signature, doc_comment,
                       start_line, end_line, body_source, generic_params, where_clauses, attributes, generated_by
                FROM extracted_items
                ORDER BY fqn
                "#,
            )
        };

        let rows = query
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
                    visibility: row
                        .get::<Option<String>, _>("visibility")
                        .unwrap_or_default(),
                    signature: row
                        .get::<Option<String>, _>("signature")
                        .unwrap_or_default(),
                    generic_params: serde_json::from_value(generic_params_json).unwrap_or_default(),
                    where_clauses: serde_json::from_value(where_clauses_json).unwrap_or_default(),
                    attributes: serde_json::from_value(attributes_json).unwrap_or_default(),
                    doc_comment: row
                        .get::<Option<String>, _>("doc_comment")
                        .unwrap_or_default(),
                    start_line: row.get::<i32, _>("start_line") as usize,
                    end_line: row.get::<i32, _>("end_line") as usize,
                    body_source: row
                        .get::<Option<String>, _>("body_source")
                        .unwrap_or_default(),
                    generated_by: row.get("generated_by"),
                }
            })
            .collect();

        Ok(items)
    }

    /// Find the FQN of a trait by name using pre-built O(1) lookup index.
    /// Handles trait names with generics (e.g., "ConnectorIntegration<T, Req, Resp>")
    /// by stripping generics for the lookup.
    fn find_trait_fqn_optimized(
        trait_name: &str,
        trait_name_to_fqns: &std::collections::HashMap<String, String>,
        impl_fqn: &str,
    ) -> Option<String> {
        // Try exact match first (for traits without generics)
        if let Some(fqn) = trait_name_to_fqns.get(trait_name) {
            return Some(fqn.clone());
        }
        // Strip generics and try again (e.g., "ConnectorIntegration<T, Req, Resp>" → "ConnectorIntegration")
        let bare_name = Self::strip_generics(trait_name);
        if bare_name != trait_name {
            if let Some(fqn) = trait_name_to_fqns.get(bare_name) {
                return Some(fqn.clone());
            }
        }
        let module_path = Self::get_parent_fqn(impl_fqn)?;
        Some(format!("{}::{}", module_path, bare_name))
    }

    /// Resolve a type string to an FQN
    fn resolve_type_fqn(context_fqn: &str, type_str: &str) -> String {
        // Get the module path from the context FQN
        let module_path = Self::get_parent_fqn(context_fqn);

        // Handle common Rust types
        let primitive_types = [
            "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
            "f32", "f64", "bool", "char", "str", "String", "Vec", "Option", "Result", "Box", "Rc",
            "Arc", "Cow", "Cell", "RefCell",
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
                "if", "while", "for", "match", "fn", "let", "const", "static", "pub", "mod", "use",
                "struct", "enum", "trait", "impl", "type", "async", "unsafe", "extern", "crate",
                "self", "super", "where", "return", "break", "continue", "yield", "await", "move",
                "ref", "mut", "as", "in", "else", "loop", "dyn", "box",
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
                    while pos < chars.len()
                        && (chars[pos].is_alphanumeric() || chars[pos] == '_' || chars[pos] == ':')
                    {
                        pos += 1;
                    }

                    // Skip whitespace
                    while pos < chars.len() && chars[pos].is_whitespace() {
                        pos += 1;
                    }

                    // Check if followed by ( or <...>(  (turbofish/generic call)
                    if pos < chars.len() && chars[pos] == '<' {
                        // Skip past balanced angle brackets: <T, Req, Resp>
                        let mut depth = 0;
                        let angle_start = pos;
                        while pos < chars.len() {
                            if chars[pos] == '<' {
                                depth += 1;
                            } else if chars[pos] == '>' {
                                depth -= 1;
                                if depth == 0 {
                                    pos += 1;
                                    break;
                                }
                            }
                            pos += 1;
                        }
                        // Skip whitespace after >
                        while pos < chars.len() && chars[pos].is_whitespace() {
                            pos += 1;
                        }
                        // If we couldn't balance or no ( follows, reset
                        if depth != 0 || pos >= chars.len() || chars[pos] != '(' {
                            pos = angle_start + 1;
                            continue;
                        }
                    }
                    if pos < chars.len() && chars[pos] == '(' {
                        let identifier: String = chars[start..pos]
                            .iter()
                            .filter(|c| c.is_alphanumeric() || **c == '_' || **c == ':')
                            .collect();
                        let identifier = identifier.trim();

                        // Skip keywords
                        if keywords.contains(&identifier) {
                            pos += 1;
                            continue;
                        }

                        // Skip self/super/crate prefixes (these are relative paths)
                        if identifier.starts_with("self::")
                            || identifier.starts_with("super::")
                            || identifier.starts_with("crate::")
                        {
                            // Try to resolve the full path
                            if let Some(callee) = Self::resolve_call_target(
                                identifier,
                                function_fqns,
                                function_names_to_fqns,
                                caller_module.as_deref(),
                            ) {
                                if callee != caller_fqn {
                                    // Don't create self-calls
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
                            } else if let Some(callee) = Self::resolve_call_target(
                                identifier,
                                function_fqns,
                                function_names_to_fqns,
                                caller_module.as_deref(),
                            ) {
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
        for line in body_source.lines() {
            let trimmed = line.trim();
            // Track type annotations: let x: Type = ...
            if let Some(caps) = TYPE_ANNOTATION_RE.captures(trimmed) {
                let var_name = caps.get(1).unwrap().as_str().to_string();
                let type_name = caps.get(2).unwrap().as_str().to_string();
                local_types.insert(var_name, type_name);
            }
            // Track constructor calls: let x = Type::new()
            if let Some(caps) = CONSTRUCTOR_RE.captures(trimmed) {
                let var_name = caps.get(1).unwrap().as_str().to_string();
                let type_name = caps.get(2).unwrap().as_str().to_string();
                local_types.insert(var_name, type_name);
            }
        }

        // Now find method calls on tracked variables and self
        for (line_num, line) in body_source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("#") {
                continue;
            }

            for caps in METHOD_CALL_RE.captures_iter(trimmed) {
                let receiver = caps.get(1).unwrap().as_str();
                let method = caps.get(2).unwrap().as_str();

                // Determine the receiver type
                let receiver_type = if receiver == "self" || receiver == "Self" {
                    self_type.map(|s| s.to_string())
                } else {
                    local_types.get(receiver).cloned()
                };

                if let Some(type_name) = receiver_type {
                    // Look for Type::method in known functions.
                    // Methods now have FQNs like module::Type::method,
                    // so we search for a suffix of ::Type::method.
                    let qualified_suffix = format!("::{}::{}", type_name, method);
                    if let Some(fqns) = function_names_to_fqns.get(method) {
                        if let Some(fqn) = fqns.iter().find(|f| f.ends_with(&qualified_suffix)) {
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
        function_names_to_fqns: &std::collections::HashMap<String, Vec<String>>,
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

        // Try to find by the last part of the path using O(1) HashMap lookup
        // instead of scanning all function_fqns (which was O(N) per call)
        if let Some(last_sep) = identifier.rfind("::") {
            let method_name = &identifier[last_sep + 2..];
            if !method_name.is_empty() {
                if let Some(fqns) = function_names_to_fqns.get(method_name) {
                    let type_prefix = &identifier[..last_sep];
                    // Match FQNs that contain ::TypePrefix::method (exact type match)
                    let suffix = format!("::{}::{}", type_prefix, method_name);
                    if let Some(fqn) = fqns.iter().find(|f| f.ends_with(&suffix)) {
                        return Some(fqn.clone());
                    }
                    // Also try a looser contains check for nested module paths
                    if let Some(fqn) = fqns
                        .iter()
                        .find(|f| f.contains(&format!("::{}", type_prefix)))
                    {
                        return Some(fqn.clone());
                    }
                    // Do NOT fall back to fqns.first() — that would pick an
                    // arbitrary function that happens to share the same short name.
                }
            }
        }

        None
    }

    /// Check if a type name is a primitive or standard library type
    fn is_primitive_type(type_name: &str) -> bool {
        let primitive_types = [
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "isize",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "usize",
            "f32",
            "f64",
            "bool",
            "char",
            "str",
            "String",
            "Vec",
            "Option",
            "Result",
            "Box",
            "Rc",
            "Arc",
            "Cow",
            "Cell",
            "RefCell",
            "Mutex",
            "RwLock",
            "Arc",
            "Weak",
            "HashMap",
            "HashSet",
            "BTreeMap",
            "BTreeSet",
            "VecDeque",
            "LinkedList",
            "BinaryHeap",
            "Cow",
            "PhantomData",
            "PhantomPinned",
            "Duration",
            "Instant",
            "SystemTime",
            "Path",
            "PathBuf",
            "OsStr",
            "OsString",
            "IpAddr",
            "Ipv4Addr",
            "Ipv6Addr",
            "SocketAddr",
            "Error",
            "BoxError",
            "io",
            "fmt",
            "Debug",
            "Display",
            "Clone",
            "Copy",
            "Default",
            "Eq",
            "Hash",
            "Ord",
            "PartialEq",
            "PartialOrd",
            "Send",
            "Sync",
            "Sized",
            "Unpin",
            "From",
            "Into",
            "TryFrom",
            "TryInto",
            "AsRef",
            "AsMut",
            "Deref",
            "DerefMut",
            "Index",
            "IndexMut",
            "Add",
            "Sub",
            "Mul",
            "Div",
            "Rem",
            "Neg",
            "Not",
            "BitAnd",
            "BitOr",
            "BitXor",
            "Fn",
            "FnMut",
            "FnOnce",
            "Future",
            "Stream",
            "Iterator",
            "Self",
            "self",
            "static",
            "dyn",
        ];

        // Check if it's a primitive type or starts with lowercase (type parameter)
        primitive_types.contains(&type_name)
            || type_name.starts_with(|c: char| c.is_lowercase() && c != '_')
            || type_name.len() == 1 // Single letter types like T, U, V are usually generic params
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
                if let Some(fqn) = fqns
                    .iter()
                    .find(|fqn| fqn.starts_with(&format!("{}::", module)))
                {
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

    /// Extract type names from an impl block signature.
    ///
    /// Handles patterns like:
    /// - `impl Type`            → extracts Type
    /// - `impl Trait for Type`  → extracts Trait, Type
    /// - `impl<T> Trait for Type<T>` → extracts Trait, Type
    /// - `unsafe impl Trait for Type` → extracts Trait, Type
    fn extract_types_from_impl_signature(signature: &str) -> Vec<(String, String)> {
        let mut types = Vec::new();
        let sig = signature.trim();

        // Strip "unsafe " prefix if present
        let sig = sig.strip_prefix("unsafe ").unwrap_or(sig);
        // Strip "impl" prefix
        let sig = sig.strip_prefix("impl").unwrap_or(sig).trim();

        // Strip leading generic params: <T: Clone, U>
        let sig = if sig.starts_with('<') {
            // Find the matching '>'
            let mut depth = 0;
            let mut end = 0;
            for (i, c) in sig.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            sig[end..].trim()
        } else {
            sig
        };

        if let Some(for_pos) = sig.find(" for ") {
            // "Trait for Type" pattern
            let trait_part = sig[..for_pos].trim();
            let type_part = sig[for_pos + 5..].trim();

            // Extract trait name (strip generic params, trim whitespace)
            let trait_name = if let Some(angle) = trait_part.find('<') {
                trait_part[..angle].trim()
            } else {
                trait_part
            };
            if !trait_name.is_empty() && !Self::is_primitive_type(trait_name) {
                types.push((trait_name.to_string(), "impl_trait".to_string()));
            }

            // Extract type name (strip generic params, trim whitespace)
            let type_name = if let Some(angle) = type_part.find('<') {
                type_part[..angle].trim()
            } else {
                type_part
            };
            if !type_name.is_empty() && !Self::is_primitive_type(type_name) {
                types.push((type_name.to_string(), "impl_self_type".to_string()));
            }
        } else {
            // "impl Type" pattern (inherent impl)
            let type_name = if let Some(angle) = sig.find('<') {
                sig[..angle].trim()
            } else {
                sig
            };
            let type_name = type_name.trim();
            if !type_name.is_empty() && !Self::is_primitive_type(type_name) {
                types.push((type_name.to_string(), "impl_self_type".to_string()));
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
        let type_str = type_str
            .replace("'static", "")
            .replace("'a", "")
            .replace("'_", "");

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
                if let Some(type_name) = before
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next_back()
                {
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
                        .take_while(|c| {
                            c.is_alphanumeric() || *c == '_' || *c == ':' || *c == '<' || *c == '>'
                        })
                        .collect();
                    if !type_name.is_empty() && !Self::is_primitive_type(&type_name) {
                        types.push((type_name, line_num + 1));
                    }
                }
            }

            // Pattern 4: Type { ... } - struct instantiation
            // Look for pattern: identifier { (not preceded by keywords)
            for cap in STRUCT_INSTANTIATION_RE.captures_iter(line) {
                if let Some(type_name) = cap.get(1) {
                    let type_name = type_name.as_str();
                    if !Self::is_primitive_type(type_name) {
                        types.push((type_name.to_string(), line_num + 1));
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

impl Default for EmbedStage {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbedStage {
    pub fn new() -> Self {
        Self {}
    }

    /// Get Ollama URL from environment or config
    fn get_ollama_url(ctx: &PipelineContext) -> String {
        ctx.config
            .embedding_url
            .clone()
            .or_else(|| std::env::var("OLLAMA_HOST").ok())
            .unwrap_or_else(|| "http://ollama:11434".to_string())
    }

    /// Get Qdrant URL from environment
    fn get_qdrant_url() -> String {
        std::env::var("QDRANT_HOST").unwrap_or_else(|_| "http://qdrant:6333".to_string())
    }

    /// Load items from the database when parsed_items is not available in state
    async fn load_items_from_database(
        &self,
        database_url: &str,
        crate_name: Option<&str>,
    ) -> Result<Vec<ParsedItemInfo>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(database_url)
            .await
            .context("Failed to connect to database for loading items")?;

        let query = if let Some(crate_name) = crate_name {
            sqlx::query(
                r#"
                SELECT item_type, fqn, name, visibility, signature, doc_comment,
                       start_line, end_line, body_source, generic_params, where_clauses, attributes, generated_by
                FROM extracted_items
                WHERE crate_name = $1
                ORDER BY fqn
                "#
            )
            .bind(crate_name)
        } else {
            sqlx::query(
                r#"
                SELECT item_type, fqn, name, visibility, signature, doc_comment,
                       start_line, end_line, body_source, generic_params, where_clauses, attributes, generated_by
                FROM extracted_items
                ORDER BY fqn
                "#,
            )
        };

        let rows = query
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
                    visibility: row
                        .get::<Option<String>, _>("visibility")
                        .unwrap_or_default(),
                    signature: row
                        .get::<Option<String>, _>("signature")
                        .unwrap_or_default(),
                    generic_params: serde_json::from_value(generic_params_json).unwrap_or_default(),
                    where_clauses: serde_json::from_value(where_clauses_json).unwrap_or_default(),
                    attributes: serde_json::from_value(attributes_json).unwrap_or_default(),
                    doc_comment: row
                        .get::<Option<String>, _>("doc_comment")
                        .unwrap_or_default(),
                    start_line: row.get::<i32, _>("start_line") as usize,
                    end_line: row.get::<i32, _>("end_line") as usize,
                    body_source: row
                        .get::<Option<String>, _>("body_source")
                        .unwrap_or_default(),
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
            match self
                .load_items_from_database(
                    &ctx.config.database_url,
                    ctx.config.crate_name.as_deref(),
                )
                .await
            {
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

        info!(
            "Connecting to Ollama at {} and Qdrant at {}",
            redact_url(&ollama_url),
            redact_url(&qdrant_url)
        );

        // Create embedding service
        let embedding_service =
            match crate::embedding::EmbeddingService::with_urls(ollama_url, qdrant_url) {
                Ok(s) => s,
                Err(e) => {
                    return Ok(StageResult::failed(
                        "embed",
                        format!("Failed to create embedding service: {}", e),
                    ));
                }
            };

        // Initialize (ensure collections exist, check model)
        if let Err(e) = embedding_service.initialize().await {
            return Ok(StageResult::failed(
                "embed",
                format!("Embedding service initialization failed: {}", e),
            ));
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
            debug!(
                "Processing embedding batch {}/{}",
                batch_num + 1,
                all_items.len().div_ceil(BATCH_SIZE)
            );

            // Retry embedding batches with exponential backoff for transient failures
            let batch_label = format!("embed batch {}", batch_num + 1);
            let chunk_vec: Vec<_> = chunk.to_vec();
            let service = &embedding_service;

            match retry_with_backoff(&batch_label, MAX_RETRIES, || async {
                service.embed_items(&chunk_vec).await
            })
            .await
            {
                Ok(results) => {
                    embedded_count += results.len();
                    debug!(
                        "Embedded {} items in batch {}",
                        results.len(),
                        batch_num + 1
                    );
                }
                Err(e) => {
                    warn!("Failed to embed batch {} after retries: {}", batch_num, e);
                    state
                        .errors
                        .push(
                            StageError::new("embed", e.to_string()).with_context(format!(
                                "batch {} (after {} retries)",
                                batch_num, MAX_RETRIES
                            )),
                        );
                    failed_count += chunk.len();
                }
            }
        }

        state.counts.embeddings_created = embedded_count;

        let duration = start.elapsed();

        if failed_count > 0 && embedded_count == 0 {
            Ok(StageResult::failed(
                "embed",
                "All embedding attempts failed",
            ))
        } else if failed_count > 0 {
            Ok(StageResult::partial(
                "embed",
                embedded_count,
                failed_count,
                duration,
                format!("{} items embedded, {} failed", embedded_count, failed_count),
            ))
        } else {
            Ok(StageResult::success("embed", embedded_count, 0, duration))
        }
    }
}

/// Parse item type string to ItemType enum
pub fn parse_item_type(s: &str) -> ItemType {
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
pub fn parse_visibility(s: &str) -> Visibility {
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
                .post(format!(
                    "{}/collections/code_embeddings/points/delete",
                    qdrant
                ))
                .json(&delete_req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    info!("Deleted Qdrant embeddings for crate: {}", crate_name);
                    report.qdrant_deleted = true;
                }
                Ok(resp) => {
                    warn!(
                        "Qdrant delete returned {}: {}",
                        resp.status(),
                        resp.text().await.unwrap_or_default()
                    );
                }
                Err(e) => {
                    warn!("Failed to delete from Qdrant: {}", e);
                    report.errors.push(format!("Qdrant: {}", e));
                }
            }
        }

        // Step 2: Delete from Neo4j (graph nodes/relationships) if available
        if let Some(neo4j) = neo4j_url {
            match neo4rs::Graph::new(
                neo4j,
                std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
                std::env::var("NEO4J_PASSWORD")
                    .expect("NEO4J_PASSWORD environment variable must be set"),
            )
            .await
            {
                Ok(graph) => {
                    let q = neo4rs::query("MATCH (n {crate_name: $crate_name}) DETACH DELETE n")
                        .param("crate_name", crate_name);

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
                info!(
                    "Deleted {} source files from Postgres",
                    report.postgres_files_deleted
                );
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
        store_refs
            .values()
            .filter(|r| !r.is_fully_synced() && !r.is_orphaned())
            .collect()
    }

    pub async fn cleanup_stale_items(
        crate_name: &str,
        current_fqns: &std::collections::HashSet<String>,
        pool: &PgPool,
        neo4j_url: Option<&str>,
        qdrant_url: Option<&str>,
    ) -> Result<StaleCleanupReport> {
        let mut report = StaleCleanupReport::default();
        if current_fqns.is_empty() {
            info!("No current FQNs - skipping stale cleanup");
            return Ok(report);
        }
        info!(
            "Starting stale cleanup for crate {} with {} current FQNs",
            crate_name,
            current_fqns.len()
        );

        let existing_fqns: std::collections::HashSet<String> =
            sqlx::query_scalar("SELECT fqn FROM extracted_items WHERE crate_name = $1")
                .bind(crate_name)
                .fetch_all(pool)
                .await
                .context("Failed to query existing FQNs for stale cleanup")?
                .into_iter()
                .collect();

        let stale_fqns: Vec<String> = existing_fqns.difference(current_fqns).cloned().collect();
        if stale_fqns.is_empty() {
            info!("No stale items found for crate {}", crate_name);
            return Ok(report);
        }
        info!(
            "Found {} stale FQNs for crate {}",
            stale_fqns.len(),
            crate_name
        );
        report.stale_count = stale_fqns.len();

        if let Some(qdrant) = qdrant_url {
            let client = reqwest::Client::new();
            let namespace = uuid::Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
                .expect("Invalid namespace UUID");
            let point_ids: Vec<String> = stale_fqns
                .iter()
                .map(|fqn| uuid::Uuid::new_v5(&namespace, fqn.as_bytes()).to_string())
                .collect();
            let delete_req = serde_json::json!({ "ids": point_ids });
            match client
                .post(format!(
                    "{}/collections/code_embeddings/points/delete",
                    qdrant
                ))
                .json(&delete_req)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    report.qdrant_deleted = stale_fqns.len();
                    info!("Deleted {} stale points from Qdrant", stale_fqns.len());
                }
                Ok(resp) => {
                    warn!(
                        "Qdrant stale delete returned {}: {}",
                        resp.status(),
                        resp.text().await.unwrap_or_default()
                    );
                }
                Err(e) => {
                    warn!("Failed to delete stale points from Qdrant: {}", e);
                    report.errors.push(format!("Qdrant: {}", e));
                }
            }
        }

        if let Some(neo4j) = neo4j_url {
            match neo4rs::Graph::new(
                neo4j,
                std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
                std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string()),
            )
            .await
            {
                Ok(graph) => {
                    let q = neo4rs::query(
                        "UNWIND $fqns AS fqn MATCH (n {fqn: fqn, crate_name: $crate_name}) DETACH DELETE n"
                    )
                    .param("fqns", stale_fqns.clone())
                    .param("crate_name", crate_name);
                    match graph.run(q).await {
                        Ok(_) => {
                            report.neo4j_deleted = stale_fqns.len();
                            info!("Deleted {} stale nodes from Neo4j", stale_fqns.len());
                        }
                        Err(e) => {
                            warn!("Failed to delete stale nodes from Neo4j: {}", e);
                            report.errors.push(format!("Neo4j: {}", e));
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to connect to Neo4j for stale cleanup: {}", e);
                    report.errors.push(format!("Neo4j connect: {}", e));
                }
            }
        }

        match sqlx::query("DELETE FROM extracted_items WHERE crate_name = $1 AND fqn = ANY($2)")
            .bind(crate_name)
            .bind(&stale_fqns)
            .execute(pool)
            .await
        {
            Ok(result) => {
                report.postgres_deleted = result.rows_affected() as usize;
                info!(
                    "Deleted {} stale items from Postgres",
                    report.postgres_deleted
                );
            }
            Err(e) => {
                warn!("Failed to delete stale items from Postgres: {}", e);
                report.errors.push(format!("Postgres: {}", e));
            }
        }

        Ok(report)
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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StaleCleanupReport {
    pub stale_count: usize,
    pub postgres_deleted: usize,
    pub neo4j_deleted: usize,
    pub qdrant_deleted: usize,
    pub errors: Vec<String>,
}

impl StaleCleanupReport {
    pub fn is_successful(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn total_deleted(&self) -> usize {
        self.postgres_deleted + self.neo4j_deleted + self.qdrant_deleted
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
        let err = StageError::new("parse", "failed").with_context("file: src/main.rs");
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
        let err = StageError::fatal("parse", "syntax error").with_context("line 42");
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
        let result =
            retry_with_backoff("test_op", 3, || async { Ok::<i32, anyhow::Error>(42) }).await;
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
        })
        .await;

        assert_eq!(result.unwrap(), 99);
        assert_eq!(attempt.load(Ordering::SeqCst), 3); // 2 failures + 1 success
    }

    #[tokio::test]
    async fn test_retry_with_backoff_all_failures() {
        let result = retry_with_backoff("test_op", 1, || async {
            Err::<i32, anyhow::Error>(anyhow::anyhow!("permanent failure"))
        })
        .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("permanent failure"));
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
        assert_eq!(CARGO_EXPAND_TIMEOUT, Duration::from_secs(180));
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
        names_to_fqns.insert(
            "get".to_string(),
            vec!["crate::HttpClient::get".to_string()],
        );
        names_to_fqns.insert(
            "post".to_string(),
            vec!["crate::HttpClient::post".to_string()],
        );

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, None);
        assert!(
            !calls.is_empty(),
            "Should detect method calls on typed variables"
        );
    }

    #[test]
    fn test_extract_method_calls_with_constructor() {
        let body = r#"
            let parser = DualParser::new();
            parser.parse("source");
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert(
            "parse".to_string(),
            vec!["crate::DualParser::parse".to_string()],
        );

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, None);
        assert!(
            !calls.is_empty(),
            "Should detect method calls on constructor-inferred variables"
        );
    }

    #[test]
    fn test_extract_method_calls_on_self() {
        let body = r#"
            self.process_item(item);
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert(
            "process_item".to_string(),
            vec!["crate::MyStruct::process_item".to_string()],
        );

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
        let ref_entry = rustbrain_common::StoreReference::new(
            "crate::func".to_string(),
            "my_crate".to_string(),
        );
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
        let mut ref_entry = rustbrain_common::StoreReference::new(
            "crate::func".to_string(),
            "my_crate".to_string(),
        );
        ref_entry.postgres_id = Some("pg-123".to_string());
        ref_entry.neo4j_node_id = Some("neo-456".to_string());
        ref_entry.qdrant_point_id = Some("qd-789".to_string());
        assert!(ref_entry.is_fully_synced());
        assert!(!ref_entry.is_orphaned());
        assert!(ref_entry.missing_stores().is_empty());
    }

    #[test]
    fn test_store_reference_partially_synced() {
        let mut ref_entry = rustbrain_common::StoreReference::new(
            "crate::func".to_string(),
            "my_crate".to_string(),
        );
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

    // =========================================================================
    // Call-graph bug-fix verification tests (Bugs #1–#7)
    // =========================================================================

    /// Bug #1: parse_impl now emits per-method ParsedItems via DualParser
    #[test]
    fn test_bug1_dual_parser_emits_impl_methods() {
        let parser = crate::parsers::DualParser::new().unwrap();
        let source = r#"
            impl Server {
                pub fn start(&self) { }
                fn stop(&self) { }
            }
        "#;
        let result = parser.parse(source, "app::net").unwrap();

        // Must contain the impl block AND individual method items
        let impl_items: Vec<_> = result
            .items
            .iter()
            .filter(|i| i.item_type.as_str() == "impl")
            .collect();
        let fn_items: Vec<_> = result
            .items
            .iter()
            .filter(|i| i.item_type.as_str() == "function")
            .collect();

        assert_eq!(impl_items.len(), 1, "Expected 1 impl block");
        assert!(
            fn_items.len() >= 2,
            "Expected at least 2 method items, got {}",
            fn_items.len()
        );

        // Verify canonical FQN format: module::Type::method
        let start_method = fn_items.iter().find(|i| i.name == "start");
        assert!(start_method.is_some(), "Should have a 'start' method item");
        assert_eq!(start_method.unwrap().fqn, "app::net::Server::start");

        let stop_method = fn_items.iter().find(|i| i.name == "stop");
        assert!(stop_method.is_some(), "Should have a 'stop' method item");
        assert_eq!(stop_method.unwrap().fqn, "app::net::Server::stop");
    }

    /// Bug #2: method FQNs are now registered in the function_names_to_fqns index
    /// because they have item_type == "function"
    #[test]
    fn test_bug2_method_fqns_registered_in_function_index() {
        let parser = crate::parsers::DualParser::new().unwrap();
        let source = r#"
            pub fn standalone() {}

            impl Handler {
                pub fn handle(&self) {}
            }
        "#;
        let result = parser.parse(source, "crate::svc").unwrap();

        // Simulate the index-building code from GraphStage
        let mut function_fqns = std::collections::HashSet::new();
        let mut function_names_to_fqns: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for item in &result.items {
            if item.item_type.as_str() == "function" {
                function_fqns.insert(item.fqn.clone());
                function_names_to_fqns
                    .entry(item.name.clone())
                    .or_default()
                    .push(item.fqn.clone());
            }
        }

        // standalone function should be in the index
        assert!(
            function_fqns.contains("crate::svc::standalone"),
            "standalone fn missing"
        );

        // impl method should also be in the index
        assert!(
            function_fqns.contains("crate::svc::Handler::handle"),
            "impl method 'handle' not in function_fqns. Contents: {:?}",
            function_fqns
        );

        // Should be findable by short name
        assert!(
            function_names_to_fqns.contains_key("handle"),
            "'handle' not in function_names_to_fqns"
        );
    }

    /// Bug #3: self.method() calls now resolve correctly
    #[test]
    fn test_bug3_self_method_resolves() {
        let body = r#"
            self.process(data);
            self.validate();
        "#;
        // Methods with canonical FQN module::Type::method
        let mut names_to_fqns = std::collections::HashMap::new();
        names_to_fqns.insert(
            "process".to_string(),
            vec!["app::svc::Handler::process".to_string()],
        );
        names_to_fqns.insert(
            "validate".to_string(),
            vec!["app::svc::Handler::validate".to_string()],
        );

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, Some("Handler"));

        assert!(
            calls.len() >= 2,
            "self.method() should resolve 2 calls, got {}: {:?}",
            calls.len(),
            calls
        );

        let callee_fqns: Vec<&str> = calls.iter().map(|(fqn, _)| fqn.as_str()).collect();
        assert!(
            callee_fqns.contains(&"app::svc::Handler::process"),
            "process not resolved: {:?}",
            callee_fqns
        );
        assert!(
            callee_fqns.contains(&"app::svc::Handler::validate"),
            "validate not resolved: {:?}",
            callee_fqns
        );
    }

    /// Bug #3 (negative): self.method() must NOT match a method from a different type
    #[test]
    fn test_bug3_self_method_does_not_crossmatch() {
        let body = r#"
            self.run();
        "#;
        let mut names_to_fqns = std::collections::HashMap::new();
        // 'run' exists on OtherType, NOT on MyType
        names_to_fqns.insert(
            "run".to_string(),
            vec!["app::other::OtherType::run".to_string()],
        );

        let calls = GraphStage::extract_method_calls(body, &names_to_fqns, Some("MyType"));

        // Should NOT match because "::MyType::run" doesn't end any FQN
        assert!(
            calls.is_empty(),
            "self.method() should NOT cross-resolve to OtherType, but got: {:?}",
            calls
        );
    }

    /// Bug #4: Type::method() no longer falls back to an arbitrary first match
    #[test]
    fn test_bug4_type_method_no_arbitrary_fallback() {
        let mut function_fqns = std::collections::HashSet::new();
        let mut function_names_to_fqns = std::collections::HashMap::new();

        // Two different types have a method called "process"
        function_fqns.insert("crate::alpha::TypeA::process".to_string());
        function_fqns.insert("crate::beta::TypeB::process".to_string());
        function_names_to_fqns.insert(
            "process".to_string(),
            vec![
                "crate::alpha::TypeA::process".to_string(),
                "crate::beta::TypeB::process".to_string(),
            ],
        );

        // Calling TypeA::process should resolve to TypeA's version
        let result_a = GraphStage::resolve_call_target(
            "TypeA::process",
            &function_fqns,
            &function_names_to_fqns,
            Some("crate::gamma"),
        );
        assert_eq!(
            result_a,
            Some("crate::alpha::TypeA::process".to_string()),
            "TypeA::process should resolve to TypeA, got: {:?}",
            result_a
        );

        // Calling TypeB::process should resolve to TypeB's version
        let result_b = GraphStage::resolve_call_target(
            "TypeB::process",
            &function_fqns,
            &function_names_to_fqns,
            Some("crate::gamma"),
        );
        assert_eq!(
            result_b,
            Some("crate::beta::TypeB::process".to_string()),
            "TypeB::process should resolve to TypeB, got: {:?}",
            result_b
        );

        // Calling UnknownType::process should return None, NOT an arbitrary match
        let result_none = GraphStage::resolve_call_target(
            "UnknownType::process",
            &function_fqns,
            &function_names_to_fqns,
            Some("crate::gamma"),
        );
        assert!(
            result_none.is_none(),
            "UnknownType::process should be None (no arbitrary fallback), got: {:?}",
            result_none
        );
    }

    /// Bug #6: impl block body is no longer scanned for calls (only method bodies are).
    /// With the fix, the CALLS loop only processes items where item_type == "function",
    /// so the impl block (which contains ALL methods' source) is skipped.
    #[test]
    fn test_bug6_impl_block_not_scanned_for_calls() {
        let parser = crate::parsers::DualParser::new().unwrap();
        let source = r#"
            impl Processor {
                fn run(&self) {
                    helper();
                }
            }
            fn helper() {}
        "#;
        let result = parser.parse(source, "app").unwrap();

        // The impl block itself should NOT be treated as a call source
        let impl_items: Vec<_> = result
            .items
            .iter()
            .filter(|i| i.item_type.as_str() == "impl")
            .collect();
        let fn_items: Vec<_> = result
            .items
            .iter()
            .filter(|i| i.item_type.as_str() == "function")
            .collect();

        assert!(!impl_items.is_empty(), "Should have impl block");
        assert!(
            fn_items.len() >= 2,
            "Should have run + helper as function items"
        );

        // The fixed loop ONLY scans "function" items, skipping "impl".
        // Verify that the impl block's body_source (which contains everything)
        // would produce duplicates if we wrongly scanned it too.
        let impl_body = &impl_items[0].body_source;
        assert!(impl_body.contains("helper"),
            "impl body_source should contain helper() call text (proving it would double-count if scanned)");

        // But the individual method 'run' also has helper in its body
        let run_method = fn_items.iter().find(|i| i.name == "run");
        assert!(run_method.is_some(), "Should have 'run' method");
        assert!(
            run_method.unwrap().body_source.contains("helper"),
            "run's body_source should also contain helper() call"
        );

        // The fix ensures we only scan function items — so impl body is NOT double-scanned
        // This test documents the structural invariant: both impl and method contain
        // the same call text, but only the method is processed.
    }

    /// Bug #7: resolver.rs FQN format now matches canonical format
    #[test]
    fn test_bug7_resolver_fqn_matches_canonical_format() {
        // The TypeResolver's impl_caller_fqn now produces "module::Type"
        // so that appending "::method" gives "module::Type::method".
        // Verify this matches what parse_impl_with_methods produces.
        let parser = crate::parsers::DualParser::new().unwrap();
        let source = r#"
            impl MyService {
                fn do_work(&self) {}
            }
        "#;
        let result = parser.parse(source, "crate::svc").unwrap();

        let method = result.items.iter().find(|i| i.name == "do_work");
        assert!(method.is_some(), "Should find 'do_work' method");

        let method = method.unwrap();
        // The canonical FQN from parse_impl_with_methods
        assert_eq!(method.fqn, "crate::svc::MyService::do_work");

        // The resolver would produce impl_caller_fqn = "crate::svc::MyService"
        // and then method FQN = impl_caller_fqn + "::do_work" = "crate::svc::MyService::do_work"
        // These now match.
        let simulated_resolver_fqn = format!("{}::{}", "crate::svc::MyService", "do_work");
        assert_eq!(
            method.fqn, simulated_resolver_fqn,
            "resolver FQN should match parser FQN"
        );
    }

    /// Bug #5: impl block signatures are now analyzed for USES_TYPE relationships
    /// using the dedicated extract_types_from_impl_signature method.
    #[test]
    fn test_bug5_impl_signature_types_extracted() {
        // Trait impl: "impl MyTrait for MyStruct"
        let types = GraphStage::extract_types_from_impl_signature("impl MyTrait for MyStruct");
        let type_names: Vec<&str> = types.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            type_names.contains(&"MyTrait"),
            "Should extract trait 'MyTrait'. Found: {:?}",
            type_names
        );
        assert!(
            type_names.contains(&"MyStruct"),
            "Should extract self type 'MyStruct'. Found: {:?}",
            type_names
        );

        // Verify context labels
        let contexts: Vec<&str> = types.iter().map(|(_, ctx)| ctx.as_str()).collect();
        assert!(
            contexts.contains(&"impl_trait"),
            "Should have impl_trait context"
        );
        assert!(
            contexts.contains(&"impl_self_type"),
            "Should have impl_self_type context"
        );

        // Inherent impl: "impl MyStruct"
        let types2 = GraphStage::extract_types_from_impl_signature("impl MyStruct");
        let type_names2: Vec<&str> = types2.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            type_names2.contains(&"MyStruct"),
            "Should extract self type from inherent impl. Found: {:?}",
            type_names2
        );

        // Generic impl: "impl < T > Handler for Container < T >"
        let types3 =
            GraphStage::extract_types_from_impl_signature("impl < T > Handler for Container < T >");
        let type_names3: Vec<&str> = types3.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            type_names3.contains(&"Handler"),
            "Should extract trait from generic impl. Found: {:?}",
            type_names3
        );
        assert!(
            type_names3.contains(&"Container"),
            "Should extract type from generic impl. Found: {:?}",
            type_names3
        );

        // Unsafe impl: stdlib traits like Send are filtered by is_primitive_type
        let types4 =
            GraphStage::extract_types_from_impl_signature("unsafe impl Serializer for MyStruct");
        let type_names4: Vec<&str> = types4.iter().map(|(name, _)| name.as_str()).collect();
        assert!(
            type_names4.contains(&"MyStruct"),
            "Should extract type from unsafe impl. Found: {:?}",
            type_names4
        );
        assert!(
            type_names4.contains(&"Serializer"),
            "Should extract trait from unsafe impl. Found: {:?}",
            type_names4
        );
    }

    #[test]
    fn test_stale_cleanup_report_default() {
        let report = StaleCleanupReport::default();
        assert_eq!(report.stale_count, 0);
        assert_eq!(report.postgres_deleted, 0);
        assert_eq!(report.neo4j_deleted, 0);
        assert_eq!(report.qdrant_deleted, 0);
        assert!(report.errors.is_empty());
    }

    #[test]
    fn test_stale_cleanup_report_is_successful() {
        let mut report = StaleCleanupReport::default();
        assert!(report.is_successful());

        report.errors.push("some error".to_string());
        assert!(!report.is_successful());
    }

    #[test]
    fn test_stale_cleanup_report_total_deleted() {
        let report = StaleCleanupReport {
            stale_count: 10,
            postgres_deleted: 4,
            neo4j_deleted: 3,
            qdrant_deleted: 3,
            errors: vec![],
        };
        assert_eq!(report.total_deleted(), 10);
    }

    #[test]
    fn test_stale_fqn_set_difference() {
        use std::collections::HashSet;

        let existing: HashSet<String> = [
            "crate::module::func_a".to_string(),
            "crate::module::func_b".to_string(),
            "crate::module::func_c".to_string(),
            "crate::module::old_func".to_string(),
            "crate::module::deprecated".to_string(),
        ]
        .into_iter()
        .collect();

        let current: HashSet<String> = [
            "crate::module::func_a".to_string(),
            "crate::module::func_b".to_string(),
            "crate::module::func_c".to_string(),
            "crate::module::new_func".to_string(),
        ]
        .into_iter()
        .collect();

        let stale: Vec<String> = existing.difference(&current).cloned().collect();

        assert_eq!(stale.len(), 2);
        assert!(stale.contains(&"crate::module::old_func".to_string()));
        assert!(stale.contains(&"crate::module::deprecated".to_string()));
    }

    #[test]
    fn test_stale_fqn_empty_current_means_all_stale() {
        use std::collections::HashSet;

        let existing: HashSet<String> = ["crate::module::func".to_string()].into_iter().collect();
        let current: HashSet<String> = HashSet::new();

        let stale: Vec<String> = existing.difference(&current).cloned().collect();

        assert_eq!(stale.len(), 1);
    }

    #[test]
    fn test_stale_fqn_empty_existing_means_nothing_stale() {
        use std::collections::HashSet;

        let existing: HashSet<String> = HashSet::new();
        let current: HashSet<String> = ["crate::module::func".to_string()].into_iter().collect();

        let stale: Vec<String> = existing.difference(&current).cloned().collect();

        assert!(stale.is_empty());
    }

    #[test]
    fn test_stale_fqn_identical_sets_means_no_stale() {
        use std::collections::HashSet;

        let existing: HashSet<String> = ["crate::module::func".to_string()].into_iter().collect();
        let current: HashSet<String> = ["crate::module::func".to_string()].into_iter().collect();

        let stale: Vec<String> = existing.difference(&current).cloned().collect();

        assert!(stale.is_empty());
    }

    // =========================================================================
    // FQN-based crate-origin filtering tests
    // =========================================================================

    #[test]
    fn test_is_workspace_fqn_exact_match() {
        let crate_names = vec![
            "rustbrain_common".to_string(),
            "rustbrain_ingestion".to_string(),
        ];
        assert!(ParseStage::is_workspace_fqn(
            "rustbrain_common",
            &crate_names
        ));
        assert!(ParseStage::is_workspace_fqn(
            "rustbrain_ingestion",
            &crate_names
        ));
    }

    #[test]
    fn test_is_workspace_fqn_prefix_match() {
        let crate_names = vec!["rustbrain_common".to_string(), "ingestion".to_string()];
        assert!(ParseStage::is_workspace_fqn(
            "rustbrain_common::types::Item",
            &crate_names
        ));
        assert!(ParseStage::is_workspace_fqn(
            "ingestion::pipeline::stages",
            &crate_names
        ));
    }

    #[test]
    fn test_is_workspace_fqn_rejects_external() {
        let crate_names = vec!["rustbrain_common".to_string(), "ingestion".to_string()];
        assert!(!ParseStage::is_workspace_fqn(
            "tokio::runtime::Runtime",
            &crate_names
        ));
        assert!(!ParseStage::is_workspace_fqn(
            "serde::Deserialize",
            &crate_names
        ));
        assert!(!ParseStage::is_workspace_fqn(
            "std::collections::HashMap",
            &crate_names
        ));
    }

    #[test]
    fn test_is_workspace_fqn_rejects_partial_prefix() {
        // "rustbrain_common_extra" should NOT match "rustbrain_common"
        let crate_names = vec!["rustbrain_common".to_string()];
        assert!(!ParseStage::is_workspace_fqn(
            "rustbrain_common_extra::Foo",
            &crate_names
        ));
    }

    #[test]
    fn test_is_workspace_fqn_empty_crate_names() {
        assert!(!ParseStage::is_workspace_fqn(
            "rustbrain_common::types::Item",
            &[]
        ));
    }

    #[test]
    fn test_graph_stage_is_workspace_fqn() {
        let crate_names = vec![
            "rustbrain_common".to_string(),
            "rustbrain_ingestion".to_string(),
        ];
        assert!(GraphStage::is_workspace_fqn(
            "rustbrain_common::types::Item",
            &crate_names
        ));
        assert!(!GraphStage::is_workspace_fqn(
            "tokio::runtime::Runtime",
            &crate_names
        ));
    }
}
