//! rust-brain Ingestion Service
//!
//! Macro expansion and source ingestion for Rust code intelligence.
//!
//! # Usage
//!
//! ```bash
//! # Run full pipeline on a crate
//! rustbrain-ingestion -c /path/to/crate -d postgres://localhost/rustbrain
//!
//! # Run specific stages only
//! rustbrain-ingestion --crate-path /path/to/crate --stages expand,parse \
//!     --database-url postgres://localhost/rustbrain
//!
//! # Dry run (no database writes)
//! rustbrain-ingestion --crate-path /path/to/crate --dry-run
//!
//! # With Neo4j and embedding service
//! rustbrain-ingestion -c /path/to/crate \
//!     --neo4j-url bolt://localhost:7687 \
//!     --embedding-url http://localhost:11434
//!
//! # Stop on first error, limit concurrency, enable debug output
//! rustbrain-ingestion -c /path/to/crate --fail-fast --max-concurrency 2 --verbose
//! ```
//!
//! # Flags
//!
//! | Flag | Short | Default | Env var |
//! |------|-------|---------|---------|
//! | `--crate-path` | `-c` | `.` | — |
//! | `--database-url` | `-d` | — | `DATABASE_URL` (required) |
//! | `--neo4j-url` | — | — | `NEO4J_URL` |
//! | `--embedding-url` | — | — | `EMBEDDING_URL` |
//! | `--workspace-id` | — | — | `INGESTION_WORKSPACE_ID` |
//! | `--workspace-label` | — | — | `INGESTION_WORKSPACE_LABEL` |
//! | `--stages` | `-s` | all | — |
//! | `--dry-run` | — | false | — |
//! | `--fail-fast` | — | false | — |
//! | `--max-concurrency` | — | 4 | — |
//! | `--verbose` | `-v` | false | — |

pub mod derive_detector;
pub mod embedding;
pub mod graph;
pub mod monitoring;
pub mod parsers;
pub mod pipeline;
pub mod typecheck;

