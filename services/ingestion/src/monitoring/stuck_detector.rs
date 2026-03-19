//! Stuck detection for ingestion pipeline stages.
//!
//! Each stage calls [`StuckDetector::heartbeat`] on every item processed.
//! A background watchdog task polls heartbeat freshness every 2 seconds and
//! escalates through Warning → Diagnostic → CircuitBreak when a stage
//! exceeds its per-stage timeout without progress.

use crate::pipeline::STAGE_NAMES;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Number of pipeline stages tracked by the detector.
pub const NUM_STAGES: usize = 6;

/// Escalation level for a stuck stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EscalationLevel {
    /// Stage exceeded its threshold once — log a warning.
    Warning,
    /// Stage exceeded 2× its threshold — emit diagnostics.
    Diagnostic,
    /// Stage exceeded 3× its threshold — request circuit break.
    CircuitBreak,
}

impl std::fmt::Display for EscalationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warning => write!(f, "WARNING"),
            Self::Diagnostic => write!(f, "DIAGNOSTIC"),
            Self::CircuitBreak => write!(f, "CIRCUIT_BREAK"),
        }
    }
}

/// Alert emitted when a stage appears stuck.
#[derive(Debug, Clone)]
pub struct StuckAlert {
    /// Index into [`STAGE_NAMES`].
    pub stage_index: usize,
    /// Human-readable stage name.
    pub stage_name: &'static str,
    /// How long since the last heartbeat.
    pub stale_duration: Duration,
    /// Current escalation level.
    pub level: EscalationLevel,
}

impl std::fmt::Display for StuckAlert {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] stage '{}' stuck for {:.1}s",
            self.level,
            self.stage_name,
            self.stale_duration.as_secs_f64(),
        )
    }
}

/// Per-stage heartbeat timestamps stored as milliseconds since an
/// arbitrary epoch ([`Instant`] is not `AtomicU64`-friendly, so we use
/// [`coarsetime`]-style offsets from detector creation).
///
/// A value of `0` means the stage has not started yet and should not be
/// considered stale.
pub struct StuckDetector {
    /// Milliseconds-since-creation of the last heartbeat per stage.
    heartbeats: Arc<[AtomicU64; NUM_STAGES]>,
    /// Per-stage staleness thresholds.
    thresholds: [Duration; NUM_STAGES],
    /// Monotonic reference point (`Instant` at construction time).
    epoch: std::time::Instant,
}

impl StuckDetector {
    /// Create a detector with the default per-stage thresholds.
    pub fn new() -> Self {
        Self::with_thresholds([
            Duration::from_secs(120), // expand
            Duration::from_secs(30),  // parse
            Duration::from_secs(30),  // typecheck
            Duration::from_secs(60),  // extract
            Duration::from_secs(60),  // graph
            Duration::from_secs(90),  // embed
        ])
    }

    /// Create a detector with custom per-stage thresholds.
    pub fn with_thresholds(thresholds: [Duration; NUM_STAGES]) -> Self {
        Self {
            heartbeats: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            thresholds,
            epoch: std::time::Instant::now(),
        }
    }

    /// Record progress for `stage_index` (0–5). Call this on every item
    /// processed so the watchdog knows the stage is alive.
    pub fn heartbeat(&self, stage_index: usize) {
        debug_assert!(stage_index < NUM_STAGES, "stage_index out of range");
        if stage_index < NUM_STAGES {
            let ms = self.epoch.elapsed().as_millis() as u64;
            self.heartbeats[stage_index].store(ms, Ordering::Relaxed);
        }
    }

    /// Return a cheaply-cloneable handle that stages can use to send
    /// heartbeats without holding a reference to the full detector.
    pub fn handle(&self) -> StuckDetectorHandle {
        StuckDetectorHandle {
            heartbeats: Arc::clone(&self.heartbeats),
            epoch: self.epoch,
        }
    }

    /// Spawn a background tokio task that checks heartbeat freshness
    /// every `poll_interval` and sends [`StuckAlert`]s on the returned
    /// channel.
    ///
    /// The task runs until the returned [`mpsc::Receiver`] is dropped or
    /// the provided `cancel` token is cancelled.
    pub fn watchdog_loop(
        &self,
        poll_interval: Duration,
        cancel: tokio_util::sync::CancellationToken,
    ) -> mpsc::Receiver<StuckAlert> {
        let (tx, rx) = mpsc::channel::<StuckAlert>(64);
        let heartbeats = Arc::clone(&self.heartbeats);
        let thresholds = self.thresholds;
        let epoch = self.epoch;

        tokio::spawn(async move {
            // Track consecutive stale ticks per stage for escalation.
            let mut stale_ticks: [u32; NUM_STAGES] = [0; NUM_STAGES];

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        debug!("stuck detector watchdog cancelled");
                        break;
                    }
                    _ = tokio::time::sleep(poll_interval) => {}
                }

                let now_ms = epoch.elapsed().as_millis() as u64;

                for i in 0..NUM_STAGES {
                    let last_ms = heartbeats[i].load(Ordering::Relaxed);

                    // Stage not started yet — skip.
                    if last_ms == 0 {
                        stale_ticks[i] = 0;
                        continue;
                    }

                    let stale = Duration::from_millis(now_ms.saturating_sub(last_ms));

                    if stale > thresholds[i] {
                        stale_ticks[i] = stale_ticks[i].saturating_add(1);

                        let level = match stale_ticks[i] {
                            1 => EscalationLevel::Warning,
                            2..=3 => EscalationLevel::Diagnostic,
                            _ => EscalationLevel::CircuitBreak,
                        };

                        let alert = StuckAlert {
                            stage_index: i,
                            stage_name: STAGE_NAMES.get(i).copied().unwrap_or("unknown"),
                            stale_duration: stale,
                            level: level.clone(),
                        };

                        match &level {
                            EscalationLevel::Warning => {
                                warn!("{alert}");
                            }
                            EscalationLevel::Diagnostic => {
                                warn!("{alert} — collecting diagnostics");
                            }
                            EscalationLevel::CircuitBreak => {
                                error!("{alert} — requesting circuit break");
                            }
                        }

                        // Best-effort send; if the receiver is gone we stop.
                        if tx.send(alert).await.is_err() {
                            debug!("stuck detector alert receiver dropped, exiting watchdog");
                            return;
                        }
                    } else {
                        // Stage made progress — reset escalation.
                        stale_ticks[i] = 0;
                    }
                }
            }
        });

        rx
    }

    /// Convenience wrapper that uses the default 2-second poll interval.
    pub fn start_watchdog(
        &self,
        cancel: tokio_util::sync::CancellationToken,
    ) -> mpsc::Receiver<StuckAlert> {
        self.watchdog_loop(Duration::from_secs(2), cancel)
    }
}

