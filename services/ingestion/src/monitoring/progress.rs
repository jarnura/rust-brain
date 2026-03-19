//! Real-time progress tracking with ETA for pipeline stages.
//!
//! Wraps `indicatif` progress bars to give operators a live view of
//! how many items each stage has processed and how long remains.

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-stage progress snapshot returned by [`ProgressTracker::snapshot`].
#[derive(Debug, Clone)]
pub struct StageProgress {
    pub stage: String,
    pub processed: u64,
    pub total: u64,
    pub elapsed_secs: f64,
    pub eta_secs: Option<f64>,
}

/// Tracks real-time progress across all pipeline stages.
///
/// Thread-safe: the inner state is behind an `Arc<Mutex<_>>` so that
/// stage futures running on different Tokio tasks can call `advance`
/// concurrently.
#[derive(Clone)]
pub struct ProgressTracker {
    multi: Arc<MultiProgress>,
    inner: Arc<Mutex<ProgressInner>>,
}

struct ProgressInner {
    bars: HashMap<String, ProgressBar>,
    start_times: HashMap<String, Instant>,
}

impl ProgressTracker {
    /// Create a new tracker.  No bars are shown until [`begin_stage`] is called.
    pub fn new() -> Self {
        Self {
            multi: Arc::new(MultiProgress::new()),
            inner: Arc::new(Mutex::new(ProgressInner {
                bars: HashMap::new(),
                start_times: HashMap::new(),
            })),
        }
    }

    /// Create a tracker that suppresses all terminal output (for tests / CI).
    pub fn hidden() -> Self {
        Self {
            multi: Arc::new(MultiProgress::with_draw_target(
                indicatif::ProgressDrawTarget::hidden(),
            )),
            inner: Arc::new(Mutex::new(ProgressInner {
                bars: HashMap::new(),
                start_times: HashMap::new(),
            })),
        }
    }

    /// Start tracking a stage with a known total item count.
    ///
    /// If `total` is 0 the bar switches to a spinner (unknown length).
    pub fn begin_stage(&self, stage: &str, total: u64) {
        let pb = if total == 0 {
            let pb = self.multi.add(ProgressBar::new_spinner());
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} {prefix}: {msg} [{elapsed_precise}]",
                )
                .unwrap(),
            );
            pb
        } else {
            let pb = self.multi.add(ProgressBar::new(total));
            pb.set_style(
                ProgressStyle::with_template(
                    "{prefix}: [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) ETA {eta} [{elapsed_precise}]",
                )
                .unwrap()
                .progress_chars("=> "),
            );
            pb
        };
        pb.set_prefix(stage.to_string());
        pb.set_message("running");

        let mut inner = self.inner.lock().unwrap();
        inner.bars.insert(stage.to_string(), pb);
        inner.start_times.insert(stage.to_string(), Instant::now());
    }

    /// Advance a stage's progress by `delta` items.
    pub fn advance(&self, stage: &str, delta: u64) {
        let inner = self.inner.lock().unwrap();
        if let Some(pb) = inner.bars.get(stage) {
            pb.inc(delta);
        }
    }

    /// Set the absolute position for a stage.
    pub fn set_position(&self, stage: &str, pos: u64) {
        let inner = self.inner.lock().unwrap();
        if let Some(pb) = inner.bars.get(stage) {
            pb.set_position(pos);
        }
    }

    /// Update the total for a stage (useful when the total is discovered mid-run).
    pub fn set_total(&self, stage: &str, total: u64) {
        let inner = self.inner.lock().unwrap();
        if let Some(pb) = inner.bars.get(stage) {
            pb.set_length(total);
        }
    }

    /// Mark a stage as finished.
    pub fn finish_stage(&self, stage: &str, message: &str) {
        let inner = self.inner.lock().unwrap();
        if let Some(pb) = inner.bars.get(stage) {
            pb.set_message(message.to_string());
            pb.finish();
        }
    }

    /// Mark a stage as finished with an error.
    pub fn fail_stage(&self, stage: &str, error: &str) {
        let inner = self.inner.lock().unwrap();
        if let Some(pb) = inner.bars.get(stage) {
            pb.set_style(
                ProgressStyle::with_template("{prefix}: {msg}")
                    .unwrap(),
            );
            pb.set_message(format!("FAILED: {}", error));
            pb.abandon();
        }
    }

    /// Return a snapshot of all tracked stages.
    pub fn snapshot(&self) -> Vec<StageProgress> {
        let inner = self.inner.lock().unwrap();
        inner
            .bars
            .iter()
            .map(|(name, pb)| {
                let elapsed = inner
                    .start_times
                    .get(name)
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let processed = pb.position();
                let total = pb.length().unwrap_or(0);
                let eta = if processed > 0 && total > 0 && processed < total {
                    let rate = processed as f64 / elapsed;
                    Some((total - processed) as f64 / rate)
                } else {
                    None
                };
                StageProgress {
                    stage: name.clone(),
                    processed,
                    total,
                    elapsed_secs: elapsed,
                    eta_secs: eta,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker_lifecycle() {
        let tracker = ProgressTracker::hidden();
        tracker.begin_stage("parse", 100);
        tracker.advance("parse", 40);
        tracker.advance("parse", 10);

        let snaps = tracker.snapshot();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].processed, 50);
        assert_eq!(snaps[0].total, 100);

        tracker.finish_stage("parse", "done");
    }

    #[test]
    fn test_set_position_and_total() {
        let tracker = ProgressTracker::hidden();
        tracker.begin_stage("expand", 0);
        tracker.set_total("expand", 50);
        tracker.set_position("expand", 25);

        let snaps = tracker.snapshot();
        assert_eq!(snaps[0].processed, 25);
        assert_eq!(snaps[0].total, 50);
    }

    #[test]
    fn test_fail_stage() {
        let tracker = ProgressTracker::hidden();
        tracker.begin_stage("graph", 10);
        tracker.advance("graph", 3);
        tracker.fail_stage("graph", "connection lost");
        // Should not panic; bar is abandoned.
    }

    #[test]
    fn test_advance_unknown_stage_is_noop() {
        let tracker = ProgressTracker::hidden();
        tracker.advance("nonexistent", 5);
        // No panic expected.
    }

    #[test]
    fn test_eta_computed_when_progress_exists() {
        let tracker = ProgressTracker::hidden();
        tracker.begin_stage("embed", 100);
        tracker.set_position("embed", 50);

        let snaps = tracker.snapshot();
        // ETA should be Some since position > 0 and < total
        assert!(snaps[0].eta_secs.is_some());
    }
}
