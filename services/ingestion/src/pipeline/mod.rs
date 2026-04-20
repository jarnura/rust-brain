//! Pipeline orchestration for rust-brain ingestion
//!
//! This module implements a staged pipeline for processing Rust source code:
//! 1. **Expand** - Macro expansion via cargo expand
//! 2. **Parse** - Dual parsing (tree-sitter + syn)
//! 3. **Typecheck** - Type resolution and inference
//! 4. **Extract** - Extract items to Postgres
//! 5. **Graph** - Build Neo4j relationship graph
//! 6. **Embed** - Create vector embeddings

pub mod circuit_breaker;
pub mod memory_accountant;
pub mod resilience;
pub mod runner;
pub mod stages;
pub mod streaming_runner;

pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError, CircuitState,
};
pub use memory_accountant::{channel_capacity, MemoryAccountant, MemoryGuard};
pub use resilience::{
    Checkpoint, CheckpointManager, DegradationTier, MemoryPressure, MemoryWatchdog,
    ResilienceCoordinator, SpillStore,
};
pub use runner::PipelineRunner;
pub use stages::{
    parse_item_type, parse_visibility, DataLifecycleManager, PipelineStage, StageError,
    StageResult, StageStatus, StaleCleanupReport,
};
pub use streaming_runner::StreamingPipelineRunner;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Unique identifier for a pipeline run
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PipelineId(pub Uuid);

impl Default for PipelineId {
    fn default() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for PipelineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for pipeline execution
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Path to the crate or workspace to process
    pub crate_path: PathBuf,

    /// Name of the crate being processed (extracted from Cargo.toml)
    pub crate_name: Option<String>,

    /// Database connection URL
    pub database_url: String,

    /// Neo4j connection URL (optional)
    pub neo4j_url: Option<String>,

    /// Embedding service URL (optional)
    pub embedding_url: Option<String>,

    /// Which stages to run (None = all stages)
    pub stages: Option<Vec<String>>,

    /// Start from this stage (skips all previous stages)
    pub from_stage: Option<String>,

    /// Dry run mode - don't write to databases
    pub dry_run: bool,

    /// Continue on non-fatal errors
    pub continue_on_error: bool,

    /// Maximum concurrent operations
    pub max_concurrency: usize,

    pub workspace_id: Option<Uuid>,

    /// Neo4j label in format `Workspace_<12hex>` matching Postgres `ws_<12hex>` schema
    pub workspace_label: Option<String>,

    /// Names of all crates in the workspace
    pub workspace_crate_names: Vec<String>,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            crate_path: PathBuf::from("."),
            crate_name: None,
            database_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable must be set"),
            neo4j_url: None,
            embedding_url: None,
            stages: None,
            from_stage: None,
            dry_run: false,
            continue_on_error: true,
            max_concurrency: 4,
            workspace_id: None,
            workspace_label: None,
            workspace_crate_names: Vec::new(),
        }
    }
}

/// Validate workspace label format: must be "Workspace_" + 12 lowercase hex chars
pub fn validate_workspace_label(label: &str) -> bool {
    const PREFIX: &str = "Workspace_";
    const HEX_LEN: usize = 12;

    if !label.starts_with(PREFIX) {
        return false;
    }
    let hex_part = &label[PREFIX.len()..];
    if hex_part.len() != HEX_LEN {
        return false;
    }
    hex_part
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

impl PipelineConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if let Some(ref from_stage) = self.from_stage {
            if !STAGE_NAMES.contains(&from_stage.as_str()) {
                anyhow::bail!(
                    "Invalid from_stage '{}'. Valid values: {}",
                    from_stage,
                    STAGE_NAMES.join(", ")
                );
            }
        }

        if let Some(ref label) = self.workspace_label {
            if !validate_workspace_label(label) {
                anyhow::bail!(
                    "Invalid workspace_label '{}'. Expected format: 'Workspace_' followed by exactly 12 lowercase hex characters",
                    label
                );
            }
        }

        if self.workspace_id.is_some() && self.workspace_label.is_none() {
            tracing::warn!(
                "workspace_id is set but workspace_label is None. Set workspace_label to match the workspace_id for graph stage consistency"
            );
        }

        Ok(())
    }
}

/// Context shared across pipeline stages
#[derive(Debug)]
pub struct PipelineContext {
    /// Unique identifier for this pipeline run
    pub id: PipelineId,

    /// Configuration
    pub config: PipelineConfig,

    /// Shared state between stages
    pub state: Arc<RwLock<PipelineState>>,
}

