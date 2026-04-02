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
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&self.ctx.config.database_url)
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
        drop(state);

        // Determine final status
        let status = if has_failures && !self.ctx.config.continue_on_error {
            PipelineStatus::Failed
        } else if has_failures || has_partial {
            PipelineStatus::Partial
        } else {
            PipelineStatus::Completed
        };

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
            database_url: "postgresql://test:test@localhost:5432/test".to_string(),
            neo4j_url: None,
            embedding_url: None,
            stages: None,
            dry_run: false,
            continue_on_error: true,
            max_concurrency: 4,
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
