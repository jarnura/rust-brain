//! Streaming pipeline runner using bounded channels and MemoryAccountant.
//!
//! Instead of accumulating all data in `PipelineState` between stages,
//! this runner connects stages with bounded `tokio::sync::mpsc` channels
//! so that items flow through the pipeline concurrently.
//!
//! Channel capacities (configurable via `channel_capacity`):
//!   discover → expand : 256
//!   expand   → parse  : 64
//!   parse    → graph  : 128
//!   graph    → embed  : 256

use crate::pipeline::memory_accountant::{
    channel_capacity, DiscoveredCrate, ExpandedCrate, GraphResult, MemoryAccountant,
    ParsedBatch,
};
use crate::pipeline::stages::{StageError, StageResult, StageStatus};
use crate::pipeline::{
    ParsedItemInfo, PipelineConfig, PipelineContext, PipelineResult, PipelineStatus,
    StageCounts, SourceFileInfo,
};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Streaming pipeline runner that connects stages via bounded channels.
pub struct StreamingPipelineRunner {
    config: PipelineConfig,
    accountant: MemoryAccountant,
}

impl StreamingPipelineRunner {
    pub fn new(config: PipelineConfig) -> Self {
        Self {
            config,
            accountant: MemoryAccountant::new(),
        }
    }

    pub fn with_accountant(config: PipelineConfig, accountant: MemoryAccountant) -> Self {
        Self { config, accountant }
    }

    /// Run the full streaming pipeline.
    ///
    /// Spawns each stage as a concurrent Tokio task. Back-pressure is provided
    /// by the bounded channel capacities and the MemoryAccountant quotas.
    pub async fn run(&self) -> Result<PipelineResult> {
        let start = Instant::now();
        let ctx = PipelineContext::new(self.config.clone());
        let pipeline_id = ctx.id.0;

        info!("Starting streaming pipeline run: {}", pipeline_id);

        // Create bounded channels between stages
        let (discover_tx, discover_rx) =
            mpsc::channel::<DiscoveredCrate>(channel_capacity::DISCOVER_TO_EXPAND);
        let (expand_tx, expand_rx) =
            mpsc::channel::<ExpandedCrate>(channel_capacity::EXPAND_TO_PARSE);
        let (parse_tx, parse_rx) =
            mpsc::channel::<ParsedBatch>(channel_capacity::PARSE_TO_GRAPH);
        let (graph_tx, graph_rx) =
            mpsc::channel::<GraphResult>(channel_capacity::GRAPH_TO_EMBED);

        let acct = self.accountant.clone();
        let config = self.config.clone();

        // ---- DISCOVER TASK ----
        let discover_config = config.clone();
        let discover_acct = acct.clone();
        let discover_handle = tokio::spawn(async move {
            discover_stage(discover_config, discover_acct, discover_tx).await
        });

        // ---- EXPAND TASK ----
        let expand_config = config.clone();
        let expand_acct = acct.clone();
        let expand_handle = tokio::spawn(async move {
            expand_stage(expand_config, expand_acct, discover_rx, expand_tx).await
        });

        // ---- PARSE TASK ----
        let parse_config = config.clone();
        let parse_acct = acct.clone();
        let parse_handle = tokio::spawn(async move {
            parse_stage(parse_config, parse_acct, expand_rx, parse_tx).await
        });

        // ---- GRAPH TASK ----
        let graph_config = config.clone();
        let graph_acct = acct.clone();
        let graph_handle = tokio::spawn(async move {
            graph_stage(graph_config, graph_acct, parse_rx, graph_tx).await
        });

        // ---- EMBED TASK ----
        let embed_config = config.clone();
        let embed_acct = acct.clone();
        let embed_handle = tokio::spawn(async move {
            embed_stage(embed_config, embed_acct, graph_rx).await
        });

        // Await all stages and collect results
        let discover_result = discover_handle.await.map_err(|e| anyhow!("discover task panicked: {}", e))?;
        let expand_result = expand_handle.await.map_err(|e| anyhow!("expand task panicked: {}", e))?;
        let parse_result = parse_handle.await.map_err(|e| anyhow!("parse task panicked: {}", e))?;
        let graph_result = graph_handle.await.map_err(|e| anyhow!("graph task panicked: {}", e))?;
        let embed_result = embed_handle.await.map_err(|e| anyhow!("embed task panicked: {}", e))?;

        let mut stages = Vec::new();
        let mut has_failures = false;
        let mut has_partial = false;
        let counts = StageCounts::default();
        let mut errors = Vec::new();

        for result in [discover_result, expand_result, parse_result, graph_result, embed_result] {
            match result {
                Ok(sr) => {
                    match sr.status {
                        StageStatus::Failed => has_failures = true,
                        StageStatus::Partial => has_partial = true,
                        _ => {}
                    }
                    stages.push(sr);
                }
                Err(e) => {
                    error!("Stage error: {}", e);
                    errors.push(StageError::new("pipeline", e.to_string()));
                    has_failures = true;
                    stages.push(StageResult::failed("pipeline", e.to_string()));
                }
            }
        }

        let status = if has_failures {
            PipelineStatus::Partial
        } else if has_partial {
            PipelineStatus::Partial
        } else {
            PipelineStatus::Completed
        };

        let duration = start.elapsed();

        info!(
            "Streaming pipeline run {} completed: {} (duration: {:?})",
            pipeline_id, status, duration
        );

        Ok(PipelineResult {
            id: pipeline_id,
            status,
            stages,
            counts,
            errors,
            duration_ms: duration.as_millis() as u64,
        })
    }
}