use anyhow::{Context, Result};
use clap::Parser;
use pipeline::{read_crate_name_from_toml, PipelineConfig, PipelineRunner, StageStatus};
use std::path::PathBuf;
use tracing::{error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

// =============================================================================
// CLI Arguments
// =============================================================================

/// rust-brain ingestion service
#[derive(Parser, Debug)]
#[command(name = "rustbrain-ingestion")]
#[command(author = "rust-brain team")]
#[command(version = "0.1.0")]
#[command(about = "Macro expansion and source ingestion for Rust code intelligence")]
struct Args {
    /// Path to the crate or workspace to process
    #[arg(short, long, default_value = ".")]
    crate_path: PathBuf,

    /// Database connection URL
    #[arg(short, long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Neo4j connection URL (optional)
    #[arg(long, env = "NEO4J_URL")]
    neo4j_url: Option<String>,

    /// Embedding service URL (optional)
    #[arg(long, env = "EMBEDDING_URL")]
    embedding_url: Option<String>,

    /// Workspace ID (optional)
    #[arg(long, env = "INGESTION_WORKSPACE_ID")]
    workspace_id: Option<String>,

    /// Workspace label (optional)
    #[arg(long, env = "INGESTION_WORKSPACE_LABEL")]
    workspace_label: Option<String>,

    /// Comma-separated list of stages to run (default: all)
    /// Available stages: expand, parse, typecheck, extract, graph, embed
    #[arg(short, long, value_delimiter = ',')]
    stages: Option<Vec<String>>,

    /// Skip all stages before this one. The named stage and all subsequent stages run.
    /// Available values: expand, parse, typecheck, extract, graph, embed
    #[arg(long, value_parser = ["expand", "parse", "typecheck", "extract", "graph", "embed"])]
    from_stage: Option<String>,

    /// Dry run mode - don't write to databases
    #[arg(long, default_value = "false")]
    dry_run: bool,

    /// Stop on first error (default: continue on non-fatal errors)
    #[arg(long, default_value = "false")]
    fail_fast: bool,

    /// Maximum concurrent operations
    #[arg(long, default_value = "4")]
    max_concurrency: usize,

    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
}

// =============================================================================
// Main Entry Point
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .with_thread_ids(false)
        .pretty()
        .init();

    info!("rust-brain ingestion service starting...");
    info!("Crate path: {:?}", args.crate_path);

    let crate_name = read_crate_name_from_toml(&args.crate_path);
    if let Some(ref name) = crate_name {
        info!("Crate name: {}", name);
    }

    // Build pipeline configuration
    let config = PipelineConfig {
        crate_path: args.crate_path.clone(),
        crate_name,
        database_url: args.database_url
            .or_else(|| std::env::var("DATABASE_URL").ok())
            .expect("DATABASE_URL must be provided via --database-url flag or DATABASE_URL environment variable"),
        neo4j_url: args.neo4j_url,
        embedding_url: args.embedding_url,
        stages: args.stages,
        from_stage: args.from_stage,
        dry_run: args.dry_run,
        continue_on_error: !args.fail_fast,
        max_concurrency: args.max_concurrency,
        workspace_id: args.workspace_id.and_then(|s| Uuid::parse_str(&s).ok()),
        workspace_label: args.workspace_label,
        workspace_crate_names: Vec::new(),
    };

    config.validate().context("Invalid configuration")?;

    if args.dry_run {
        info!("Running in DRY RUN mode - no database writes");
    }

    if let Some(ref stages) = config.stages {
        info!("Running stages: {}", stages.join(", "));
    } else {
        info!("Running all stages");
    }

    // Create pipeline runner
    let mut runner = PipelineRunner::new(config).context("Failed to create pipeline runner")?;

    // Connect to database (if not dry run)
    if !args.dry_run {
        runner
            .connect()
            .await
            .context("Failed to connect to database")?;
    }

    // Run the pipeline
    let result = runner.run().await.context("Pipeline execution failed")?;

    // Print summary
    println!("\n{}", "=".repeat(60));
    println!("Pipeline Run: {}", result.id);
    println!("Status: {}", result.status);
    println!("Duration: {}ms", result.duration_ms);
    println!("{}", "=".repeat(60));

    // Print stage results
    println!("\nStage Results:");
    for stage in &result.stages {
        let status_icon = match stage.status {
            StageStatus::Success => "✅",
            StageStatus::Partial => "⚠️",
            StageStatus::Failed => "❌",
            StageStatus::Skipped => "⏭️",
        };
        println!(
            "  {} {}: {} processed, {} failed ({}ms)",
            status_icon, stage.name, stage.items_processed, stage.items_failed, stage.duration_ms
        );
        if let Some(ref error) = stage.error {
            println!("      Error: {}", error);
        }
    }

    // Print counts
    println!("\nCounts:");
    println!("  Files expanded: {}", result.counts.files_expanded);
    println!("  Files parsed: {}", result.counts.files_parsed);
    println!("  Items parsed: {}", result.counts.items_parsed);
    println!("  Items typechecked: {}", result.counts.items_typechecked);
    println!("  Items extracted: {}", result.counts.items_extracted);
    println!("  Graph nodes: {}", result.counts.graph_nodes);
    println!("  Graph edges: {}", result.counts.graph_edges);
    println!("  Embeddings: {}", result.counts.embeddings_created);

    // Print errors if any
    if !result.errors.is_empty() {
        println!("\nErrors ({}):", result.errors.len());
        for error in &result.errors {
            let fatal_marker = if error.is_fatal { " [FATAL]" } else { "" };
            println!(
                "  [{}]{} {}: {}",
                error.stage,
                fatal_marker,
                error.message,
                error
                    .context
                    .as_ref()
                    .map(|c| format!("({})", c))
                    .unwrap_or_default()
            );
        }
    }

    // Exit with appropriate code
    match result.status {
        pipeline::PipelineStatus::Completed => {
            info!("Ingestion completed successfully");
            std::process::exit(0);
        }
        pipeline::PipelineStatus::Partial => {
            warn!("Ingestion completed with partial success");
            std::process::exit(0);
        }
        pipeline::PipelineStatus::Failed => {
            error!("Ingestion failed");
            std::process::exit(1);
        }
        pipeline::PipelineStatus::Running => unreachable!(),
    }
}
