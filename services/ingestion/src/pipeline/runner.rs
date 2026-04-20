//! Pipeline runner for sequential stage execution
//!
//! Executes pipeline stages in order, handling errors gracefully
//! and recording progress to the database.

use crate::monitoring::audit::AuditEmitter;
use crate::monitoring::{Monitor, MonitorConfig};
use crate::pipeline::resilience::{CheckpointManager, DegradationTier, ResilienceCoordinator};
use crate::pipeline::stages::{
    EmbedStage, ExpandStage, ExtractStage, GraphStage, ParseStage, StageError, StageResult,
    StageStatus, TypecheckStage,
};
use crate::pipeline::{
    PipelineConfig, PipelineContext, PipelineResult, PipelineStage, PipelineStatus, StageCounts,
    STAGE_NAMES,
};
use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Pipeline runner that orchestrates stage execution
pub struct PipelineRunner {
    /// Pipeline context
    ctx: PipelineContext,

    /// Database pool for recording runs
    pool: Option<PgPool>,

    /// Stage implementations
    stages: Vec<Box<dyn PipelineStage>>,

    /// Resilience coordinator (memory watchdog, circuit breakers, spill, checkpoints)
    resilience: Option<Arc<ResilienceCoordinator>>,

    /// Central monitoring coordinator (metrics, progress bars, stuck detection, audit)
    monitor: Option<Arc<Monitor>>,
}

impl PipelineRunner {
    /// Create a new pipeline runner
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