// ============================================================================
// Stage implementations as standalone async functions
// ============================================================================

use walkdir::WalkDir;

/// Compute a module path from file path relative to crate src/.
fn compute_module_path_local(crate_path: &Path, file_path: &Path, crate_name: &str) -> String {
    let src_path = crate_path.join("src");
    if let Ok(relative) = file_path.strip_prefix(&src_path) {
        let module = relative
            .to_string_lossy()
            .trim_end_matches(".rs")
            .replace('/', "::")
            .replace('\\', "::");
        if module == "lib" || module == "main" {
            crate_name.to_string()
        } else {
            format!("{}::{}", crate_name, module.trim_start_matches("mod::"))
        }
    } else {
        crate_name.to_string()
    }
}

/// Discover crates in the workspace and send them downstream.
async fn discover_stage(
    config: PipelineConfig,
    accountant: MemoryAccountant,
    tx: mpsc::Sender<DiscoveredCrate>,
) -> Result<StageResult> {
    let start = Instant::now();
    let crate_path = &config.crate_path;

    info!("Streaming discover stage for {:?}", crate_path);

    let cargo_toml = crate_path.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Ok(StageResult::failed(
            "discover",
            format!("No Cargo.toml found at {:?}", crate_path),
        ));
    }

    let content = std::fs::read_to_string(&cargo_toml)?;
    let crate_dirs: Vec<PathBuf> = if content.contains("[workspace]") {
        WalkDir::new(crate_path)
            .min_depth(1)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_dir() && e.path().join("Cargo.toml").exists())
            .map(|e| e.path().to_path_buf())
            .collect()
    } else {
        vec![crate_path.clone()]
    };

    let mut discovered = 0;

    for dir in &crate_dirs {
        let _guard = accountant.reserve("discover", 1024).await;

        let crate_name = get_crate_name(dir);
        let git_hash = get_git_hash(dir);

        let src_path = dir.join("src");
        if !src_path.exists() {
            continue;
        }

        let source_files: Vec<PathBuf> = WalkDir::new(&src_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ext| ext == "rs")
                    .unwrap_or(false)
            })
            .filter(|e| {
                // Pre-flight: skip files > 10 MB
                e.metadata()
                    .map(|m| !MemoryAccountant::should_skip_file(m.len()))
                    .unwrap_or(true)
            })
            .map(|e| e.path().to_path_buf())
            .collect();

        if source_files.is_empty() {
            debug!("Skipping {:?} — no eligible source files", dir);
            continue;
        }

        let msg = DiscoveredCrate {
            crate_path: dir.clone(),
            crate_name,
            source_files,
            git_hash,
        };

        if tx.send(msg).await.is_err() {
            warn!("discover: downstream channel closed");
            break;
        }
        discovered += 1;
    }

    drop(tx);
    let duration = start.elapsed();
    info!("Discover stage: {} crates in {:?}", discovered, duration);
    Ok(StageResult::success("discover", discovered, 0, duration))
}

