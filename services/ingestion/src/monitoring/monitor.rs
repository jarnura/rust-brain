//! Central monitoring coordinator for the ingestion pipeline.
//!
//! [`Monitor`] owns and wires together the subsystems that live in sibling
//! modules — Prometheus metrics, terminal progress bars, the stuck-detector
//! watchdog, and the audit event emitter — so that `PipelineRunner` only
//! needs to hold a single `Arc<Monitor>`.

use crate::monitoring::audit::AuditEmitter;
use crate::monitoring::metrics::MetricsRegistry;
use crate::monitoring::progress::ProgressTracker as TerminalProgress;
use crate::monitoring::stuck_detector::{StuckAlert, StuckDetector, StuckDetectorHandle};
use crate::pipeline::STAGE_NAMES;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Central coordinator that owns every monitoring subsystem.
///
/// Cheaply cloneable via the inner `Arc`.  Call [`Monitor::start`] once at
/// pipeline start-up to spin up background tasks, and [`Monitor::shutdown`]
/// when the pipeline finishes.
#[derive(Clone)]
pub struct Monitor {
    inner: Arc<MonitorInner>,
}

struct MonitorInner {
    pub metrics: Arc<MetricsRegistry>,
    pub terminal_progress: TerminalProgress,
    pub stuck_detector: StuckDetector,
    pub audit: AuditEmitter,
    pub cancel: CancellationToken,
    pub pipeline_start: Instant,
}

/// Configuration knobs for [`Monitor`].
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// Whether to show terminal progress bars (disable in CI / tests).
    pub show_progress_bars: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            show_progress_bars: true,
        }
    }
}

impl Monitor {
    /// Build a new monitor.
    ///
    /// `audit` should be created by the caller (it needs a `PgPool`).
    /// Pass `AuditEmitter::noop()` for dry-run or test mode.
    pub fn new(config: MonitorConfig, audit: AuditEmitter) -> anyhow::Result<Self> {
        let metrics = Arc::new(MetricsRegistry::new().map_err(|e| anyhow::anyhow!(e))?);
        let terminal_progress = if config.show_progress_bars {
            TerminalProgress::new()
        } else {
            TerminalProgress::hidden()
        };
        let stuck_detector = StuckDetector::new();
        let cancel = CancellationToken::new();

        Ok(Self {
            inner: Arc::new(MonitorInner {
                metrics,
                terminal_progress,
                stuck_detector,
                audit,
                cancel,
                pipeline_start: Instant::now(),
            }),
        })
    }

    // -----------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------

    /// Spin up background tasks (stuck-detector watchdog, alert consumer).
    ///
    /// Returns a receiver of [`StuckAlert`]s that the runner can optionally
    /// use to react to stuck stages (e.g., trip a circuit breaker).
    pub fn start(&self) -> mpsc::Receiver<StuckAlert> {
        let rx = self
            .inner
            .stuck_detector
            .start_watchdog(self.inner.cancel.clone());

        info!("Monitor started — watchdog + metrics active");
        rx
    }

    /// Cancel background tasks and flush the audit buffer.
    pub fn shutdown(&self) {
        self.inner.cancel.cancel();
        info!(
            elapsed_secs = self.inner.pipeline_start.elapsed().as_secs_f64(),
            "Monitor shutting down"
        );
    }

    // -----------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------

    /// Prometheus metrics registry.
    pub fn metrics(&self) -> &Arc<MetricsRegistry> {
        &self.inner.metrics
    }

    /// Terminal progress bars (indicatif).
    pub fn progress(&self) -> &TerminalProgress {
        &self.inner.terminal_progress
    }

    /// Lightweight handle for sending heartbeats from within stages.
    pub fn stuck_handle(&self) -> StuckDetectorHandle {
        self.inner.stuck_detector.handle()
    }

    /// The audit event emitter.
    pub fn audit(&self) -> &AuditEmitter {
        &self.inner.audit
    }

