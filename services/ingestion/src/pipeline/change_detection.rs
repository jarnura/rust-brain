//! Change detection module for incremental ingestion pipeline
//!
//! Implements ADR-006: Incremental Ingestion via Content-Addressed Storage
//! Detects file changes by comparing SHA-256 content hashes between disk and database.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

/// Mode of ingestion - full rebuild or incremental update
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IngestionMode {
    /// Process all files regardless of changes
    Full,
    /// Only process changed/deleted files
    Incremental,
}

/// Context for incremental ingestion runs
/// Tracks file categorization from change detection
#[derive(Debug, Clone)]
pub struct IncrementalContext {
    /// Ingestion mode for this run
    pub mode: IngestionMode,
    /// Files that are new or modified (need re-processing)
    pub changed_files: HashSet<PathBuf>,
    /// Files that were in DB but removed from disk (need cleanup)
    pub deleted_files: HashSet<PathBuf>,
    /// Files unchanged since last ingestion (can skip)
    pub unchanged_files: HashSet<PathBuf>,
}

impl IncrementalContext {
    /// Create a context for full ingestion mode with empty sets
    pub fn full() -> Self {
        Self {
            mode: IngestionMode::Full,
            changed_files: HashSet::new(),
            deleted_files: HashSet::new(),
            unchanged_files: HashSet::new(),
        }
    }

    /// Create a context for incremental ingestion
    pub fn incremental(
        changed_files: HashSet<PathBuf>,
        deleted_files: HashSet<PathBuf>,
        unchanged_files: HashSet<PathBuf>,
    ) -> Self {
        Self {
            mode: IngestionMode::Incremental,
            changed_files,
            deleted_files,
            unchanged_files,
        }
    }

    /// Total number of files tracked
    pub fn total_files(&self) -> usize {
        self.changed_files.len() + self.deleted_files.len() + self.unchanged_files.len()
    }

    /// Number of files that need processing
    pub fn files_to_process(&self) -> usize {
        self.changed_files.len() + self.deleted_files.len()
    }
}

/// Detects file changes by comparing disk state with database records
#[derive(Debug)]
pub struct ChangeDetector;

impl ChangeDetector {
    /// Create a new change detector
    pub fn new() -> Self {
        Self
    }

    /// Detect changes between files on disk and stored in database
    ///
    /// # Arguments
    /// * `crate_path` - Path to the crate directory
    /// * `crate_name` - Name of the crate (for database queries)
    /// * `pool` - PostgreSQL connection pool
    ///
    /// # Returns
    /// An `IncrementalContext` categorizing all files as changed/unchanged/deleted
    pub async fn detect_changes(
        &self,
        crate_path: &Path,
        crate_name: &str,
        pool: &PgPool,
    ) -> Result<IncrementalContext> {
        debug!(
            "Starting change detection for crate '{}' at {:?}",
            crate_name, crate_path
        );

        // Step 1: Find all .rs files on disk
        let disk_files = self.discover_rs_files(crate_path).await?;
        debug!("Found {} .rs files on disk", disk_files.len());

        // Step 2: Compute SHA-256 hash for each file
        let mut disk_hashes = std::collections::HashMap::new();
        for file_path in &disk_files {
            match self.compute_file_hash(file_path).await {
                Ok(hash) => {
                    debug!("Computed hash for {:?}: {}", file_path, hash);
                    disk_hashes.insert(file_path.clone(), hash);
                }
                Err(e) => {
                    warn!("Failed to hash {:?}: {}", file_path, e);
                    // Treat unreadable files as changed to ensure they're re-processed
                    disk_hashes.insert(file_path.clone(), String::new());
                }
            }
        }

        // Step 3: Query database for existing files
        let db_records = self.fetch_db_hashes(pool, crate_name).await?;
        debug!(
            "Found {} existing files in database for crate '{}'",
            db_records.len(),
            crate_name
        );

        // Step 4: Partition files into categories
        let mut changed_files = HashSet::new();
        let mut unchanged_files = HashSet::new();
        let mut deleted_files = HashSet::new();

        // Check disk files against DB
        for (file_path, disk_hash) in &disk_hashes {
            match db_records.get(file_path) {
                None => {
                    // File not in DB - it's new
                    debug!("New file detected: {:?}", file_path);
                    changed_files.insert(file_path.clone());
                }
                Some(db_hash) => {
                    if disk_hash == db_hash {
                        // Hash matches - unchanged
                        debug!("Unchanged file: {:?}", file_path);
                        unchanged_files.insert(file_path.clone());
                    } else {
                        // Hash differs - modified
                        debug!(
                            "Modified file: {:?} (disk: {}, db: {})",
                            file_path, disk_hash, db_hash
                        );
                        changed_files.insert(file_path.clone());
                    }
                }
            }
        }

        // Find deleted files (in DB but not on disk)
        for db_file_path in db_records.keys() {
            if !disk_hashes.contains_key(db_file_path) {
                debug!("Deleted file detected: {:?}", db_file_path);
                deleted_files.insert(db_file_path.clone());
            }
        }

        // Log summary
        info!(
            "Change detection: {} changed, {} unchanged, {} deleted",
            changed_files.len(),
            unchanged_files.len(),
            deleted_files.len()
        );

        Ok(IncrementalContext::incremental(
            changed_files,
            deleted_files,
            unchanged_files,
        ))
    }