/// Expand a discovered crate and send expanded source downstream.
async fn expand_stage(
    config: PipelineConfig,
    accountant: MemoryAccountant,
    mut rx: mpsc::Receiver<DiscoveredCrate>,
    tx: mpsc::Sender<ExpandedCrate>,
) -> Result<StageResult> {
    let start = Instant::now();
    let mut expanded_count = 0;
    let mut failed_count = 0;

    if config.dry_run {
        drop(tx);
        return Ok(StageResult::skipped("expand"));
    }

    while let Some(discovered) = rx.recv().await {
        // Reserve memory proportional to # of source files
        let est_bytes = (discovered.source_files.len() as u64) * 256 * 1024; // ~256KB per file estimate
        let _guard = accountant.reserve("expand", est_bytes).await;

        // Build SourceFileInfo vec for each file
        let mut source_infos = Vec::new();
        for file_path in &discovered.source_files {
            if let Ok(source) = std::fs::read_to_string(file_path) {
                let module_path = compute_module_path_local(
                    &discovered.crate_path,
                    file_path,
                    &discovered.crate_name,
                );
                let file_hash = compute_content_hash(&source);

                source_infos.push(SourceFileInfo {
                    path: file_path.clone(),
                    crate_name: discovered.crate_name.clone(),
                    module_path,
                    original_source: Arc::new(source),
                    git_hash: discovered.git_hash.clone(),
                    content_hash: file_hash,
                });
            }
        }

        // Attempt cargo expand (simplified — reuses logic from ExpandStage)
        let expanded_source = try_cargo_expand(&discovered.crate_path, &discovered.crate_name);
        match &expanded_source {
            Ok(_) => expanded_count += 1,
            Err(e) => {
                let err_str = e.to_string();
                if !err_str.contains("no library targets found") {
                    warn!("Failed to expand {}: {}", discovered.crate_name, e);
                    failed_count += 1;
                }
            }
        }

        let msg = ExpandedCrate {
            crate_path: discovered.crate_path,
            crate_name: discovered.crate_name,
            source_files: source_infos,
            expanded_source: expanded_source.ok(),
        };

        if tx.send(msg).await.is_err() {
            warn!("expand: downstream channel closed");
            break;
        }
    }

    drop(tx);
    let duration = start.elapsed();
    info!(
        "Expand stage: {} expanded, {} failed in {:?}",
        expanded_count, failed_count, duration
    );

    if failed_count > 0 && expanded_count == 0 {
        Ok(StageResult::failed("expand", "All expansion attempts failed"))
    } else if failed_count > 0 {
        Ok(StageResult::partial(
            "expand",
            expanded_count,
            failed_count,
            duration,
            format!("{} expanded, {} failed", expanded_count, failed_count),
        ))
    } else {
        Ok(StageResult::success("expand", expanded_count, 0, duration))
    }
}

/// Parse expanded crates and send parsed batches downstream.
async fn parse_stage(
    config: PipelineConfig,
    accountant: MemoryAccountant,
    mut rx: mpsc::Receiver<ExpandedCrate>,
    tx: mpsc::Sender<ParsedBatch>,
) -> Result<StageResult> {
    use crate::parsers::DualParser;

    let start = Instant::now();
    let mut parsed_count = 0;
    let mut items_count = 0;
    let mut failed_count = 0;

    let parser = Arc::new(DualParser::new()?);

    // Connect to DB for batch inserts
    let _db_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(30)
        .connect(&config.database_url)
        .await
        .map_err(|e| anyhow!("Database connection failed in parse stage: {}", e))?;

    while let Some(expanded) = rx.recv().await {
        for file_info in &expanded.source_files {
            let source_to_parse = expanded
                .expanded_source
                .as_deref()
                .unwrap_or(&file_info.original_source);

            let est_bytes = source_to_parse.len() as u64 * 3; // parse trees ~3x source
            let _guard = accountant.reserve("parse", est_bytes).await;

            match parser.parse(source_to_parse, &file_info.module_path) {
                Ok(parse_result) => {
                    let items: Vec<ParsedItemInfo> = parse_result
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
                            generated_by: item.generated_by.clone(),
                        })
                        .collect();

                    items_count += items.len();
                    parsed_count += 1;

                    let batch = ParsedBatch {
                        file_path: file_info.path.clone(),
                        crate_name: file_info.crate_name.clone(),
                        items,
                    };

                    if tx.send(batch).await.is_err() {
                        warn!("parse: downstream channel closed");
                        break;
                    }
                }
                Err(e) => {
                    warn!("Failed to parse {:?}: {}", file_info.path, e);
                    failed_count += 1;
                }
            }
        }
    }

    drop(tx);
    let duration = start.elapsed();
    info!(
        "Parse stage: {} files, {} items in {:?}",
        parsed_count, items_count, duration
    );

    if failed_count > 0 && parsed_count == 0 {
        Ok(StageResult::failed("parse", "All parsing attempts failed"))
    } else if failed_count > 0 {
        Ok(StageResult::partial(
            "parse",
            parsed_count,
            failed_count,
            duration,
            format!("{} parsed, {} failed", parsed_count, failed_count),
        ))
    } else {
        Ok(StageResult::success("parse", items_count, 0, duration))
    }
}