impl Default for StuckDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Lightweight, cloneable handle for sending heartbeats from stages.
#[derive(Clone)]
pub struct StuckDetectorHandle {
    heartbeats: Arc<[AtomicU64; NUM_STAGES]>,
    epoch: std::time::Instant,
}

impl StuckDetectorHandle {
    /// Record progress for `stage_index`.
    pub fn heartbeat(&self, stage_index: usize) {
        debug_assert!(stage_index < NUM_STAGES, "stage_index out of range");
        if stage_index < NUM_STAGES {
            let ms = self.epoch.elapsed().as_millis() as u64;
            self.heartbeats[stage_index].store(ms, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn heartbeat_prevents_alert() {
        let detector = StuckDetector::with_thresholds(
            std::array::from_fn(|_| Duration::from_millis(100)),
        );

        // Start watchdog with fast polling.
        let cancel = CancellationToken::new();
        let mut rx = detector.watchdog_loop(Duration::from_millis(50), cancel.clone());

        // Heartbeat all stages continuously.
        let handle = detector.handle();
        let ticker = tokio::spawn(async move {
            for _ in 0..20 {
                for i in 0..NUM_STAGES {
                    handle.heartbeat(i);
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });

        // Give the watchdog time to run a few cycles.
        tokio::time::sleep(Duration::from_millis(250)).await;
        cancel.cancel();
        ticker.await.unwrap();

        // No alerts should have been emitted.
        assert!(rx.try_recv().is_err(), "expected no stuck alerts");
    }

    #[tokio::test]
    async fn stale_stage_triggers_alert() {
        let mut thresholds = [Duration::from_secs(60); NUM_STAGES];
        // Make the parse stage (index 1) have a tiny threshold.
        thresholds[1] = Duration::from_millis(50);

        let detector = StuckDetector::with_thresholds(thresholds);

        // Heartbeat parse once to mark it as started, then stop.
        detector.heartbeat(1);

        let cancel = CancellationToken::new();
        let mut rx = detector.watchdog_loop(Duration::from_millis(30), cancel.clone());

        // Wait long enough for the threshold to expire.
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();

        let alert = rx.try_recv().expect("expected a stuck alert");
        assert_eq!(alert.stage_index, 1);
        assert_eq!(alert.stage_name, "parse");
        assert_eq!(alert.level, EscalationLevel::Warning);
    }

    #[tokio::test]
    async fn escalation_progresses() {
        let mut thresholds = [Duration::from_secs(60); NUM_STAGES];
        thresholds[0] = Duration::from_millis(30);

        let detector = StuckDetector::with_thresholds(thresholds);
        detector.heartbeat(0); // mark expand as started

        let cancel = CancellationToken::new();
        let mut rx = detector.watchdog_loop(Duration::from_millis(20), cancel.clone());

        // Collect alerts for enough ticks to reach CircuitBreak (4+ ticks).
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();

        let mut levels = Vec::new();
        while let Ok(alert) = rx.try_recv() {
            levels.push(alert.level.clone());
        }

        assert!(!levels.is_empty(), "expected escalation alerts");
        assert_eq!(levels[0], EscalationLevel::Warning);
        // Should eventually escalate beyond Warning.
        assert!(
            levels.iter().any(|l| *l != EscalationLevel::Warning),
            "expected escalation beyond Warning, got: {levels:?}",
        );
    }

    #[test]
    fn handle_heartbeat_is_clone_safe() {
        let detector = StuckDetector::new();
        let h1 = detector.handle();
        let h2 = h1.clone();
        h1.heartbeat(0);
        h2.heartbeat(1);
        // No panic — just verifying the clone + heartbeat path compiles.
    }

    #[test]
    fn default_thresholds() {
        let detector = StuckDetector::new();
        assert_eq!(detector.thresholds[0], Duration::from_secs(120)); // expand
        assert_eq!(detector.thresholds[1], Duration::from_secs(30));  // parse
        assert_eq!(detector.thresholds[2], Duration::from_secs(30));  // typecheck
        assert_eq!(detector.thresholds[3], Duration::from_secs(60));  // extract
        assert_eq!(detector.thresholds[4], Duration::from_secs(60));  // graph
        assert_eq!(detector.thresholds[5], Duration::from_secs(90));  // embed
    }
}