    /// Discover all .rs files under the crate path
    async fn discover_rs_files(&self, crate_path: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        let walker = WalkDir::new(crate_path).follow_links(false);

        for entry in walker.into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();

            // Skip non-file entries
            if !entry.file_type().is_file() {
                continue;
            }

            // Only include .rs files
            if path.extension().map(|ext| ext == "rs").unwrap_or(false) {
                // Get absolute path for consistent comparison with DB
                let absolute_path = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()
                        .context("Failed to get current directory")?
                        .join(path)
                };
                files.push(absolute_path);
            }
        }

        Ok(files)
    }

    /// Compute SHA-256 hash of file contents, returning hex-encoded string
    async fn compute_file_hash(&self, file_path: &Path) -> Result<String> {
        let content = tokio::fs::read_to_string(file_path)
            .await
            .with_context(|| format!("Failed to read file: {:?}", file_path))?;

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let result = hasher.finalize();

        // Encode as lowercase hex string
        Ok(format!("{:x}", result))
    }

    /// Fetch file hashes from database for a given crate
    async fn fetch_db_hashes(
        &self,
        pool: &PgPool,
        crate_name: &str,
    ) -> Result<std::collections::HashMap<PathBuf, String>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT file_path, content_hash FROM source_files WHERE crate_name = $1",
        )
        .bind(crate_name)
        .fetch_all(pool)
        .await
        .with_context(|| format!("Failed to query source_files for crate '{}'", crate_name))?;

        let mut hashes = std::collections::HashMap::new();
        for (file_path, content_hash) in rows {
            hashes.insert(PathBuf::from(file_path), content_hash);
        }

        Ok(hashes)
    }
}

impl Default for ChangeDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_incremental_context_full() {
        let ctx = IncrementalContext::full();
        assert_eq!(ctx.mode, IngestionMode::Full);
        assert!(ctx.changed_files.is_empty());
        assert!(ctx.deleted_files.is_empty());
        assert!(ctx.unchanged_files.is_empty());
        assert_eq!(ctx.total_files(), 0);
        assert_eq!(ctx.files_to_process(), 0);
    }

    #[test]
    fn test_incremental_context_counts() {
        let mut changed = HashSet::new();
        changed.insert(PathBuf::from("/a.rs"));
        changed.insert(PathBuf::from("/b.rs"));

        let mut deleted = HashSet::new();
        deleted.insert(PathBuf::from("/c.rs"));

        let mut unchanged = HashSet::new();
        unchanged.insert(PathBuf::from("/d.rs"));
        unchanged.insert(PathBuf::from("/e.rs"));
        unchanged.insert(PathBuf::from("/f.rs"));

        let ctx = IncrementalContext::incremental(changed, deleted, unchanged);

        assert_eq!(ctx.mode, IngestionMode::Incremental);
        assert_eq!(ctx.total_files(), 6);
        assert_eq!(ctx.files_to_process(), 3); // 2 changed + 1 deleted
    }

    #[test]
    fn test_change_detector_new() {
        let detector = ChangeDetector::new();
        // Just verify it constructs without panic
        let _ = format!("{:?}", detector);
    }

    #[test]
    fn test_change_detector_default() {
        let detector: ChangeDetector = Default::default();
        let _ = format!("{:?}", detector);
    }

    #[test]
    fn test_ingestion_mode_serialize() {
        let full = IngestionMode::Full;
        let json = serde_json::to_string(&full).unwrap();
        assert_eq!(json, "\"Full\"");

        let incremental = IngestionMode::Incremental;
        let json = serde_json::to_string(&incremental).unwrap();
        assert_eq!(json, "\"Incremental\"");
    }

    #[test]
    fn test_ingestion_mode_deserialize() {
        let full: IngestionMode = serde_json::from_str("\"Full\"").unwrap();
        assert_eq!(full, IngestionMode::Full);

        let incremental: IngestionMode = serde_json::from_str("\"Incremental\"").unwrap();
        assert_eq!(incremental, IngestionMode::Incremental);
    }
}
