//! Audit Trail Module
//!
//! Provides comprehensive audit logging for all API operations.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;

/// A single audit log entry recording an operation
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AuditEntry {
    /// Unique identifier for this audit entry
    pub id: u64,
    /// When the operation occurred
    pub timestamp: DateTime<Utc>,
    /// The type of operation performed
    pub operation: Operation,
    /// Result status of the operation
    pub status: Status,
    /// Duration of the operation in milliseconds
    pub duration_ms: u64,
    /// Input parameters for the operation
    pub input: serde_json::Value,
    /// Output/result of the operation
    pub output: serde_json::Value,
    /// Error message if operation failed
    pub error: Option<String>,
}

/// Types of operations that can be audited
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    SemanticSearch { query: String },
    GetFunction { fqn: String },
    GetCallers { fqn: String, depth: usize },
    GetTraitImpls { trait_name: String },
    FindUsages { type_name: String },
    ModuleTree { crate_name: String },
    GraphQuery { query: String },
    Ingestion { crate_name: String },
    HealthCheck,
}

impl Operation {
    /// Get a string representation of the operation type for filtering
    pub fn type_name(&self) -> &'static str {
        match self {
            Operation::SemanticSearch { .. } => "SemanticSearch",
            Operation::GetFunction { .. } => "GetFunction",
            Operation::GetCallers { .. } => "GetCallers",
            Operation::GetTraitImpls { .. } => "GetTraitImpls",
            Operation::FindUsages { .. } => "FindUsages",
            Operation::ModuleTree { .. } => "ModuleTree",
            Operation::GraphQuery { .. } => "GraphQuery",
            Operation::Ingestion { .. } => "Ingestion",
            Operation::HealthCheck => "HealthCheck",
        }
    }
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Operation::SemanticSearch { query } => write!(f, "SemanticSearch({})", query),
            Operation::GetFunction { fqn } => write!(f, "GetFunction({})", fqn),
            Operation::GetCallers { fqn, depth } => write!(f, "GetCallers({}, depth={})", fqn, depth),
            Operation::GetTraitImpls { trait_name } => write!(f, "GetTraitImpls({})", trait_name),
            Operation::FindUsages { type_name } => write!(f, "FindUsages({})", type_name),
            Operation::ModuleTree { crate_name } => write!(f, "ModuleTree({})", crate_name),
            Operation::GraphQuery { query } => write!(f, "GraphQuery({})", query),
            Operation::Ingestion { crate_name } => write!(f, "Ingestion({})", crate_name),
            Operation::HealthCheck => write!(f, "HealthCheck"),
        }
    }
}

/// Status of an audited operation
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    PartialSuccess,
    Failure,
}

/// Statistics derived from audit entries
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct AuditStats {
    /// Total number of entries
    pub total_entries: usize,
    /// Total queries per operation type
    pub queries_by_type: std::collections::HashMap<String, u64>,
    /// Success count per operation type
    pub success_by_type: std::collections::HashMap<String, u64>,
    /// Failure count per operation type
    pub failures_by_type: std::collections::HashMap<String, u64>,
    /// Average duration per operation type (in ms)
    pub avg_duration_by_type: std::collections::HashMap<String, f64>,
    /// Most common queries (operation input summaries)
    pub common_queries: Vec<CommonQuery>,
    /// Overall success rate
    pub overall_success_rate: f64,
}

/// A frequently occurring query pattern
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct CommonQuery {
    /// Operation type
    pub operation_type: String,
    /// Query string (simplified representation)
    pub query: String,
    /// Number of times this query appeared
    pub count: u64,
}

/// The main audit log storage
pub struct AuditLog {
    /// Ring buffer of audit entries
    entries: Arc<RwLock<VecDeque<AuditEntry>>>,
    /// Monotonic counter for entry IDs
    counter: Arc<RwLock<u64>>,
    /// Maximum number of entries to retain
    max_entries: usize,
}

