//! Defense-in-depth resilience mechanisms for the ingestion pipeline.
//!
//! - **MemoryWatchdog**: Polls `/proc/self/statm` every 500ms, publishes pressure levels.
//! - **SpillStore**: Spills parsed items to temp files when memory is elevated or count > 1000.
//! - **DegradationTier**: Controls which stages run based on system health.
//! - **CheckpointManager**: Writes checkpoints to Postgres every N files; resumes on restart.

use crate::pipeline::ParsedItemInfo;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// =============================================================================
// MEMORY PRESSURE
// =============================================================================

/// Memory pressure levels, ordered by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum MemoryPressure {
    /// < 50% of available memory used
    Normal = 0,
    /// 50–70%
    Elevated = 1,
    /// 70–85%
    High = 2,
    /// 85–95%
    Critical = 3,
    /// > 95%
    Emergency = 4,
}

impl From<u8> for MemoryPressure {
    fn from(v: u8) -> Self {
        match v {
            0 => Self::Normal,
            1 => Self::Elevated,
            2 => Self::High,
            3 => Self::Critical,
            4 => Self::Emergency,
            _ => Self::Emergency,
        }
    }
}

impl std::fmt::Display for MemoryPressure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "normal"),
            Self::Elevated => write!(f, "elevated"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
            Self::Emergency => write!(f, "emergency"),
        }
    }
}

impl MemoryPressure {
    /// Classify a usage ratio (0.0–1.0) into a pressure level.
    pub fn from_ratio(ratio: f64) -> Self {
        if ratio >= 0.95 {
            Self::Emergency
        } else if ratio >= 0.85 {
            Self::Critical
        } else if ratio >= 0.70 {
            Self::High
        } else if ratio >= 0.50 {
            Self::Elevated
        } else {
            Self::Normal
        }
    }
}

/// Snapshot of process memory from /proc/self/statm.
#[derive(Debug, Clone, Copy)]
pub struct MemorySnapshot {
    /// Resident set size in bytes
    pub rss_bytes: u64,
    /// Total system memory in bytes
    pub total_bytes: u64,
    /// Usage ratio (rss / total)
    pub ratio: f64,
    /// Derived pressure level
    pub pressure: MemoryPressure,
}

/// Read RSS from `/proc/self/statm` and total memory from `/proc/meminfo`,
/// capped by the container's cgroup memory limit when running in a container.
///
/// `/proc/self/statm` fields (in pages): size resident shared text lib data dt
/// We read field 1 (resident) and multiply by page size.
fn read_memory_snapshot() -> Option<MemorySnapshot> {
    // Read RSS from /proc/self/statm
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let rss_bytes = rss_pages * page_size;

    // Read host total memory from /proc/meminfo
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let host_total_kb: u64 = meminfo
        .lines()
        .find(|l| l.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let host_total_bytes = host_total_kb * 1024;

    // Read cgroup memory limit (container-aware).
    // Use the smaller of host memory and cgroup limit.
    let cgroup_limit = read_cgroup_memory_limit();
    let total_bytes = match cgroup_limit {
        Some(limit) if limit < host_total_bytes => limit,
        _ => host_total_bytes,
    };

    let ratio = if total_bytes > 0 {
        rss_bytes as f64 / total_bytes as f64
    } else {
        0.0
    };

    Some(MemorySnapshot {
        rss_bytes,
        total_bytes,
        ratio,
        pressure: MemoryPressure::from_ratio(ratio),
    })
}

/// Read the cgroup memory limit, trying cgroup v2 first, then v1.
/// Returns `None` if no cgroup limit is set or the limit is effectively unlimited.
fn read_cgroup_memory_limit() -> Option<u64> {
    // cgroup v2: /sys/fs/cgroup/memory.max — contains a number or "max"
    if let Ok(content) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        let trimmed = content.trim();
        if trimmed != "max" {
            if let Ok(limit) = trimmed.parse::<u64>() {
                return Some(limit);
            }
        }
        return None;
    }

    // cgroup v1: /sys/fs/cgroup/memory/memory.limit_in_bytes
    // A value near u64::MAX (e.g. 9223372036854771712) means no limit.
    if let Ok(content) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Ok(limit) = content.trim().parse::<u64>() {
            // Values above 1 exbibyte are effectively "no limit"
            if limit < (1u64 << 60) {
                return Some(limit);
            }
        }
    }

    None
}