impl PipelineContext {
    /// Create a new pipeline context
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            id: PipelineId::default(),
            config,
            state: Arc::new(RwLock::new(PipelineState::default())),
        }
    }

    /// Create with a specific ID (for resuming runs)
    pub fn with_id(id: Uuid, config: PipelineConfig) -> Self {
        Self {
            id: PipelineId(id),
            config,
            state: Arc::new(RwLock::new(PipelineState::default())),
        }
    }
}

/// Mutable state accumulated during pipeline execution
#[derive(Debug, Default)]
pub struct PipelineState {
    /// Source files discovered
    pub source_files: Vec<SourceFileInfo>,

    /// Cache file paths for expanded source code by source file path.
    /// Content is read on-demand from cache files to prevent OOM.
    pub expanded_sources: Arc<HashMap<PathBuf, PathBuf>>,

    /// Parsed items by file
    pub parsed_items: HashMap<PathBuf, Vec<ParsedItemInfo>>,

    /// Extracted item IDs by FQN
    pub extracted_items: HashMap<String, Uuid>,

    /// Graph node IDs by FQN
    pub graph_nodes: HashMap<String, String>,

    /// Errors encountered
    pub errors: Vec<StageError>,

    /// Counts for each stage
    pub counts: StageCounts,

    /// Cache of expand results keyed by content hash (for incremental runs)
    pub expand_cache: HashMap<String, String>,

    /// Cross-store references for consistency tracking
    pub store_references: HashMap<String, rustbrain_common::StoreReference>,

    /// Current FQNs extracted during this run (for stale detection)
    pub current_fqns: HashSet<String>,
}

/// Information about a source file
#[derive(Debug, Clone)]
pub struct SourceFileInfo {
    pub path: PathBuf,
    pub crate_name: String,
    pub module_path: String,
    /// Arc<String> to avoid cloning large source strings - just increment refcount
    pub original_source: Arc<String>,
    pub git_hash: Option<String>,
    /// SHA-256 content hash for change detection
    pub content_hash: String,
}

/// Simplified parsed item info for state tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedItemInfo {
    pub fqn: String,
    pub item_type: String,
    pub name: String,
    pub visibility: String,
    pub signature: String,
    pub generic_params: Vec<crate::parsers::GenericParam>,
    pub where_clauses: Vec<crate::parsers::WhereClause>,
    pub attributes: Vec<String>,
    pub doc_comment: String,
    pub start_line: usize,
    pub end_line: usize,
    pub body_source: String,
    /// Source of macro generation, e.g., "derive(Debug)"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
}

/// Counts tracked per stage
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StageCounts {
    pub files_expanded: usize,
    pub files_parsed: usize,
    pub items_parsed: usize,
    pub items_typechecked: usize,
    pub items_extracted: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub embeddings_created: usize,
}

impl StageCounts {
    pub fn total_processed(&self) -> usize {
        self.files_expanded + self.items_parsed + self.items_extracted
    }
}

/// Overall pipeline result
#[derive(Debug, Serialize, Deserialize)]
pub struct PipelineResult {
    pub id: Uuid,
    pub status: PipelineStatus,
    pub stages: Vec<StageResult>,
    pub counts: StageCounts,
    pub errors: Vec<StageError>,
    pub duration_ms: u64,
}

/// Pipeline execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStatus {
    Running,
    Completed,
    Partial,
    Failed,
}