        Ok(Self {
            ctx,
            pool: None,
            stages,
            resilience: None,
            monitor: None,
        })
    }

    /// Create a runner with an existing context
    pub fn with_context(ctx: PipelineContext) -> Result<Self> {
        let _config = ctx.config.clone();
        let stages: Vec<Box<dyn PipelineStage>> = vec![
            Box::new(ExpandStage::new()?),
            Box::new(ParseStage::new()?),
            Box::new(TypecheckStage::new()),
            Box::new(ExtractStage::new()),
            Box::new(GraphStage::new()),
            Box::new(EmbedStage::new()),
        ];

        Ok(Self {
            ctx,
            pool: None,
            stages,
            resilience: None,
            monitor: None,
        })
    }

    /// Connect to the database for run tracking and initialize resilience + monitoring.
    pub async fn connect(&mut self) -> Result<()> {
        let pool = self
            .ctx
            .config
            .create_pg_pool(5)
            .await
            .context("Failed to connect to database")?;

        // Initialize resilience coordinator with checkpoint support
        let coordinator = ResilienceCoordinator::new(Some(pool.clone()), self.ctx.id.0)?;
        coordinator.ensure_checkpoint_table().await?;
        self.resilience = Some(Arc::new(coordinator));

        // Initialize monitor with database-backed audit emitter
        if self.monitor.is_none() {
            let (audit, _audit_handle) = AuditEmitter::spawn(pool.clone(), None);
            let monitor = Monitor::new(MonitorConfig::default(), audit)
                .context("Failed to create monitor")?;
            self.monitor = Some(Arc::new(monitor));
        }

        self.pool = Some(pool);
        Ok(())
    }

    /// Run the pipeline with resilience: degradation tiers, circuit breakers,
    /// memory watchdog, and checkpoint/resume.
    pub async fn run(&mut self) -> Result<PipelineResult> {
        let start = Instant::now();
        let pipeline_id = self.ctx.id.0;

        info!("Starting pipeline run: {}", pipeline_id);

        // Initialize resilience if not already done (e.g., dry-run mode without connect())
        if self.resilience.is_none() {
            let coordinator = ResilienceCoordinator::new(None, pipeline_id)?;
            self.resilience = Some(Arc::new(coordinator));
        }
        let resilience = self.resilience.as_ref().unwrap().clone();

        // Initialize monitor if not already done (dry-run / no-db path)
        if self.monitor.is_none() {
            let show_bars = !self.ctx.config.dry_run;
            let monitor = Monitor::new(
                MonitorConfig {
                    show_progress_bars: show_bars,
                },
                AuditEmitter::noop(),
            )?;
            self.monitor = Some(Arc::new(monitor));
        }
        let monitor = self.monitor.as_ref().unwrap().clone();
        let _alert_rx = monitor.start();

        // Check for a resumable checkpoint
        let resume_from_stage = if let Some(pool) = &self.pool {
            match CheckpointManager::load_latest(pool, pipeline_id).await? {
                Some(cp) => {
                    info!(
                        "Resuming from checkpoint: stage={}, files_processed={}",
                        cp.last_stage, cp.files_processed
                    );
                    Some(cp.last_stage.clone())
                }
                None => None,
            }
        } else {
            None
        };

        // Create ingestion run record
        if !self.ctx.config.dry_run {
            self.create_ingestion_run(pipeline_id).await?;
        }

        let mut results = Vec::new();
        let mut has_failures = false;
        let mut has_partial = false;
        let mut completed_stages: Vec<String> = Vec::new();
        let mut skip_until_resume = resume_from_stage.is_some();

        // Execute stages in order
        for stage in &self.stages {
            let stage_name = stage.name();

            if let Some(ref target) = self.ctx.config.from_stage {
                let target_idx = STAGE_NAMES
                    .iter()
                    .position(|s| *s == target.as_str())
                    .expect("from_stage validated at config construction");
                let current_idx = STAGE_NAMES
                    .iter()
                    .position(|s| *s == stage_name)
                    .expect("stage name must be in STAGE_NAMES");
                if current_idx < target_idx {
                    info!("Skipping stage (from_stage={}): {}", target, stage_name);
                    results.push(StageResult::skipped(stage_name));
                    continue;
                }
            }

            // If resuming, skip stages already completed
            if skip_until_resume {
                if let Some(ref resume_stage) = resume_from_stage {
                    if stage_name == resume_stage {
                        skip_until_resume = false;
                        // This stage was the last completed — skip it too
                        info!("Skipping already-completed stage (resume): {}", stage_name);
                        results.push(StageResult::skipped(stage_name));
                        continue;
                    }
                }
                info!("Skipping already-completed stage (resume): {}", stage_name);
                results.push(StageResult::skipped(stage_name));
                continue;
            }

            // Check if stage should run per config
            if !self.should_run_stage(stage_name) {
                info!("Skipping stage (config): {}", stage_name);
                results.push(StageResult::skipped(stage_name));
                continue;
            }

            // Check degradation tier — may skip stages dynamically
            let tier = resilience.current_tier();
            if !tier.should_run_stage(stage_name) {
                warn!(
                    "Skipping stage {} due to degradation tier: {}",
                    stage_name, tier
                );
                results.push(StageResult::skipped(stage_name));
                continue;
            }

            // Log resilience status before each stage
            resilience.log_status();

            // Check for emergency — flush and return partial results
            if tier == DegradationTier::Emergency {
                warn!("Emergency degradation — flushing partial results");
                has_partial = true;
                break;
            }

            if let Err(e) = self.verify_pre_stage_gate(stage_name).await {
                error!("Verification gate failed for stage {}: {}", stage_name, e);
                let stage_result = StageResult::failed(stage_name, e.to_string());
                results.push(stage_result);
                has_failures = true;
                if !self.ctx.config.continue_on_error {
                    break;
                }
                continue;
            }

            info!("Running stage: {} (tier: {})", stage_name, tier);

            // Notify monitor of stage start
            monitor.begin_stage(stage_name, 0);
            let stage_start = Instant::now();

            // Run the stage
            let result = stage.run(&self.ctx).await;

            let stage_duration_secs = stage_start.elapsed().as_secs_f64();

            match &result {
                Ok(stage_result) => {
                    info!(
                        "Stage {} completed: {} (processed: {}, failed: {}, duration: {}ms)",
                        stage_name,
                        stage_result.status,
                        stage_result.items_processed,
                        stage_result.items_failed,
                        stage_result.duration_ms
                    );

                    match stage_result.status {
                        StageStatus::Success => {
                            monitor.finish_stage(
                                stage_name,
                                stage_duration_secs,
                                stage_result.items_processed as u64,
                            );
                        }
                        StageStatus::Partial => {
                            has_partial = true;
                            monitor.finish_stage(
                                stage_name,
                                stage_duration_secs,
                                stage_result.items_processed as u64,
                            );
                        }
                        StageStatus::Failed => {
                            has_failures = true;
                            monitor.fail_stage(
                                stage_name,
                                stage_duration_secs,
                                stage_result.error.as_deref().unwrap_or("unknown"),
                            );
                            if !self.ctx.config.continue_on_error {
                                error!("Stage {} failed, stopping pipeline", stage_name);
                                results.push(stage_result.clone());
                                break;
                            }
                        }
                        StageStatus::Skipped => {}
                    }

                    results.push(stage_result.clone());
                    completed_stages.push(stage_name.to_string());

                    if stage_name == "extract" && stage_result.status == StageStatus::Success {
                        if let Err(e) = self.collect_current_fqns().await {
                            warn!("Failed to collect FQNs after extract stage: {}", e);
                        }
                    }

                    // Record stage completion
                    if !self.ctx.config.dry_run {
                        self.record_stage_completion(pipeline_id, stage_name, stage_result)
                            .await?;
                    }

                    // Write checkpoint at stage boundary
                    if let Err(e) = resilience
                        .checkpoint(stage_name, completed_stages.len(), &completed_stages)
                        .await
                    {
                        warn!(
                            "Failed to write checkpoint after stage {}: {}",
                            stage_name, e
                        );
                    }
                }
                Err(e) => {
                    error!("Stage {} errored: {}", stage_name, e);
                    monitor.fail_stage(stage_name, stage_duration_secs, &e.to_string());

                    let stage_result = StageResult::failed(stage_name, e.to_string());
                    results.push(stage_result.clone());
                    has_failures = true;

                    // Record error
                    if !self.ctx.config.dry_run {
                        self.record_stage_completion(pipeline_id, stage_name, &stage_result)
                            .await?;
                    }

                    if !self.ctx.config.continue_on_error {
                        break;
                    }
                }
            }
        }

        // Get final counts
        let state = self.ctx.state.read().await;
        let counts = state.counts.clone();
        let errors = state.errors.clone();
        let current_fqns = state.current_fqns.clone();
        drop(state);

        // Determine final status
        let status = if has_failures && !self.ctx.config.continue_on_error {
            PipelineStatus::Failed
        } else if has_failures || has_partial {
            PipelineStatus::Partial
        } else {
            PipelineStatus::Completed
        };

        let is_full_run = self.ctx.config.from_stage.is_none()
            && self.ctx.config.stages.is_none()
            && !has_partial;

        if !self.ctx.config.dry_run
            && status == PipelineStatus::Completed
            && is_full_run
            && !current_fqns.is_empty()
        {
            if let Err(e) = self.run_stale_cleanup(&current_fqns).await {
                warn!("Stale cleanup failed: {}", e);
            }
        }

        let duration = start.elapsed();

        // Update ingestion run record
        if !self.ctx.config.dry_run {
            self.complete_ingestion_run(pipeline_id, &status, &counts, &errors)
                .await?;
        }

        // Clear checkpoints on full success
        if status == PipelineStatus::Completed {
            if let Err(e) = resilience.clear_checkpoints().await {
                warn!("Failed to clear checkpoints: {}", e);
            }
        }

        // Shut down monitoring background tasks
        monitor.shutdown();

        // Final resilience status
        resilience.log_status();

        info!(
            "Pipeline run {} completed: {} (duration: {:?})",
            pipeline_id, status, duration
        );

        Ok(PipelineResult {
            id: pipeline_id,
            status,
            stages: results,
            counts,
            errors,
            duration_ms: duration.as_millis() as u64,
        })
    }

    /// Check if a stage should run based on config
    fn should_run_stage(&self, stage_name: &str) -> bool {
        match &self.ctx.config.stages {
            Some(stages) => stages.iter().any(|s| s == stage_name),
            None => true,
        }
    }

    /// Create ingestion run record in database
    async fn create_ingestion_run(&self, id: Uuid) -> Result<()> {
        let pool = self.pool.as_ref().context("Database not connected")?;

        let now = Utc::now();
        let metadata = serde_json::json!({
            "crate_path": self.ctx.config.crate_path.to_string_lossy(),
            "dry_run": self.ctx.config.dry_run,
        });

        sqlx::query(
            r#"
            INSERT INTO ingestion_runs (id, started_at, status, metadata)
            VALUES ($1, $2, 'running', $3)
            "#,
        )
        .bind(id)
        .bind(now)
        .bind(metadata)
        .execute(pool)
        .await
        .context("Failed to create ingestion run record")?;

        debug!("Created ingestion run record: {}", id);
        Ok(())
    }

    /// Record stage completion in database
    async fn record_stage_completion(
        &self,
        run_id: Uuid,
        stage_name: &str,
        result: &StageResult,
    ) -> Result<()> {
        let pool = self.pool.as_ref().context("Database not connected")?;

        // Store stage result in metadata
        let stage_metadata = serde_json::json!({
            "stage": stage_name,
            "status": result.status,
            "items_processed": result.items_processed,
            "items_failed": result.items_failed,
            "duration_ms": result.duration_ms,
            "error": result.error,
            "timestamp": result.timestamp,
        });

        // Append to metadata
        sqlx::query(
            r#"
            UPDATE ingestion_runs
            SET metadata = metadata || jsonb_build_object('stages', 
                COALESCE(metadata->'stages', '[]'::jsonb) || $2::jsonb)
            WHERE id = $1
            "#,
        )
        .bind(run_id)
        .bind(stage_metadata)
        .execute(pool)
        .await
        .context("Failed to record stage completion")?;

        Ok(())
    }

    /// Complete ingestion run record in database
    async fn complete_ingestion_run(
        &self,
        id: Uuid,
        status: &PipelineStatus,
        counts: &StageCounts,
        errors: &[StageError],
    ) -> Result<()> {
        let pool = self.pool.as_ref().context("Database not connected")?;

        let now = Utc::now();
        let errors_json = serde_json::to_value(errors).unwrap_or(serde_json::json!([]));

        sqlx::query(
            r#"
            UPDATE ingestion_runs
            SET completed_at = $2,
                status = $3,
                crates_processed = $4,
                items_extracted = $5,
                errors = $6
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(now)
        .bind(status.to_string())
        .bind(counts.files_expanded as i32)
        .bind(counts.items_extracted as i32)
        .bind(errors_json)
        .execute(pool)
        .await
        .context("Failed to complete ingestion run record")?;

        debug!(
            "Completed ingestion run record: {} with status {}",
            id, status
        );
        Ok(())
    }

    /// Get the pipeline context
    pub fn context(&self) -> &PipelineContext {
        &self.ctx
    }

    /// Get mutable access to the context
    pub fn context_mut(&mut self) -> &mut PipelineContext {
        &mut self.ctx
    }

    /// Get the resilience coordinator (if initialized).
    pub fn resilience(&self) -> Option<&Arc<ResilienceCoordinator>> {
        self.resilience.as_ref()
    }

    /// Get the monitor (if initialized).
    pub fn monitor(&self) -> Option<&Arc<Monitor>> {
        self.monitor.as_ref()
    }

    /// Attach an externally-created monitor to this runner.
    pub fn set_monitor(&mut self, monitor: Arc<Monitor>) {
        self.monitor = Some(monitor);
    }

    async fn verify_pre_stage_gate(&self, stage_name: &str) -> Result<()> {
        if self.ctx.config.dry_run {
            debug!(
                "Skipping verification gate for {} (dry_run mode)",
                stage_name
            );
            return Ok(());
        }
        let crate_name = match &self.ctx.config.crate_name {
            Some(name) => name,
            None => {
                debug!(
                    "Skipping verification gate for {} (no crate_name)",
                    stage_name
                );
                return Ok(());
            }
        };
        match stage_name {
            "graph" => self.verify_pre_graph_gate(crate_name).await,
            "embed" => self.verify_pre_embed_gate(crate_name).await,
            _ => Ok(()),
        }
    }

    async fn verify_pre_graph_gate(&self, crate_name: &str) -> Result<()> {
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                debug!("Skipping pre-graph gate (no database connection)");
                return Ok(());
            }
        };
        let orphan_count: i64 = sqlx::query_scalar(
            r#"SELECT count(*)::bigint FROM extracted_items WHERE source_file_id IS NULL AND crate_name = $1"#
        )
        .bind(crate_name)
        .fetch_one(pool)
        .await
        .context("Failed to query orphan count for pre-graph gate")?;
        if orphan_count > 0 {
            anyhow::bail!(
                "Verification gate failed: {} extracted items have NULL source_file_id for crate '{}'.\n\
                 This indicates the Extract stage did not complete successfully.\n\
                 Fix: re-run ingestion from the extract stage: --from-stage extract",
                orphan_count, crate_name
            );
        }
        info!(
            "Pre-graph gate passed: no orphaned extracted_items for crate {}",
            crate_name
        );
        Ok(())
    }

    async fn verify_pre_embed_gate(&self, crate_name: &str) -> Result<()> {
        let neo4j_url = match &self.ctx.config.neo4j_url {
            Some(url) => url,
            None => {
                debug!("Skipping pre-embed gate (no Neo4j URL configured)");
                return Ok(());
            }
        };
        let pool = match &self.pool {
            Some(p) => p,
            None => {
                debug!("Skipping pre-embed gate (no database connection)");
                return Ok(());
            }
        };
        let sampled_fqns: Vec<String> = sqlx::query_scalar(
            r#"SELECT fqn FROM extracted_items WHERE crate_name = $1 ORDER BY random() LIMIT 10"#,
        )
        .bind(crate_name)
        .fetch_all(pool)
        .await
        .context("Failed to sample FQNs for pre-embed gate")?;
        if sampled_fqns.is_empty() {
            debug!(
                "Skipping pre-embed gate (no extracted_items found for crate {})",
                crate_name
            );
            return Ok(());
        }
        let graph = match neo4rs::Graph::new(
            neo4j_url,
            std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
            std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string()),
        )
        .await
        {
            Ok(g) => g,
            Err(e) => {
                warn!("Skipping pre-embed gate: failed to connect to Neo4j: {}", e);
                return Ok(());
            }
        };
        for fqn in &sampled_fqns {
            let query = neo4rs::query("MATCH (n {fqn: $fqn}) RETURN count(n) as cnt")
                .param("fqn", fqn.clone());
            let mut result = graph
                .execute(query)
                .await
                .context("Failed to execute Neo4j query")?;
            match result.next().await {
                Ok(Some(row)) => {
                    let count: i64 = row.get("cnt").unwrap_or(0);
                    if count == 0 {
                        anyhow::bail!(
                            "Verification gate failed: Neo4j node missing for FQN '{}'.\n\
                             This indicates the Graph stage did not complete successfully.\n\
                             Fix: re-run ingestion from the graph stage: --from-stage graph",
                            fqn
                        );
                    }
                }
                Ok(None) => {
                    anyhow::bail!(
                        "Verification gate failed: Neo4j query returned no result for FQN '{}'.\n\
                         This indicates the Graph stage did not complete successfully.\n\
                         Fix: re-run ingestion from the graph stage: --from-stage graph",
                        fqn
                    );
                }
                Err(e) => {
                    warn!("Skipping pre-embed gate: Neo4j query error: {}", e);
                    return Ok(());
                }
            }
        }
        info!(
            "Pre-embed gate passed: all {} sampled FQNs have Neo4j nodes for crate {}",
            sampled_fqns.len(),
            crate_name
        );
        Ok(())
    }

    async fn collect_current_fqns(&self) -> Result<()> {
        let pool = match &self.pool {
            Some(p) => p,
            None => return Ok(()),
        };
        let crate_name = match &self.ctx.config.crate_name {
            Some(name) => name,
            None => return Ok(()),
        };
        let fqns: std::collections::HashSet<String> =
            sqlx::query_scalar("SELECT fqn FROM extracted_items WHERE crate_name = $1")
                .bind(crate_name)
                .fetch_all(pool)
                .await
                .context("Failed to query FQNs for stale detection")?
                .into_iter()
                .collect();
        let mut state = self.ctx.state.write().await;
        state.current_fqns = fqns;
        info!(
            "Collected {} current FQNs for stale detection",
            state.current_fqns.len()
        );
        Ok(())
    }

    async fn run_stale_cleanup(
        &self,
        current_fqns: &std::collections::HashSet<String>,
    ) -> Result<()> {
        let pool = match &self.pool {
            Some(p) => p,
            None => return Ok(()),
        };
        let crate_name = match &self.ctx.config.crate_name {
            Some(name) => name,
            None => return Ok(()),
        };
        info!("Running stale cleanup for crate {}", crate_name);
        let report = crate::pipeline::stages::DataLifecycleManager::cleanup_stale_items(
            crate_name,
            current_fqns,
            pool,
            self.ctx.config.neo4j_url.as_deref(),
            self.ctx.config.embedding_url.as_deref(),
        )
        .await
        .context("Stale cleanup failed")?;
        if report.stale_count > 0 {
            info!(
                "Stale cleanup complete: {} stale items deleted (Postgres: {}, Neo4j: {}, Qdrant: {})",
                report.stale_count, report.postgres_deleted, report.neo4j_deleted, report.qdrant_deleted
            );
        }
        Ok(())
    }

    /// Create a runner that resumes from a previous checkpoint.
    ///
    /// Looks up the latest checkpoint for the given run_id and creates
    /// a runner with that ID so `run()` will skip already-completed stages.
    pub async fn resume(config: PipelineConfig, run_id: Uuid) -> Result<Self> {
        let ctx = PipelineContext::with_id(run_id, config);
        let stages: Vec<Box<dyn PipelineStage>> = vec![
            Box::new(ExpandStage::new()?),
            Box::new(ParseStage::new()?),
            Box::new(TypecheckStage::new()),
            Box::new(ExtractStage::new()),
            Box::new(GraphStage::new()),
            Box::new(EmbedStage::new()),
        ];

        Ok(Self {
            ctx,
            pool: None,
            stages,
            resilience: None,
            monitor: None,
        })
    }
}