/// Background thread that polls memory every 500ms and publishes pressure
/// via a `tokio::sync::watch` channel and an atomic for lock-free reads.
pub struct MemoryWatchdog {
    /// Atomic for lock-free pressure reads on the hot path
    pressure: Arc<AtomicU8>,
    /// Watch channel for async subscribers that want to react to changes
    rx: watch::Receiver<MemoryPressure>,
    /// Handle to the background task (kept alive as long as the watchdog lives)
    _handle: tokio::task::JoinHandle<()>,
}

impl MemoryWatchdog {
    /// Spawn the watchdog. Returns immediately.
    pub fn spawn() -> Self {
        let pressure = Arc::new(AtomicU8::new(MemoryPressure::Normal as u8));
        let (tx, rx) = watch::channel(MemoryPressure::Normal);
        let pressure_clone = pressure.clone();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
            let mut last_logged = MemoryPressure::Normal;

            loop {
                interval.tick().await;

                let level = match read_memory_snapshot() {
                    Some(snap) => {
                        if snap.pressure != last_logged {
                            match snap.pressure {
                                MemoryPressure::Normal => {
                                    if last_logged >= MemoryPressure::Elevated {
                                        info!(
                                            "Memory pressure: normal (RSS={:.1}MB, {:.1}%)",
                                            snap.rss_bytes as f64 / 1_048_576.0,
                                            snap.ratio * 100.0
                                        );
                                    }
                                }
                                MemoryPressure::Elevated => {
                                    info!(
                                        "Memory pressure: elevated (RSS={:.1}MB, {:.1}%)",
                                        snap.rss_bytes as f64 / 1_048_576.0,
                                        snap.ratio * 100.0
                                    );
                                }
                                MemoryPressure::High => {
                                    warn!(
                                        "Memory pressure: HIGH (RSS={:.1}MB, {:.1}%)",
                                        snap.rss_bytes as f64 / 1_048_576.0,
                                        snap.ratio * 100.0
                                    );
                                }
                                MemoryPressure::Critical => {
                                    warn!(
                                        "Memory pressure: CRITICAL (RSS={:.1}MB, {:.1}%)",
                                        snap.rss_bytes as f64 / 1_048_576.0,
                                        snap.ratio * 100.0
                                    );
                                }
                                MemoryPressure::Emergency => {
                                    error!(
                                        "Memory pressure: EMERGENCY (RSS={:.1}MB, {:.1}%)",
                                        snap.rss_bytes as f64 / 1_048_576.0,
                                        snap.ratio * 100.0
                                    );
                                }
                            }
                            last_logged = snap.pressure;
                        }
                        snap.pressure
                    }
                    None => {
                        // /proc not available (macOS, etc.) — assume normal
                        MemoryPressure::Normal
                    }
                };

                pressure_clone.store(level as u8, Ordering::Release);
                let _ = tx.send(level);
            }
        });

        Self {
            pressure,
            rx,
            _handle: handle,
        }
    }

    /// Lock-free read of current pressure level.
    pub fn current_pressure(&self) -> MemoryPressure {
        MemoryPressure::from(self.pressure.load(Ordering::Acquire))
    }

    /// Subscribe to pressure changes (clone of the watch receiver).
    pub fn subscribe(&self) -> watch::Receiver<MemoryPressure> {
        self.rx.clone()
    }
}

// =============================================================================
// SPILL-TO-DISK STORE
// =============================================================================

/// Spills `ParsedItemInfo` batches to temp files when in-memory count exceeds
/// a threshold or memory pressure is elevated, and drains them back for the
/// graph stage.
///
/// Format: one JSON line per item (JSONL), gzip would be overkill for temp data.
pub struct SpillStore {
    /// Directory for spill files
    spill_dir: PathBuf,
    /// Paths of spill files written (in order)
    spill_files: Vec<PathBuf>,
    /// Current in-memory count across all files
    in_memory_count: usize,
    /// Threshold before spilling
    spill_threshold: usize,
}