impl std::fmt::Display for PipelineStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Partial => write!(f, "partial"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Stage names in execution order
pub const STAGE_NAMES: &[&str] = &["expand", "parse", "typecheck", "extract", "graph", "embed"];

pub fn read_crate_name_from_toml(crate_path: &std::path::Path) -> Option<String> {
    let cargo_toml = crate_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("name") {
            if let Some(name) = trimmed.split('=').nth(1) {
                return Some(name.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

pub fn discover_workspace_crate_names(workspace_path: &Path) -> anyhow::Result<Vec<String>> {
    let workspace_cargo_toml = workspace_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&workspace_cargo_toml)?;
    let doc: toml_edit::DocumentMut = content.parse()?;
    let mut crate_names = Vec::new();

    if let Some(toml_edit::Item::Table(workspace_table)) = doc.get("workspace") {
        if let Some(toml_edit::Item::Value(toml_edit::Value::Array(members))) =
            workspace_table.get("members")
        {
            for item in members.iter() {
                if let toml_edit::Value::String(member_path) = item {
                    let member_cargo_toml =
                        workspace_path.join(member_path.value()).join("Cargo.toml");
                    if let Ok(member_content) = std::fs::read_to_string(&member_cargo_toml) {
                        if let Ok(member_doc) = member_content.parse::<toml_edit::DocumentMut>() {
                            if let Some(toml_edit::Item::Value(toml_edit::Value::String(name))) =
                                member_doc.get("package").and_then(|p| p.get("name"))
                            {
                                crate_names.push(name.value().to_string());
                            } else if let Some(toml_edit::Item::Value(toml_edit::Value::String(
                                name,
                            ))) = member_doc.get("name")
                            {
                                crate_names.push(name.value().to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    if crate_names.is_empty() {
        if let Some(toml_edit::Item::Value(toml_edit::Value::String(name))) =
            doc.get("package").and_then(|p| p.get("name"))
        {
            crate_names.push(name.value().to_string());
        } else if let Some(toml_edit::Item::Value(toml_edit::Value::String(name))) = doc.get("name")
        {
            crate_names.push(name.value().to_string());
        }
    }

    Ok(crate_names)
}

/// Check if a stage should run based on config
pub fn should_run_stage(config: &PipelineConfig, stage_name: &str) -> bool {
    match &config.stages {
        Some(stages) => stages.iter().any(|s| s == stage_name),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a PipelineConfig for testing (no DATABASE_URL required)
    fn test_config() -> PipelineConfig {
        PipelineConfig {
            crate_path: PathBuf::from("."),
            crate_name: None,
            database_url: "postgresql://test:test@localhost:5432/test".to_string(),
            neo4j_url: None,
            embedding_url: None,
            stages: None,
            from_stage: None,
            dry_run: false,
            continue_on_error: true,
            max_concurrency: 4,
            workspace_id: None,
            workspace_label: None,
            workspace_crate_names: Vec::new(),
        }
    }

    #[test]
    fn test_pipeline_id_default() {
        let id = PipelineId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn test_should_run_stage() {
        let mut config = test_config();

        // No stages specified = run all
        assert!(should_run_stage(&config, "expand"));
        assert!(should_run_stage(&config, "parse"));

        // Specific stages = only those
        config.stages = Some(vec!["expand".to_string(), "parse".to_string()]);
        assert!(should_run_stage(&config, "expand"));
        assert!(should_run_stage(&config, "parse"));
        assert!(!should_run_stage(&config, "embed"));
    }

    #[test]
    fn test_pipeline_id_uniqueness() {
        let id1 = PipelineId::default();
        let id2 = PipelineId::default();
        assert_ne!(id1.0, id2.0);
    }

    #[test]
    fn test_pipeline_id_display() {
        let id = PipelineId(Uuid::nil());
        assert_eq!(id.to_string(), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn test_pipeline_config_default() {
        let config = test_config();
        assert_eq!(config.crate_path, PathBuf::from("."));
        assert!(config.neo4j_url.is_none());
        assert!(config.embedding_url.is_none());
        assert!(config.stages.is_none());
        assert!(!config.dry_run);
        assert!(config.continue_on_error);
        assert_eq!(config.max_concurrency, 4);
    }

    #[test]
    fn test_pipeline_context_creation() {
        let config = test_config();
        let ctx = PipelineContext::new(config);
        assert!(!ctx.id.0.is_nil());
    }

    #[test]
    fn test_pipeline_context_with_id() {
        let id = Uuid::new_v4();
        let config = test_config();
        let ctx = PipelineContext::with_id(id, config);
        assert_eq!(ctx.id.0, id);
    }

    #[test]
    fn test_stage_counts_default() {
        let counts = StageCounts::default();
        assert_eq!(counts.files_expanded, 0);
        assert_eq!(counts.files_parsed, 0);
        assert_eq!(counts.items_parsed, 0);
        assert_eq!(counts.total_processed(), 0);
    }

    #[test]
    fn test_stage_counts_total_processed() {
        let counts = StageCounts {
            files_expanded: 5,
            files_parsed: 3,
            items_parsed: 20,
            items_typechecked: 15,
            items_extracted: 10,
            graph_nodes: 8,
            graph_edges: 12,
            embeddings_created: 7,
        };
        assert_eq!(counts.total_processed(), 5 + 20 + 10);
    }

    #[test]
    fn test_pipeline_status_display() {
        assert_eq!(PipelineStatus::Running.to_string(), "running");
        assert_eq!(PipelineStatus::Completed.to_string(), "completed");
        assert_eq!(PipelineStatus::Partial.to_string(), "partial");
        assert_eq!(PipelineStatus::Failed.to_string(), "failed");
    }

    #[test]
    fn test_stage_names_order() {
        assert_eq!(STAGE_NAMES.len(), 6);
        assert_eq!(STAGE_NAMES[0], "expand");
        assert_eq!(STAGE_NAMES[1], "parse");
        assert_eq!(STAGE_NAMES[2], "typecheck");
        assert_eq!(STAGE_NAMES[3], "extract");
        assert_eq!(STAGE_NAMES[4], "graph");
        assert_eq!(STAGE_NAMES[5], "embed");
    }

    #[tokio::test]
    async fn test_pipeline_state_default() {
        let config = test_config();
        let ctx = PipelineContext::new(config);
        let state = ctx.state.read().await;
        assert!(state.source_files.is_empty());
        assert!(state.expanded_sources.is_empty());
        assert!(state.parsed_items.is_empty());
        assert!(state.errors.is_empty());
    }

    #[test]
    fn test_pipeline_status_serialization() {
        let json = serde_json::to_string(&PipelineStatus::Completed).unwrap();
        assert_eq!(json, "\"completed\"");
        let deserialized: PipelineStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, PipelineStatus::Completed);
    }

    #[test]
    fn test_should_run_stage_empty_list() {
        let config = PipelineConfig {
            stages: Some(vec![]),
            ..test_config()
        };
        // Empty list means no stages should run
        assert!(!should_run_stage(&config, "expand"));
    }

    #[test]
    fn test_validate_workspace_label_valid() {
        assert!(validate_workspace_label("Workspace_a1b2c3d4e5f6"));
        assert!(validate_workspace_label("Workspace_abcdef123456"));
        assert!(validate_workspace_label("Workspace_000000000000"));
        assert!(validate_workspace_label("Workspace_ffffffffffff"));
    }

    #[test]
    fn test_validate_workspace_label_invalid_format() {
        // Lowercase 'w' in Workspace
        assert!(!validate_workspace_label("workspace_a1b2c3d4e5f6"));
        // Uppercase hex characters
        assert!(!validate_workspace_label("Workspace_A1B2C3D4E5F6"));
        // Too short (less than 12 hex chars)
        assert!(!validate_workspace_label("Workspace_short"));
        // Too long (more than 12 hex chars)
        assert!(!validate_workspace_label("Workspace_a1b2c3d4e5f6extra"));
        // No underscore separator
        assert!(!validate_workspace_label("Workspacea1b2c3d4e5f6"));
        // Wrong prefix entirely
        assert!(!validate_workspace_label("Project_a1b2c3d4e5f6"));
    }

    #[test]
    fn test_validate_config_with_valid_workspace() {
        let mut config = test_config();
        config.workspace_id = Some(Uuid::new_v4());
        config.workspace_label = Some("Workspace_a1b2c3d4e5f6".to_string());

        // Should validate successfully
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_config_invalid_workspace_label() {
        let mut config = test_config();
        config.workspace_id = Some(Uuid::new_v4());
        config.workspace_label = Some("invalid_label".to_string());

        // Should fail validation
        let result = config.validate();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid workspace_label"));
    }

    #[test]
    fn test_validate_workspace_label_none_ok() {
        // Config with neither workspace_id nor workspace_label should validate fine
        let config = test_config();
        assert!(config.workspace_id.is_none());
        assert!(config.workspace_label.is_none());

        // Should validate successfully (graph stage will just skip)
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_discover_workspace_crate_names_nonexistent_path() {
        let result = discover_workspace_crate_names(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn test_discover_workspace_crate_names_single_crate() {
        let tmpdir = tempfile::tempdir().unwrap();
        let cargo_toml = tmpdir.path().join("Cargo.toml");
        std::fs::write(
            &cargo_toml,
            r#"[package]
name = "my_crate"
version = "0.1.0"
"#,
        )
        .unwrap();

        let names = discover_workspace_crate_names(tmpdir.path()).unwrap();
        assert_eq!(names, vec!["my_crate"]);
    }

    #[test]
    fn test_discover_workspace_crate_names_workspace() {
        let tmpdir = tempfile::tempdir().unwrap();

        // Root Cargo.toml with workspace
        let root_cargo = tmpdir.path().join("Cargo.toml");
        std::fs::write(
            &root_cargo,
            r#"[workspace]
members = ["crates/common", "crates/app"]
"#,
        )
        .unwrap();

        // Member crate 1
        let common_dir = tmpdir.path().join("crates/common");
        std::fs::create_dir_all(&common_dir).unwrap();
        std::fs::write(
            common_dir.join("Cargo.toml"),
            r#"[package]
name = "my_common"
version = "0.1.0"
"#,
        )
        .unwrap();

        // Member crate 2
        let app_dir = tmpdir.path().join("crates/app");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            r#"[package]
name = "my_app"
version = "0.1.0"
"#,
        )
        .unwrap();

        let names = discover_workspace_crate_names(tmpdir.path()).unwrap();
        assert_eq!(names, vec!["my_common", "my_app"]);
    }
}
