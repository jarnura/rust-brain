//! Pipeline orchestration for rust-brain ingestion
//!
//! This module implements a staged pipeline for processing Rust source code:
//! 1. **Expand** - Macro expansion via cargo expand
//! 2. **Parse** - Dual parsing (tree-sitter + syn)
//! 3. **Typecheck** - Type resolution and inference
//! 4. **Extract** - Extract items to Postgres
//! 5. **Graph** - Build Neo4j relationship graph
//! 6. **Embed** - Create vector embeddings

pub mod stages;
pub mod runner;
pub mod memory_accountant;
pub mod streaming_runner;
pub mod circuit_breaker;
pub mod resilience;

pub use stages::{PipelineStage, StageResult, StageError, StageStatus};
pub use runner::PipelineRunner;
pub use memory_accountant::{MemoryAccountant, MemoryGuard, channel_capacity};
pub use streaming_runner::StreamingPipelineRunner;
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError, CircuitState};
pub use resilience::{
    MemoryPressure, MemoryWatchdog, SpillStore, DegradationTier,
    CheckpointManager, Checkpoint, ResilienceCoordinator,
};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
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
    
    /// Database connection URL
    pub database_url: String,
    
    /// Neo4j connection URL (optional)
    pub neo4j_url: Option<String>,
    
    /// Embedding service URL (optional)
    pub embedding_url: Option<String>,
    
    /// Which stages to run (None = all stages)
    pub stages: Option<Vec<String>>,
    
    /// Dry run mode - don't write to databases
    pub dry_run: bool,
    
    /// Continue on non-fatal errors
    pub continue_on_error: bool,
    
    /// Maximum concurrent operations
    pub max_concurrency: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            crate_path: PathBuf::from("."),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL environment variable must be set"),
            neo4j_url: None,
            embedding_url: None,
            stages: None,
            dry_run: false,
            continue_on_error: true,
            max_concurrency: 4,
        }
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

    /// Expanded source code by file path (Arc for cheap cloning across stages)
    pub expanded_sources: Arc<HashMap<PathBuf, String>>,

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
pub const STAGE_NAMES: &[&str] = &[
    "expand",
    "parse", 
    "typecheck",
    "extract",
    "graph",
    "embed",
];

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
    
    #[test]
    fn test_pipeline_id_default() {
        let id = PipelineId::default();
        assert!(!id.0.is_nil());
    }
    
    #[test]
    fn test_should_run_stage() {
        let mut config = PipelineConfig::default();

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
        let config = PipelineConfig::default();
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
        let config = PipelineConfig::default();
        let ctx = PipelineContext::new(config);
        assert!(!ctx.id.0.is_nil());
    }

    #[test]
    fn test_pipeline_context_with_id() {
        let id = Uuid::new_v4();
        let config = PipelineConfig::default();
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
        let config = PipelineConfig::default();
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
            ..Default::default()
        };
        // Empty list means no stages should run
        assert!(!should_run_stage(&config, "expand"));
    }
}