impl SpillStore {
    /// Create a new spill store rooted at the given directory.
    pub fn new(spill_dir: PathBuf, spill_threshold: usize) -> Result<Self> {
        std::fs::create_dir_all(&spill_dir)
            .with_context(|| format!("Failed to create spill dir: {}", spill_dir.display()))?;
        Ok(Self {
            spill_dir,
            spill_files: Vec::new(),
            in_memory_count: 0,
            spill_threshold,
        })
    }

    /// Create with default threshold of 1000 items, using a temp directory.
    pub fn with_defaults() -> Result<Self> {
        let spill_dir = std::env::temp_dir().join(format!("rustbrain-spill-{}", Uuid::new_v4()));
        Self::new(spill_dir, 1000)
    }

    /// Check whether items should be spilled based on count and pressure.
    pub fn should_spill(&self, current_count: usize, pressure: MemoryPressure) -> bool {
        current_count > self.spill_threshold || pressure >= MemoryPressure::Elevated
    }

    /// Spill a batch of items to a temp file. Returns the number of items spilled.
    pub fn spill(&mut self, items: &[ParsedItemInfo]) -> Result<usize> {
        if items.is_empty() {
            return Ok(0);
        }

        let file_name = format!("spill-{:04}.jsonl", self.spill_files.len());
        let path = self.spill_dir.join(&file_name);

        let file = std::fs::File::create(&path)
            .with_context(|| format!("Failed to create spill file: {}", path.display()))?;
        let mut writer = BufWriter::new(file);

        let mut count = 0;
        for item in items {
            let line = serde_json::to_string(item)
                .with_context(|| format!("Failed to serialize item: {}", item.fqn))?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
            count += 1;
        }
        writer.flush()?;

        info!("Spilled {} items to {}", count, path.display());
        self.spill_files.push(path);
        self.in_memory_count = self.in_memory_count.saturating_sub(count);
        Ok(count)
    }

    /// Drain all spilled items back into memory. Yields items file-by-file
    /// to avoid loading everything at once.
    pub fn drain(&mut self) -> Result<Vec<Vec<ParsedItemInfo>>> {
        let mut batches = Vec::with_capacity(self.spill_files.len());

        for path in &self.spill_files {
            let file = std::fs::File::open(path)
                .with_context(|| format!("Failed to open spill file: {}", path.display()))?;
            let reader = BufReader::new(file);
            let mut items = Vec::new();

            for line in reader.lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                let item: ParsedItemInfo = serde_json::from_str(&line).with_context(|| {
                    format!("Failed to deserialize spill line in {}", path.display())
                })?;
                items.push(item);
            }

            debug!("Drained {} items from {}", items.len(), path.display());
            batches.push(items);
        }

        Ok(batches)
    }

    /// Drain items one file at a time via a callback, to keep memory bounded.
    pub fn drain_streaming<F>(&mut self, mut callback: F) -> Result<usize>
    where
        F: FnMut(Vec<ParsedItemInfo>) -> Result<()>,
    {
        let mut total = 0;

        for path in &self.spill_files {
            let file = std::fs::File::open(path)
                .with_context(|| format!("Failed to open spill file: {}", path.display()))?;
            let reader = BufReader::new(file);
            let mut items = Vec::new();

            for line in reader.lines() {
                let line = line?;
                if line.is_empty() {
                    continue;
                }
                let item: ParsedItemInfo = serde_json::from_str(&line)?;
                items.push(item);
            }

            total += items.len();
            callback(items)?;
        }

        Ok(total)
    }

    /// Clean up all spill files and the directory.
    pub fn cleanup(&mut self) -> Result<()> {
        for path in &self.spill_files {
            if path.exists() {
                std::fs::remove_file(path).ok();
            }
        }
        self.spill_files.clear();

        if self.spill_dir.exists() {
            std::fs::remove_dir_all(&self.spill_dir).ok();
        }

        debug!("Cleaned up spill directory: {}", self.spill_dir.display());
        Ok(())
    }

    /// Number of spill files written.
    pub fn spill_file_count(&self) -> usize {
        self.spill_files.len()
    }

    /// Update tracked in-memory count (called by pipeline when items are added).
    pub fn set_in_memory_count(&mut self, count: usize) {
        self.in_memory_count = count;
    }
}