/// Run a single stage by name (for testing/debugging)
pub async fn run_single_stage(config: PipelineConfig, stage_name: &str) -> Result<StageResult> {
    let ctx = PipelineContext::new(config);

    let stage: Box<dyn PipelineStage> = match stage_name {
        "expand" => Box::new(ExpandStage::new()?),
        "parse" => Box::new(ParseStage::new()?),
        "typecheck" => Box::new(TypecheckStage::new()),
        "extract" => Box::new(ExtractStage::new()),
        "graph" => Box::new(GraphStage::new()),
        "embed" => Box::new(EmbedStage::new()),
        _ => anyhow::bail!("Unknown stage: {}", stage_name),
    };

    stage.run(&ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::STAGE_NAMES;
    use std::path::PathBuf;

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
    fn test_pipeline_runner_creation() {
        let config = test_config();
        let runner = PipelineRunner::new(config);
        assert!(runner.is_ok());
    }

    #[test]
    fn test_should_run_stage() {
        let config = PipelineConfig {
            stages: Some(vec!["expand".to_string(), "parse".to_string()]),
            ..test_config()
        };

        let runner = PipelineRunner::new(config).unwrap();

        assert!(runner.should_run_stage("expand"));
        assert!(runner.should_run_stage("parse"));
        assert!(!runner.should_run_stage("embed"));
    }

    #[test]
    fn test_all_stages_run_by_default() {
        let config = test_config();
        let runner = PipelineRunner::new(config).unwrap();

        for stage_name in STAGE_NAMES {
            assert!(runner.should_run_stage(stage_name));
        }
    }
}
