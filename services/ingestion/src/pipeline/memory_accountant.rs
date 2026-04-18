//! Memory-bounded pipeline accounting
//!
//! Provides `MemoryAccountant` to enforce per-stage memory quotas and a global
//! budget, plus `MemoryGuard` (RAII) that releases reserved bytes on drop.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, warn};

/// Default total memory budget: 16 GB
const DEFAULT_TOTAL_BUDGET: u64 = 16 * 1024 * 1024 * 1024;

/// Pre-flight file size limit: skip files larger than 10 MB before expansion
pub const MAX_FILE_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Channel capacities between pipeline stages
pub mod channel_capacity {
    pub const DISCOVER_TO_EXPAND: usize = 256;
    pub const EXPAND_TO_PARSE: usize = 64;
    pub const PARSE_TO_GRAPH: usize = 128;
    pub const GRAPH_TO_EMBED: usize = 256;
}

/// Per-stage default memory quotas
fn default_stage_quotas() -> HashMap<String, u64> {
    let mut quotas = HashMap::new();
    quotas.insert("discover".to_string(), 512 * 1024 * 1024); // 512 MB
    quotas.insert("expand".to_string(), 2 * 1024 * 1024 * 1024); // 2 GB
    quotas.insert("parse".to_string(), 3 * 1024 * 1024 * 1024); // 3 GB
    quotas.insert("typecheck".to_string(), 1024 * 1024 * 1024); // 1 GB
    quotas.insert("graph".to_string(), 2 * 1024 * 1024 * 1024); // 2 GB
    quotas.insert("embed".to_string(), 1536 * 1024 * 1024); // 1.5 GB
    quotas
}

/// Shared inner state protected by a mutex.
#[derive(Debug)]
struct AccountantInner {
    total_budget: u64,
    total_reserved: u64,
    stage_quotas: HashMap<String, u64>,
    stage_reserved: HashMap<String, u64>,
}

/// Tracks memory reservations per stage and globally.
///
/// `reserve()` is async and will wait (via `Notify`) until enough budget is
/// available. `MemoryGuard` releases on drop so callers never forget.
#[derive(Clone)]
pub struct MemoryAccountant {
    inner: Arc<Mutex<AccountantInner>>,
    notify: Arc<Notify>,
}

impl MemoryAccountant {
    /// Create with the default 16 GB budget and standard per-stage quotas.
    pub fn new() -> Self {
        Self::with_budget(DEFAULT_TOTAL_BUDGET, default_stage_quotas())
    }