impl AuditLog {
    /// Create a new audit log with a maximum capacity
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(RwLock::new(VecDeque::with_capacity(max_entries))),
            counter: Arc::new(RwLock::new(0)),
            max_entries,
        }
    }

    /// Log a new audit entry
    pub async fn log(&self, entry: AuditEntry) {
        let mut entries = self.entries.write().await;
        
        // If at capacity, remove oldest entry
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        
        entries.push_back(entry);
    }

    /// Create a new audit entry with auto-generated ID and timestamp
    pub async fn create_entry(
        &self,
        operation: Operation,
        status: Status,
        duration_ms: u64,
        input: serde_json::Value,
        output: serde_json::Value,
        error: Option<String>,
    ) -> AuditEntry {
        let mut counter = self.counter.write().await;
        *counter += 1;
        let id = *counter;

        AuditEntry {
            id,
            timestamp: Utc::now(),
            operation,
            status,
            duration_ms,
            input,
            output,
            error,
        }
    }

    /// Get recent audit entries, limited by count
    pub async fn get_recent(&self, limit: usize) -> Vec<AuditEntry> {
        let entries = self.entries.read().await;
        entries.iter().rev().take(limit).cloned().collect()
    }

    /// Get all audit entries
    pub async fn get_all(&self) -> Vec<AuditEntry> {
        let entries = self.entries.read().await;
        entries.iter().rev().cloned().collect()
    }

    /// Get entries filtered by operation type
    pub async fn get_by_operation(&self, op: &str) -> Vec<AuditEntry> {
        let entries = self.entries.read().await;
        entries
            .iter()
            .rev()
            .filter(|e| e.operation.type_name().eq_ignore_ascii_case(op))
            .cloned()
            .collect()
    }

    /// Get entries filtered by status
    pub async fn get_by_status(&self, status: &Status) -> Vec<AuditEntry> {
        let entries = self.entries.read().await;
        entries
            .iter()
            .rev()
            .filter(|e| &e.status == status)
            .cloned()
            .collect()
    }

    /// Get entry by ID
    pub async fn get_by_id(&self, id: u64) -> Option<AuditEntry> {
        let entries = self.entries.read().await;
        entries.iter().rev().find(|e| e.id == id).cloned()
    }

    /// Get current count of entries
    pub async fn count(&self) -> usize {
        let entries = self.entries.read().await;
        entries.len()
    }

    /// Clear all audit entries (use with caution)
    pub async fn clear(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }

    /// Compute statistics from current audit entries
    pub async fn get_stats(&self) -> AuditStats {
        let entries = self.entries.read().await;
        
        let mut stats = AuditStats::default();
        stats.total_entries = entries.len();
        
        // Temporary storage for duration calculations
        let mut total_duration_by_type: std::collections::HashMap<String, u64> = 
            std::collections::HashMap::new();
        let mut count_by_type: std::collections::HashMap<String, u64> = 
            std::collections::HashMap::new();
        
        // Track query patterns for common_queries
        let mut query_counts: std::collections::HashMap<(String, String), u64> = 
            std::collections::HashMap::new();
        
        let mut total_success = 0u64;
        let mut total_operations = 0u64;
        
        for entry in entries.iter() {
            let op_type = entry.operation.type_name().to_string();
            
            // Count by type
            *stats.queries_by_type.entry(op_type.clone()).or_insert(0) += 1;
            
            // Count success/failure by type
            match &entry.status {
                Status::Success | Status::PartialSuccess => {
                    *stats.success_by_type.entry(op_type.clone()).or_insert(0) += 1;
                    total_success += 1;
                }
                Status::Failure => {
                    *stats.failures_by_type.entry(op_type.clone()).or_insert(0) += 1;
                }
            }
            
            // Track durations
            *total_duration_by_type.entry(op_type.clone()).or_insert(0) += entry.duration_ms;
            *count_by_type.entry(op_type.clone()).or_insert(0) += 1;
            total_operations += 1;
            
            // Track query patterns
            let query_key = entry.get_query_key();
            *query_counts.entry((op_type, query_key)).or_insert(0) += 1;
        }
        
        // Calculate average durations
        for (op_type, count) in count_by_type.iter() {
            if let Some(&total_duration) = total_duration_by_type.get(op_type) {
                let avg = total_duration as f64 / *count as f64;
                stats.avg_duration_by_type.insert(op_type.clone(), avg);
            }
        }
        
        // Calculate overall success rate
        if total_operations > 0 {
            stats.overall_success_rate = (total_success as f64 / total_operations as f64) * 100.0;
        }
        
        // Get top 10 most common queries
        let mut query_vec: Vec<((String, String), u64)> = query_counts.into_iter().collect();
        query_vec.sort_by(|a, b| b.1.cmp(&a.1));
        
        stats.common_queries = query_vec
            .into_iter()
            .take(10)
            .map(|((op_type, query), count)| CommonQuery {
                operation_type: op_type,
                query,
                count,
            })
            .collect();
        
        stats
    }
}