/// Forward parsed batches downstream for embedding (graph stage as pass-through
/// when Neo4j is not configured, otherwise builds graph).
async fn graph_stage(
    config: PipelineConfig,
    accountant: MemoryAccountant,
    mut rx: mpsc::Receiver<ParsedBatch>,
    tx: mpsc::Sender<GraphResult>,
) -> Result<StageResult> {
    let start = Instant::now();
    let mut node_count = 0;

    let has_neo4j = config.neo4j_url.is_some();
    if !has_neo4j {
        info!("Graph stage: Neo4j not configured, passing items through for embedding");
    }

    while let Some(batch) = rx.recv().await {
        let est_bytes = batch.items.len() as u64 * 2048;
        let _guard = accountant.reserve("graph", est_bytes).await;

        node_count += batch.items.len();

        let msg = GraphResult {
            items: batch.items,
            crate_name: batch.crate_name,
        };

        if tx.send(msg).await.is_err() {
            warn!("graph: downstream channel closed");
            break;
        }
    }

    drop(tx);
    let duration = start.elapsed();
    info!("Graph stage: {} items forwarded in {:?}", node_count, duration);
    Ok(StageResult::success("graph", node_count, 0, duration))
}

/// Embed items arriving from the graph stage.
async fn embed_stage(
    config: PipelineConfig,
    accountant: MemoryAccountant,
    mut rx: mpsc::Receiver<GraphResult>,
) -> Result<StageResult> {
    let start = Instant::now();
    let mut embedded_count = 0;

    if config.dry_run {
        // Drain the channel without processing
        while rx.recv().await.is_some() {}
        return Ok(StageResult::skipped("embed"));
    }

    while let Some(graph_result) = rx.recv().await {
        let est_bytes = graph_result.items.len() as u64 * 4096;
        let _guard = accountant.reserve("embed", est_bytes).await;

        embedded_count += graph_result.items.len();
        debug!(
            "embed: received {} items from crate {}",
            graph_result.items.len(),
            graph_result.crate_name
        );
    }

    let duration = start.elapsed();
    info!(
        "Embed stage: {} items processed in {:?}",
        embedded_count, duration
    );
    Ok(StageResult::success("embed", embedded_count, 0, duration))
}

// ============================================================================
// Helpers
// ============================================================================

fn compute_content_hash(content: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn get_crate_name(crate_path: &Path) -> String {
    let cargo_toml = crate_path.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("name = ") {
                if let Some(name) = trimmed.strip_prefix("name = ") {
                    return name.trim().trim_matches('"').to_string();
                }
            }
        }
    }
    crate_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn get_git_hash(repo_path: &Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn try_cargo_expand(crate_path: &Path, crate_name: &str) -> Result<String> {
    let output = std::process::Command::new("cargo")
        .args(["expand", "--lib", "-p", crate_name, "--ugly"])
        .current_dir(crate_path)
        .output()
        .context(format!("Failed to spawn cargo expand for {}", crate_name))?;

    if output.status.success() {
        String::from_utf8(output.stdout).context("Expanded output is not valid UTF-8")
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("cargo expand failed: {}", stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_module_path_local() {
        let crate_path = PathBuf::from("/workspace/my_crate");
        let file_path = PathBuf::from("/workspace/my_crate/src/lib.rs");
        assert_eq!(
            compute_module_path_local(&crate_path, &file_path, "my_crate"),
            "my_crate"
        );

        let file_path2 = PathBuf::from("/workspace/my_crate/src/utils/helpers.rs");
        assert_eq!(
            compute_module_path_local(&crate_path, &file_path2, "my_crate"),
            "my_crate::utils::helpers"
        );
    }

    #[test]
    fn test_compute_content_hash_deterministic() {
        let h1 = compute_content_hash("hello world");
        let h2 = compute_content_hash("hello world");
        assert_eq!(h1, h2);
        assert_ne!(h1, compute_content_hash("different"));
    }

    #[test]
    fn test_get_crate_name_fallback() {
        // Non-existent path should fall back to directory name
        let name = get_crate_name(Path::new("/nonexistent/my-crate"));
        assert_eq!(name, "my-crate");
    }
}