impl Drop for SpillStore {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

// =============================================================================
// DEGRADATION TIERS
// =============================================================================

/// Degradation tiers control which pipeline stages execute based on
/// system health (memory pressure, circuit breaker state, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DegradationTier {
    /// All stages run: expand, parse, typecheck, extract, graph, embed
    Full,
    /// Skip embedding stage (Ollama/Qdrant unavailable or memory high)
    Reduced,
    /// Parse only + write to Postgres (Neo4j and Ollama both unavailable)
    Minimal,
    /// Flush whatever we have and return partial results immediately
    Emergency,
}

impl std::fmt::Display for DegradationTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Reduced => write!(f, "reduced"),
            Self::Minimal => write!(f, "minimal"),
            Self::Emergency => write!(f, "emergency"),
        }
    }
}

impl DegradationTier {
    /// Determine the tier from current system state.
    pub fn from_state(pressure: MemoryPressure, neo4j_open: bool, ollama_open: bool) -> Self {
        if pressure >= MemoryPressure::Emergency {
            return Self::Emergency;
        }
        if pressure >= MemoryPressure::Critical {
            return Self::Minimal;
        }
        if neo4j_open && ollama_open {
            return Self::Minimal;
        }
        if ollama_open || pressure >= MemoryPressure::High {
            return Self::Reduced;
        }
        Self::Full
    }

    /// Whether the given stage should run at this tier.
    pub fn should_run_stage(&self, stage_name: &str) -> bool {
        match self {
            Self::Full => true,
            Self::Reduced => stage_name != "embed",
            Self::Minimal => matches!(stage_name, "expand" | "parse" | "typecheck" | "extract"),
            Self::Emergency => matches!(stage_name, "expand" | "parse"),
        }
    }

    /// List of stages that run at this tier.
    pub fn active_stages(&self) -> &[&str] {
        match self {
            Self::Full => &["expand", "parse", "typecheck", "extract", "graph", "embed"],
            Self::Reduced => &["expand", "parse", "typecheck", "extract", "graph"],
            Self::Minimal => &["expand", "parse", "typecheck", "extract"],
            Self::Emergency => &["expand", "parse"],
        }
    }
}

// =============================================================================
// CHECKPOINT / RESUME
// =============================================================================

/// Checkpoint data persisted to Postgres for crash recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Pipeline run ID
    pub run_id: Uuid,
    /// Last completed stage name
    pub last_stage: String,
    /// Number of files processed so far
    pub files_processed: usize,
    /// Timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Degradation tier at checkpoint time
    pub tier: String,
    /// Files that were successfully processed (for skip-on-resume)
    pub completed_files: Vec<String>,
}

/// Manages checkpoint writes to Postgres and resume-from-checkpoint logic.
pub struct CheckpointManager {
    pool: PgPool,
    run_id: Uuid,
    /// Write a checkpoint every N files
    checkpoint_interval: usize,
    /// Counter since last checkpoint
    since_last_checkpoint: usize,
}

impl CheckpointManager {
    pub fn new(pool: PgPool, run_id: Uuid) -> Self {
        Self {
            pool,
            run_id,
            checkpoint_interval: 100,
            since_last_checkpoint: 0,
        }
    }

    pub fn with_interval(mut self, interval: usize) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    /// Ensure the checkpoints table exists.
    pub async fn ensure_table(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS pipeline_checkpoints (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                run_id UUID NOT NULL,
                last_stage TEXT NOT NULL,
                files_processed INTEGER NOT NULL DEFAULT 0,
                tier TEXT NOT NULL DEFAULT 'full',
                completed_files JSONB NOT NULL DEFAULT '[]'::jsonb,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .context("Failed to create pipeline_checkpoints table")?;

        // Index for fast lookup by run_id
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_pipeline_checkpoints_run_id
            ON pipeline_checkpoints (run_id, created_at DESC)
            "#,
        )
        .execute(&self.pool)
        .await
        .ok(); // ignore if already exists

        Ok(())
    }