impl AuditEntry {
    /// Get a simplified query key for tracking common patterns
    fn get_query_key(&self) -> String {
        match &self.operation {
            Operation::SemanticSearch { query } => query.clone(),
            Operation::GetFunction { fqn } => fqn.clone(),
            Operation::GetCallers { fqn, .. } => fqn.clone(),
            Operation::GetTraitImpls { trait_name } => trait_name.clone(),
            Operation::FindUsages { type_name } => type_name.clone(),
            Operation::ModuleTree { crate_name } => crate_name.clone(),
            Operation::GraphQuery { query } => {
                // Truncate long queries
                if query.len() > 100 {
                    format!("{}...", &query[..97])
                } else {
                    query.clone()
                }
            }
            Operation::Ingestion { crate_name } => crate_name.clone(),
            Operation::HealthCheck => "health_check".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_log_basic() {
        let log = AuditLog::new(100);
        
        let entry = log.create_entry(
            Operation::SemanticSearch { query: "test query".to_string() },
            Status::Success,
            50,
            serde_json::json!({"query": "test query"}),
            serde_json::json!({"results": []}),
            None,
        ).await;
        
        log.log(entry.clone()).await;
        
        let recent = log.get_recent(10).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].operation.type_name(), "SemanticSearch");
    }

    #[tokio::test]
    async fn test_audit_log_capacity() {
        let log = AuditLog::new(5);
        
        for i in 0..10 {
            let entry = log.create_entry(
                Operation::HealthCheck,
                Status::Success,
                i,
                serde_json::json!({}),
                serde_json::json!({}),
                None,
            ).await;
            log.log(entry).await;
        }
        
        let all = log.get_all().await;
        assert_eq!(all.len(), 5);
        // Most recent entries should be preserved
        assert!(all.iter().any(|e| e.duration_ms >= 5));
    }

    #[tokio::test]
    async fn test_audit_stats() {
        let log = AuditLog::new(100);
        
        // Add some entries
        for i in 0..5 {
            let entry = log.create_entry(
                Operation::SemanticSearch { query: format!("query {}", i) },
                Status::Success,
                100 + i,
                serde_json::json!({}),
                serde_json::json!({}),
                None,
            ).await;
            log.log(entry).await;
        }
        
        let failed_entry = log.create_entry(
            Operation::GetFunction { fqn: "test::func".to_string() },
            Status::Failure,
            200,
            serde_json::json!({}),
            serde_json::json!({}),
            Some("Not found".to_string()),
        ).await;
        log.log(failed_entry).await;
        
        let stats = log.get_stats().await;
        assert_eq!(stats.total_entries, 6);
        assert_eq!(*stats.queries_by_type.get("SemanticSearch").unwrap(), 5);
        assert_eq!(*stats.queries_by_type.get("GetFunction").unwrap(), 1);
        assert_eq!(*stats.failures_by_type.get("GetFunction").unwrap(), 1);
    }
}