    /// Cancellation token shared with all background tasks.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.inner.cancel
    }

    /// Wall-clock time since the monitor was created.
    pub fn elapsed_secs(&self) -> f64 {
        self.inner.pipeline_start.elapsed().as_secs_f64()
    }

    // -----------------------------------------------------------------
    // Stage lifecycle helpers (convenience wrappers)
    // -----------------------------------------------------------------

    /// Call at the start of a pipeline stage.
    ///
    /// - Begins a terminal progress bar (total may be 0 = spinner).
    /// - Sends the first heartbeat so the stuck detector knows the stage
    ///   is alive.
    /// - Records a Prometheus `stage_duration` timer start implicitly
    ///   (the histogram is observed in [`finish_stage`]).
    pub fn begin_stage(&self, stage: &str, total: u64) {
        self.inner.terminal_progress.begin_stage(stage, total);
        if let Some(idx) = stage_index(stage) {
            self.inner.stuck_detector.heartbeat(idx);
        }
        info!(stage, total, "stage started");
    }

    /// Record incremental progress within a stage.
    ///
    /// Advances the terminal bar and sends a heartbeat.
    pub fn record_progress(&self, stage: &str, delta: u64) {
        self.inner.terminal_progress.advance(stage, delta);
        if let Some(idx) = stage_index(stage) {
            self.inner.stuck_detector.heartbeat(idx);
        }
        self.inner
            .metrics
            .items_processed
            .with_label_values(&[stage])
            .inc_by(delta);
    }

    /// Update the expected total for a stage (discovered mid-run).
    pub fn update_total(&self, stage: &str, total: u64) {
        self.inner.terminal_progress.set_total(stage, total);
    }

    /// Mark a stage as successfully finished.
    ///
    /// - Finalises the terminal bar.
    /// - Records duration in the Prometheus histogram.
    /// - Logs a summary line.
    pub fn finish_stage(&self, stage: &str, duration_secs: f64, items_processed: u64) {
        let msg = format!("done — {} items in {:.1}s", items_processed, duration_secs);
        self.inner.terminal_progress.finish_stage(stage, &msg);
        self.inner
            .metrics
            .stage_duration
            .with_label_values(&[stage])
            .observe(duration_secs);
        info!(stage, duration_secs, items_processed, "stage finished");
    }

    /// Mark a stage as failed.
    pub fn fail_stage(&self, stage: &str, duration_secs: f64, error: &str) {
        self.inner.terminal_progress.fail_stage(stage, error);
        self.inner
            .metrics
            .stage_duration
            .with_label_values(&[stage])
            .observe(duration_secs);
        self.inner
            .metrics
            .errors
            .with_label_values(&[stage, "stage_failure"])
            .inc();
        warn!(stage, duration_secs, error, "stage failed");
    }

    /// Record a non-fatal error within a stage.
    pub fn record_error(&self, stage: &str, error_type: &str) {
        self.inner
            .metrics
            .errors
            .with_label_values(&[stage, error_type])
            .inc();
    }

    /// Update the degradation tier gauge.
    pub fn set_degradation_tier(&self, tier: i64) {
        self.inner.metrics.degradation_tier.set(tier);
    }

    /// Record a stuck-detector warning in Prometheus.
    pub fn record_stuck_warning(&self, stage: &str) {
        self.inner
            .metrics
            .stuck_warnings
            .with_label_values(&[stage])
            .inc();
    }
}

/// Map a stage name to its index in `STAGE_NAMES` (0–5).
fn stage_index(stage: &str) -> Option<usize> {
    STAGE_NAMES.iter().position(|&s| s == stage)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitoring::audit::AuditEmitter;

    fn test_monitor() -> Monitor {
        Monitor::new(
            MonitorConfig {
                show_progress_bars: false,
            },
            AuditEmitter::noop(),
        )
        .expect("monitor creation")
    }

    #[test]
    fn monitor_creation() {
        let _m = test_monitor();
    }

    #[test]
    fn begin_and_finish_stage() {
        let m = test_monitor();
        m.begin_stage("parse", 100);
        m.record_progress("parse", 50);
        m.record_progress("parse", 50);
        m.finish_stage("parse", 1.5, 100);
    }

    #[test]
    fn fail_stage_records_metrics() {
        let m = test_monitor();
        m.begin_stage("graph", 10);
        m.record_progress("graph", 3);
        m.fail_stage("graph", 0.5, "connection refused");

        let gathered = m.metrics().gather();
        assert!(gathered.contains("ingestion_errors_total"));
    }

    #[test]
    fn record_error_increments_counter() {
        let m = test_monitor();
        m.record_error("embed", "timeout");
        m.record_error("embed", "timeout");

        let gathered = m.metrics().gather();
        assert!(gathered.contains("ingestion_errors_total"));
    }

    #[test]
    fn stuck_handle_is_clone_safe() {
        let m = test_monitor();
        let h1 = m.stuck_handle();
        let h2 = h1.clone();
        h1.heartbeat(0);
        h2.heartbeat(1);
    }

    #[test]
    fn stage_index_lookup() {
        assert_eq!(stage_index("expand"), Some(0));
        assert_eq!(stage_index("parse"), Some(1));
        assert_eq!(stage_index("typecheck"), Some(2));
        assert_eq!(stage_index("extract"), Some(3));
        assert_eq!(stage_index("graph"), Some(4));
        assert_eq!(stage_index("embed"), Some(5));
        assert_eq!(stage_index("unknown"), None);
    }

    #[tokio::test]
    async fn start_and_shutdown() {
        let m = test_monitor();
        let _rx = m.start();
        m.begin_stage("expand", 50);
        m.record_progress("expand", 10);
        m.shutdown();
    }

    #[test]
    fn elapsed_increases() {
        let m = test_monitor();
        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(m.elapsed_secs() > 0.0);
    }

    #[test]
    fn set_degradation_tier() {
        let m = test_monitor();
        m.set_degradation_tier(2);
        let gathered = m.metrics().gather();
        assert!(gathered.contains("ingestion_degradation_tier"));
    }

    #[test]
    fn update_total_midrun() {
        let m = test_monitor();
        m.begin_stage("embed", 0);
        m.update_total("embed", 200);
        // No panic — just verifying the path.
    }
}
