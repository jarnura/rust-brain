//! Pipeline runner for sequential stage execution
//!
//! Executes pipeline stages in order, handling errors gracefully
//! and recording progress to the database.

use crate::pipeline::{
    PipelineConfig, PipelineContext, PipelineResult, PipelineStatus,
    PipelineStage, StageCounts,
};
use crate::pipeline::stages::{
    ExpandStage, ExtractStage, GraphStage, ParseStage, StageError, StageResult, StageStatus,
    TypecheckStage, EmbedStage,
};
use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::PgPool;
use std::time::Instant;
use tracing::{debug, error, info};
use uuid::Uuid;

/// Pipeline runner that orchestrates stage execution
pub struct PipelineRunner {
    /// Pipeline context
    ctx: PipelineContext,
    
    /// Database pool for recording runs
    pool: Option<PgPool>,
    
    /// Stage implementations
    stages: Vec<Box<dyn PipelineStage>>,
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
        })
    }
    
    /// Connect to the database for run tracking
    pub async fn connect(&mut self) -> Result<()> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .connect(&self.ctx.config.database_url)
            .await
            .context("Failed to connect to database")?;
        
        self.pool = Some(pool);
        Ok(())
    }
    
    /// Run the pipeline
    pub async fn run(&mut self) -> Result<PipelineResult> {
        let start = Instant::now();
        let pipeline_id = self.ctx.id.0;
        
        info!("Starting pipeline run: {}", pipeline_id);
        
        // Create ingestion run record
        if !self.ctx.config.dry_run {
            self.create_ingestion_run(pipeline_id).await?;
        }
        
        let mut results = Vec::new();
        let mut has_failures = false;
        let mut has_partial = false;
        
        // Execute stages in order
        for stage in &self.stages {
            let stage_name = stage.name();
            
            // Check if stage should run
            if !self.should_run_stage(stage_name) {
                info!("Skipping stage: {}", stage_name);
                results.push(StageResult::skipped(stage_name));
                continue;
            }
            
            info!("Running stage: {}", stage_name);
            
            // Run the stage
            let result = stage.run(&self.ctx).await;
            
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
                        StageStatus::Success => {}
                        StageStatus::Partial => {
                            has_partial = true;
                        }
                        StageStatus::Failed => {
                            has_failures = true;
                            // Check if we should continue
                            if !self.ctx.config.continue_on_error {
                                error!("Stage {} failed, stopping pipeline", stage_name);
                                results.push(stage_result.clone());
                                break;
                            }
                        }
                        StageStatus::Skipped => {}
                    }
                    
                    results.push(stage_result.clone());
                    
                    // Record stage completion
                    if !self.ctx.config.dry_run {
                        self.record_stage_completion(pipeline_id, stage_name, stage_result)
                            .await?;
                    }
                }
                Err(e) => {
                    error!("Stage {} errored: {}", stage_name, e);
                    
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
        } else if has_failures {
            PipelineStatus::Partial
        } else if has_partial {
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
        let pool = self.pool.as_ref()
            .context("Database not connected")?;
        
        let now = Utc::now();
        let metadata = serde_json::json!({
            "crate_path": self.ctx.config.crate_path.to_string_lossy(),
            "dry_run": self.ctx.config.dry_run,
        });
        
        sqlx::query(
            r#"
            INSERT INTO ingestion_runs (id, started_at, status, metadata)
            VALUES ($1, $2, 'running', $3)
            "#
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
        let pool = self.pool.as_ref()
            .context("Database not connected")?;
        
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
            "#
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
        let pool = self.pool.as_ref()
            .context("Database not connected")?;
        
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
            "#
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
        
        debug!("Completed ingestion run record: {} with status {}", id, status);
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
}

/// Run a single stage by name (for testing/debugging)
pub async fn run_single_stage(
    config: PipelineConfig,
    stage_name: &str,
) -> Result<StageResult> {
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

    #[test]
    fn test_pipeline_runner_creation() {
        let config = PipelineConfig {
            crate_path: PathBuf::from("."),
            ..Default::default()
        };
        
        let runner = PipelineRunner::new(config);
        assert!(runner.is_ok());
    }
    
    #[test]
    fn test_should_run_stage() {
        let config = PipelineConfig {
            crate_path: PathBuf::from("."),
            stages: Some(vec!["expand".to_string(), "parse".to_string()]),
            ..Default::default()
        };
        
        let runner = PipelineRunner::new(config).unwrap();
        
        assert!(runner.should_run_stage("expand"));
        assert!(runner.should_run_stage("parse"));
        assert!(!runner.should_run_stage("embed"));
    }
    
    #[test]
    fn test_all_stages_run_by_default() {
        let config = PipelineConfig::default();
        let runner = PipelineRunner::new(config).unwrap();
        
        for stage_name in STAGE_NAMES {
            assert!(runner.should_run_stage(stage_name));
        }
    }
}