    /// Record a file as processed. Writes a checkpoint every `checkpoint_interval` files.
    pub async fn record_file(
        &mut self,
        stage: &str,
        _file_path: &str,
        tier: DegradationTier,
        completed_files: &[String],
    ) -> Result<bool> {
        self.since_last_checkpoint += 1;

        if self.since_last_checkpoint >= self.checkpoint_interval {
            self.write_checkpoint(stage, completed_files.len(), tier, completed_files)
                .await?;
            self.since_last_checkpoint = 0;
            return Ok(true);
        }
        Ok(false)
    }

    /// Force-write a checkpoint (e.g., at stage boundaries).
    pub async fn write_checkpoint(
        &self,
        stage: &str,
        files_processed: usize,
        tier: DegradationTier,
        completed_files: &[String],
    ) -> Result<()> {
        let completed_json = serde_json::to_value(completed_files)?;

        sqlx::query(
            r#"
            INSERT INTO pipeline_checkpoints (run_id, last_stage, files_processed, tier, completed_files)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(self.run_id)
        .bind(stage)
        .bind(files_processed as i32)
        .bind(tier.to_string())
        .bind(completed_json)
        .execute(&self.pool)
        .await
        .context("Failed to write checkpoint")?;

        info!(
            "Checkpoint written: run={}, stage={}, files={}, tier={}",
            self.run_id, stage, files_processed, tier
        );
        Ok(())
    }

    /// Load the most recent checkpoint for a given run ID.
    /// Returns None if no checkpoint exists (fresh run).
    pub async fn load_latest(pool: &PgPool, run_id: Uuid) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            r#"
            SELECT run_id, last_stage, files_processed, tier, completed_files, created_at
            FROM pipeline_checkpoints
            WHERE run_id = $1
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .context("Failed to load checkpoint")?;

        match row {
            Some(r) => {
                let completed_files: Vec<String> =
                    serde_json::from_value(r.completed_files).unwrap_or_default();
                Ok(Some(Checkpoint {
                    run_id: r.run_id,
                    last_stage: r.last_stage,
                    files_processed: r.files_processed as usize,
                    created_at: r.created_at,
                    tier: r.tier,
                    completed_files,
                }))
            }
            None => Ok(None),
        }
    }

    /// Find the most recent checkpoint across all runs for a given crate path.
    /// Useful for resuming after a crash when the run_id is lost.
    pub async fn find_resumable(pool: &PgPool) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            r#"
            SELECT run_id, last_stage, files_processed, tier, completed_files, created_at
            FROM pipeline_checkpoints
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(pool)
        .await
        .context("Failed to find resumable checkpoint")?;

        match row {
            Some(r) => {
                let completed_files: Vec<String> =
                    serde_json::from_value(r.completed_files).unwrap_or_default();
                Ok(Some(Checkpoint {
                    run_id: r.run_id,
                    last_stage: r.last_stage,
                    files_processed: r.files_processed as usize,
                    created_at: r.created_at,
                    tier: r.tier,
                    completed_files,
                }))
            }
            None => Ok(None),
        }
    }

    /// Delete all checkpoints for a run (called after successful completion).
    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM pipeline_checkpoints WHERE run_id = $1")
            .bind(self.run_id)
            .execute(&self.pool)
            .await
            .context("Failed to clear checkpoints")?;
        debug!("Cleared checkpoints for run {}", self.run_id);
        Ok(())
    }
}

/// Internal row type for sqlx deserialization.
#[derive(sqlx::FromRow)]
struct CheckpointRow {
    run_id: Uuid,
    last_stage: String,
    files_processed: i32,
    tier: String,
    completed_files: serde_json::Value,
    created_at: chrono::DateTime<chrono::Utc>,
}

// =============================================================================
// RESILIENCE COORDINATOR
// =============================================================================