    /// Create with a custom total budget and per-stage quotas.
    pub fn with_budget(total_budget: u64, stage_quotas: HashMap<String, u64>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(AccountantInner {
                total_budget,
                total_reserved: 0,
                stage_quotas,
                stage_reserved: HashMap::new(),
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Try to reserve `bytes` for `stage`. Returns immediately if space is
    /// available, otherwise awaits until a `MemoryGuard` drops and frees
    /// enough room.
    ///
    /// Returns a `MemoryGuard` whose `Drop` releases the reservation.
    pub async fn reserve(&self, stage: &str, bytes: u64) -> MemoryGuard {
        loop {
            {
                let mut inner = self.inner.lock().await;

                let stage_used = *inner.stage_reserved.get(stage).unwrap_or(&0);
                let stage_quota = *inner.stage_quotas.get(stage).unwrap_or(&u64::MAX);

                let within_stage = stage_used + bytes <= stage_quota;
                let within_global = inner.total_reserved + bytes <= inner.total_budget;

                if within_stage && within_global {
                    inner.total_reserved += bytes;
                    *inner.stage_reserved.entry(stage.to_string()).or_insert(0) += bytes;
                    debug!(
                        "memory_accountant: reserved {} bytes for {} (stage {}/{}, global {}/{})",
                        bytes,
                        stage,
                        stage_used + bytes,
                        stage_quota,
                        inner.total_reserved,
                        inner.total_budget,
                    );
                    return MemoryGuard {
                        accountant: self.clone(),
                        stage: stage.to_string(),
                        bytes,
                    };
                }

                // Log why we're waiting
                if !within_stage {
                    debug!(
                        "memory_accountant: stage {} over quota ({} + {} > {}), waiting",
                        stage, stage_used, bytes, stage_quota
                    );
                } else {
                    debug!(
                        "memory_accountant: global budget full ({} + {} > {}), waiting",
                        inner.total_reserved, bytes, inner.total_budget
                    );
                }
            }
            // Wait until someone drops a guard
            self.notify.notified().await;
        }
    }

    /// Release `bytes` from `stage`. Called automatically by `MemoryGuard::drop`.
    async fn release(&self, stage: &str, bytes: u64) {
        let mut inner = self.inner.lock().await;
        inner.total_reserved = inner.total_reserved.saturating_sub(bytes);
        if let Some(v) = inner.stage_reserved.get_mut(stage) {
            *v = v.saturating_sub(bytes);
        }
        debug!(
            "memory_accountant: released {} bytes for {} (global now {})",
            bytes, stage, inner.total_reserved,
        );
        drop(inner);
        // Wake any tasks waiting in reserve()
        self.notify.notify_waiters();
    }

    /// Current total reserved bytes (for diagnostics).
    pub async fn total_reserved(&self) -> u64 {
        self.inner.lock().await.total_reserved
    }

    /// Current reserved bytes for a given stage (for diagnostics).
    pub async fn stage_reserved(&self, stage: &str) -> u64 {
        *self
            .inner
            .lock()
            .await
            .stage_reserved
            .get(stage)
            .unwrap_or(&0)
    }

    /// Check if a file should be skipped based on its size (pre-flight check).
    pub fn should_skip_file(file_size: u64) -> bool {
        if file_size > MAX_FILE_SIZE_BYTES {
            warn!(
                "Skipping file: {} bytes exceeds {} byte pre-flight limit",
                file_size, MAX_FILE_SIZE_BYTES
            );
            true
        } else {
            false
        }
    }
}

impl Default for MemoryAccountant {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard that releases its memory reservation when dropped.
///
/// Created by [`MemoryAccountant::reserve`]. Intentionally does NOT implement
/// `Clone` — each reservation is unique.
pub struct MemoryGuard {
    accountant: MemoryAccountant,
    stage: String,
    bytes: u64,
}

impl MemoryGuard {
    /// How many bytes this guard is holding.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Which stage this guard belongs to.
    pub fn stage(&self) -> &str {
        &self.stage
    }
}

impl Drop for MemoryGuard {
    fn drop(&mut self) {
        let accountant = self.accountant.clone();
        let stage = self.stage.clone();
        let bytes = self.bytes;
        // Spawn a task to do the async release.
        // In practice the Mutex is uncontended during drop so this is fast.
        tokio::spawn(async move {
            accountant.release(&stage, bytes).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Message types flowing through bounded channels
// ---------------------------------------------------------------------------

use std::path::PathBuf;

/// A discovered crate ready for expansion.
#[derive(Debug, Clone)]
pub struct DiscoveredCrate {
    pub crate_path: PathBuf,
    pub crate_name: String,
    pub source_files: Vec<PathBuf>,
    pub git_hash: Option<String>,
}

/// Expanded source for a single crate.
#[derive(Debug, Clone)]
pub struct ExpandedCrate {
    pub crate_path: PathBuf,
    pub crate_name: String,
    pub source_files: Vec<super::SourceFileInfo>,
    pub expanded_source: Option<String>,
}

/// A batch of parsed items for one file, ready for graph/extract.
#[derive(Debug, Clone)]
pub struct ParsedBatch {
    pub file_path: PathBuf,
    pub crate_name: String,
    pub items: Vec<super::ParsedItemInfo>,
}

/// A graph result ready for embedding.
#[derive(Debug, Clone)]
pub struct GraphResult {
    pub items: Vec<super::ParsedItemInfo>,
    pub crate_name: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reserve_and_drop_releases() {
        let accountant =
            MemoryAccountant::with_budget(1024, [("test".to_string(), 1024)].into_iter().collect());

        {
            let guard = accountant.reserve("test", 512).await;
            assert_eq!(guard.bytes(), 512);
            assert_eq!(accountant.total_reserved().await, 512);
            assert_eq!(accountant.stage_reserved("test").await, 512);
        }

        // Give the spawned release task a moment
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        assert_eq!(accountant.total_reserved().await, 0);
    }

    #[tokio::test]
    async fn test_stage_quota_blocks_then_proceeds() {
        let accountant =
            MemoryAccountant::with_budget(2048, [("small".to_string(), 100)].into_iter().collect());

        let g1 = accountant.reserve("small", 80).await;
        assert_eq!(accountant.stage_reserved("small").await, 80);

        // Spawn a second reservation that will exceed the 100-byte stage quota
        let acct = accountant.clone();
        let handle = tokio::spawn(async move {
            let _g2 = acct.reserve("small", 80).await;
            acct.stage_reserved("small").await
        });

        // Let the spawned task register its notified() wait
        tokio::task::yield_now().await;

        // Drop g1 → frees 80 bytes → unblocks g2
        drop(g1);

        let reserved_after = handle.await.unwrap();
        assert_eq!(reserved_after, 80);
    }

    #[test]
    fn test_should_skip_file() {
        assert!(!MemoryAccountant::should_skip_file(1024));
        assert!(!MemoryAccountant::should_skip_file(10 * 1024 * 1024)); // exactly 10 MB
        assert!(MemoryAccountant::should_skip_file(10 * 1024 * 1024 + 1));
    }

    #[test]
    fn test_default_stage_quotas() {
        let quotas = default_stage_quotas();
        assert_eq!(quotas.len(), 6);
        assert_eq!(*quotas.get("discover").unwrap(), 512 * 1024 * 1024);
        assert_eq!(*quotas.get("expand").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(*quotas.get("parse").unwrap(), 3 * 1024 * 1024 * 1024);
        assert_eq!(*quotas.get("typecheck").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(*quotas.get("graph").unwrap(), 2 * 1024 * 1024 * 1024);
        assert_eq!(*quotas.get("embed").unwrap(), 1536 * 1024 * 1024);
    }

    #[test]
    fn test_channel_capacities() {
        assert_eq!(channel_capacity::DISCOVER_TO_EXPAND, 256);
        assert_eq!(channel_capacity::EXPAND_TO_PARSE, 64);
        assert_eq!(channel_capacity::PARSE_TO_GRAPH, 128);
        assert_eq!(channel_capacity::GRAPH_TO_EMBED, 256);
    }
}