/// Bundles all resilience mechanisms into a single handle that stages can query.
pub struct ResilienceCoordinator {
    pub watchdog: MemoryWatchdog,
    pub spill_store: std::sync::Mutex<SpillStore>,
    pub neo4j_breaker: crate::pipeline::circuit_breaker::CircuitBreaker,
    pub ollama_breaker: crate::pipeline::circuit_breaker::CircuitBreaker,
    pub qdrant_breaker: crate::pipeline::circuit_breaker::CircuitBreaker,
    pub checkpoint_mgr: Option<tokio::sync::Mutex<CheckpointManager>>,
}

impl ResilienceCoordinator {
    /// Create a fully-wired coordinator.
    pub fn new(pool: Option<PgPool>, run_id: Uuid) -> Result<Self> {
        let spill_store = SpillStore::with_defaults()?;
        let checkpoint_mgr =
            pool.map(|p| tokio::sync::Mutex::new(CheckpointManager::new(p, run_id)));

        Ok(Self {
            watchdog: MemoryWatchdog::spawn(),
            spill_store: std::sync::Mutex::new(spill_store),
            neo4j_breaker: crate::pipeline::circuit_breaker::CircuitBreaker::neo4j(),
            ollama_breaker: crate::pipeline::circuit_breaker::CircuitBreaker::ollama(),
            qdrant_breaker: crate::pipeline::circuit_breaker::CircuitBreaker::qdrant(),
            checkpoint_mgr,
        })
    }

    /// Compute the current degradation tier.
    pub fn current_tier(&self) -> DegradationTier {
        let pressure = self.watchdog.current_pressure();
        let neo4j_open =
            self.neo4j_breaker.state() == crate::pipeline::circuit_breaker::CircuitState::Open;
        let ollama_open =
            self.ollama_breaker.state() == crate::pipeline::circuit_breaker::CircuitState::Open;

        DegradationTier::from_state(pressure, neo4j_open, ollama_open)
    }

    /// Whether a stage should run right now, considering degradation.
    pub fn should_run_stage(&self, stage_name: &str) -> bool {
        self.current_tier().should_run_stage(stage_name)
    }

    /// Log a summary of all resilience state.
    pub fn log_status(&self) {
        let tier = self.current_tier();
        let pressure = self.watchdog.current_pressure();
        let neo4j = self.neo4j_breaker.metrics();
        let ollama = self.ollama_breaker.metrics();
        let qdrant = self.qdrant_breaker.metrics();
        let spill_count = self
            .spill_store
            .lock()
            .map(|s| s.spill_file_count())
            .unwrap_or(0);

        info!(
            "Resilience status: tier={}, pressure={}, spill_files={}, {}; {}; {}",
            tier, pressure, spill_count, neo4j, ollama, qdrant
        );
    }

    /// Ensure checkpoint table exists (call once at startup).
    pub async fn ensure_checkpoint_table(&self) -> Result<()> {
        if let Some(ref mgr) = self.checkpoint_mgr {
            mgr.lock().await.ensure_table().await?;
        }
        Ok(())
    }

    /// Write a checkpoint through the coordinator.
    pub async fn checkpoint(
        &self,
        stage: &str,
        files_processed: usize,
        completed_files: &[String],
    ) -> Result<()> {
        if let Some(ref mgr) = self.checkpoint_mgr {
            let tier = self.current_tier();
            mgr.lock()
                .await
                .write_checkpoint(stage, files_processed, tier, completed_files)
                .await?;
        }
        Ok(())
    }

    /// Record a single file and auto-checkpoint if interval reached.
    pub async fn record_file(
        &self,
        stage: &str,
        file_path: &str,
        completed_files: &[String],
    ) -> Result<bool> {
        if let Some(ref mgr) = self.checkpoint_mgr {
            let tier = self.current_tier();
            mgr.lock()
                .await
                .record_file(stage, file_path, tier, completed_files)
                .await
        } else {
            Ok(false)
        }
    }

    /// Clear checkpoints after successful completion.
    pub async fn clear_checkpoints(&self) -> Result<()> {
        if let Some(ref mgr) = self.checkpoint_mgr {
            mgr.lock().await.clear().await?;
        }
        Ok(())
    }
}

// =============================================================================
// TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- MemoryPressure ----

    #[test]
    fn test_pressure_from_ratio() {
        assert_eq!(MemoryPressure::from_ratio(0.0), MemoryPressure::Normal);
        assert_eq!(MemoryPressure::from_ratio(0.49), MemoryPressure::Normal);
        assert_eq!(MemoryPressure::from_ratio(0.50), MemoryPressure::Elevated);
        assert_eq!(MemoryPressure::from_ratio(0.69), MemoryPressure::Elevated);
        assert_eq!(MemoryPressure::from_ratio(0.70), MemoryPressure::High);
        assert_eq!(MemoryPressure::from_ratio(0.84), MemoryPressure::High);
        assert_eq!(MemoryPressure::from_ratio(0.85), MemoryPressure::Critical);
        assert_eq!(MemoryPressure::from_ratio(0.94), MemoryPressure::Critical);
        assert_eq!(MemoryPressure::from_ratio(0.95), MemoryPressure::Emergency);
        assert_eq!(MemoryPressure::from_ratio(1.0), MemoryPressure::Emergency);
    }

    #[test]
    fn test_pressure_ordering() {
        assert!(MemoryPressure::Normal < MemoryPressure::Elevated);
        assert!(MemoryPressure::Elevated < MemoryPressure::High);
        assert!(MemoryPressure::High < MemoryPressure::Critical);
        assert!(MemoryPressure::Critical < MemoryPressure::Emergency);
    }

    #[test]
    fn test_pressure_display() {
        assert_eq!(MemoryPressure::Normal.to_string(), "normal");
        assert_eq!(MemoryPressure::Emergency.to_string(), "emergency");
    }

    #[test]
    fn test_pressure_roundtrip() {
        for v in 0u8..=4 {
            let p = MemoryPressure::from(v);
            assert_eq!(p as u8, v);
        }
    }

    // ---- DegradationTier ----

    #[test]
    fn test_tier_full() {
        let tier = DegradationTier::from_state(MemoryPressure::Normal, false, false);
        assert_eq!(tier, DegradationTier::Full);
        assert!(tier.should_run_stage("embed"));
        assert!(tier.should_run_stage("graph"));
    }

    #[test]
    fn test_tier_reduced_on_ollama_open() {
        let tier = DegradationTier::from_state(MemoryPressure::Normal, false, true);
        assert_eq!(tier, DegradationTier::Reduced);
        assert!(!tier.should_run_stage("embed"));
        assert!(tier.should_run_stage("graph"));
    }

    #[test]
    fn test_tier_reduced_on_high_memory() {
        let tier = DegradationTier::from_state(MemoryPressure::High, false, false);
        assert_eq!(tier, DegradationTier::Reduced);
    }

    #[test]
    fn test_tier_minimal_both_open() {
        let tier = DegradationTier::from_state(MemoryPressure::Normal, true, true);
        assert_eq!(tier, DegradationTier::Minimal);
        assert!(!tier.should_run_stage("graph"));
        assert!(!tier.should_run_stage("embed"));
        assert!(tier.should_run_stage("parse"));
    }

    #[test]
    fn test_tier_minimal_on_critical() {
        let tier = DegradationTier::from_state(MemoryPressure::Critical, false, false);
        assert_eq!(tier, DegradationTier::Minimal);
    }

    #[test]
    fn test_tier_emergency() {
        let tier = DegradationTier::from_state(MemoryPressure::Emergency, false, false);
        assert_eq!(tier, DegradationTier::Emergency);
        assert!(tier.should_run_stage("expand"));
        assert!(tier.should_run_stage("parse"));
        assert!(!tier.should_run_stage("typecheck"));
        assert!(!tier.should_run_stage("extract"));
    }

    #[test]
    fn test_tier_active_stages() {
        assert_eq!(DegradationTier::Full.active_stages().len(), 6);
        assert_eq!(DegradationTier::Reduced.active_stages().len(), 5);
        assert_eq!(DegradationTier::Minimal.active_stages().len(), 4);
        assert_eq!(DegradationTier::Emergency.active_stages().len(), 2);
    }

    // ---- SpillStore ----

    #[test]
    fn test_spill_store_should_spill() {
        let store = SpillStore {
            spill_dir: PathBuf::from("/tmp/test"),
            spill_files: Vec::new(),
            in_memory_count: 0,
            spill_threshold: 1000,
        };

        assert!(!store.should_spill(500, MemoryPressure::Normal));
        assert!(store.should_spill(1001, MemoryPressure::Normal));
        assert!(store.should_spill(500, MemoryPressure::Elevated));
        assert!(store.should_spill(500, MemoryPressure::High));
    }

    #[test]
    fn test_spill_roundtrip() {
        let mut store = SpillStore::with_defaults().unwrap();

        let items = vec![
            ParsedItemInfo {
                fqn: "crate::foo".to_string(),
                item_type: "function".to_string(),
                name: "foo".to_string(),
                visibility: "public".to_string(),
                signature: "fn foo()".to_string(),
                generic_params: vec![],
                where_clauses: vec![],
                attributes: vec![],
                doc_comment: String::new(),
                start_line: 1,
                end_line: 5,
                body_source: "{ }".to_string(),
                generated_by: None,
                source_file_path: None,
            },
            ParsedItemInfo {
                fqn: "crate::bar".to_string(),
                item_type: "struct".to_string(),
                name: "bar".to_string(),
                visibility: "public".to_string(),
                signature: "struct Bar".to_string(),
                generic_params: vec![],
                where_clauses: vec![],
                attributes: vec![],
                doc_comment: "A bar".to_string(),
                start_line: 10,
                end_line: 15,
                body_source: "{ x: i32 }".to_string(),
                generated_by: Some("derive(Debug)".to_string()),
                source_file_path: None,
            },
        ];

        let spilled = store.spill(&items).unwrap();
        assert_eq!(spilled, 2);
        assert_eq!(store.spill_file_count(), 1);

        let batches = store.drain().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(batches[0][0].fqn, "crate::foo");
        assert_eq!(batches[0][1].fqn, "crate::bar");
        assert_eq!(
            batches[0][1].generated_by,
            Some("derive(Debug)".to_string())
        );

        store.cleanup().unwrap();
    }

    #[test]
    fn test_spill_empty() {
        let mut store = SpillStore::with_defaults().unwrap();
        let spilled = store.spill(&[]).unwrap();
        assert_eq!(spilled, 0);
        assert_eq!(store.spill_file_count(), 0);
        store.cleanup().unwrap();
    }

    #[test]
    fn test_spill_multiple_files() {
        let mut store = SpillStore::with_defaults().unwrap();
        let item = ParsedItemInfo {
            fqn: "crate::a".to_string(),
            item_type: "function".to_string(),
            name: "a".to_string(),
            visibility: "public".to_string(),
            signature: "fn a()".to_string(),
            generic_params: vec![],
            where_clauses: vec![],
            attributes: vec![],
            doc_comment: String::new(),
            start_line: 1,
            end_line: 1,
            body_source: String::new(),
            generated_by: None,
            source_file_path: None,
        };

        store.spill(std::slice::from_ref(&item)).unwrap();
        store.spill(std::slice::from_ref(&item)).unwrap();
        store.spill(&[item]).unwrap();
        assert_eq!(store.spill_file_count(), 3);

        let mut total = 0;
        store
            .drain_streaming(|batch| {
                total += batch.len();
                Ok(())
            })
            .unwrap();
        assert_eq!(total, 3);

        store.cleanup().unwrap();
    }

    // ---- MemoryWatchdog ----

    #[tokio::test]
    async fn test_watchdog_starts_normal() {
        let wd = MemoryWatchdog::spawn();
        // On a normal dev machine, initial pressure should be Normal or Elevated
        let p = wd.current_pressure();
        assert!(p <= MemoryPressure::High);
    }

    #[tokio::test]
    async fn test_watchdog_subscribe() {
        let wd = MemoryWatchdog::spawn();
        let mut rx = wd.subscribe();
        // Should be able to read the current value
        let _val = *rx.borrow_and_update();
    }
}
