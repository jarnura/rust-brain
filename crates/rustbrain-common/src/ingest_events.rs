//! Shared ingestion-pipeline event types per ADR-007 §3.1/§3.2.
//!
//! Moved to `rustbrain-common` so that `projector-pg` and `ingest-status`
//! can share a single definition without a cross-service dependency.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Pipeline stage identifiers used in [`IngestStatusEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IngestStage {
    Unspecified,
    Clone,
    Expand,
    Parse,
    Typecheck,
    Extract,
    Embed,
    ProjectPg,
    ProjectNeo4j,
    ProjectQdrant,
}

impl IngestStage {
    /// Returns the lowercase string representation used in logs and status tables.
    pub fn as_str(self) -> &'static str {
        match self {
            IngestStage::Unspecified => "unspecified",
            IngestStage::Clone => "clone",
            IngestStage::Expand => "expand",
            IngestStage::Parse => "parse",
            IngestStage::Typecheck => "typecheck",
            IngestStage::Extract => "extract",
            IngestStage::Embed => "embed",
            IngestStage::ProjectPg => "project_pg",
            IngestStage::ProjectNeo4j => "project_neo4j",
            IngestStage::ProjectQdrant => "project_qdrant",
        }
    }

    /// Monotonically increasing sequence number for ordering stages.
    pub fn stage_seq(self) -> i32 {
        match self {
            IngestStage::Unspecified => 0,
            IngestStage::Clone => 1,
            IngestStage::Expand => 2,
            IngestStage::Parse => 3,
            IngestStage::Typecheck => 4,
            IngestStage::Extract => 5,
            IngestStage::Embed => 6,
            IngestStage::ProjectPg => 7,
            IngestStage::ProjectNeo4j => 8,
            IngestStage::ProjectQdrant => 9,
        }
    }
}

impl std::fmt::Display for IngestStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Status of a single pipeline stage within one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStageStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

impl IngestStageStatus {
    /// Returns the lowercase string stored in `pipeline_stage_runs.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            IngestStageStatus::Pending => "pending",
            IngestStageStatus::Running => "running",
            IngestStageStatus::Succeeded => "succeeded",
            IngestStageStatus::Failed => "failed",
            IngestStageStatus::Skipped => "skipped",
        }
    }
}

impl std::fmt::Display for IngestStageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A single stage-completion event emitted to the `rb.projector.events` channel.
///
/// Consumed by `ingest-status` to update `pipeline_stage_runs` and fan out
/// to per-tenant SSE streams.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestStatusEvent {
    /// Unique identifier for this event (used for idempotency in the consumer).
    pub event_id: Uuid,
    /// The workspace / tenant this event belongs to.
    pub tenant_id: Uuid,
    /// Which pipeline stage just transitioned.
    pub stage: IngestStage,
    /// Sequence number for this stage (matches [`IngestStage::stage_seq`]).
    pub stage_seq: i32,
    /// The ingestion run this event belongs to.
    pub ingest_run_id: Uuid,
    /// Retry counter, starting at `1`.
    pub attempt: i32,
    /// New status for this stage.
    pub status: IngestStageStatus,
    /// Error detail when `status` is `Failed`.
    pub error_message: Option<String>,
    /// Wall-clock time the event was emitted.
    pub timestamp: DateTime<Utc>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_stage_seq_values() {
        assert_eq!(IngestStage::Clone.stage_seq(), 1);
        assert_eq!(IngestStage::Expand.stage_seq(), 2);
        assert_eq!(IngestStage::Parse.stage_seq(), 3);
        assert_eq!(IngestStage::Typecheck.stage_seq(), 4);
        assert_eq!(IngestStage::Extract.stage_seq(), 5);
        assert_eq!(IngestStage::Embed.stage_seq(), 6);
        assert_eq!(IngestStage::ProjectPg.stage_seq(), 7);
        assert_eq!(IngestStage::ProjectNeo4j.stage_seq(), 8);
        assert_eq!(IngestStage::ProjectQdrant.stage_seq(), 9);
    }

    #[test]
    fn ingest_stage_display() {
        assert_eq!(IngestStage::Clone.to_string(), "clone");
        assert_eq!(IngestStage::ProjectQdrant.to_string(), "project_qdrant");
        assert_eq!(IngestStage::ProjectPg.to_string(), "project_pg");
    }

    #[test]
    fn ingest_stage_status_display() {
        assert_eq!(IngestStageStatus::Running.to_string(), "running");
        assert_eq!(IngestStageStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(IngestStageStatus::Failed.to_string(), "failed");
        assert_eq!(IngestStageStatus::Pending.to_string(), "pending");
        assert_eq!(IngestStageStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn ingest_stage_status_as_str_matches_display() {
        for (status, expected) in [
            (IngestStageStatus::Pending, "pending"),
            (IngestStageStatus::Running, "running"),
            (IngestStageStatus::Succeeded, "succeeded"),
            (IngestStageStatus::Failed, "failed"),
            (IngestStageStatus::Skipped, "skipped"),
        ] {
            assert_eq!(status.as_str(), expected);
            assert_eq!(status.to_string(), expected);
        }
    }

    #[test]
    fn ingest_status_event_serialization() {
        let event = IngestStatusEvent {
            event_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            stage: IngestStage::Parse,
            stage_seq: 3,
            ingest_run_id: Uuid::new_v4(),
            attempt: 1,
            status: IngestStageStatus::Running,
            error_message: None,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: IngestStatusEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.stage, IngestStage::Parse);
        assert_eq!(restored.status, IngestStageStatus::Running);
        assert_eq!(restored.attempt, 1);
        assert_eq!(restored.stage_seq, 3);
    }

    #[test]
    fn ingest_stage_as_str_all_variants() {
        for (stage, expected) in [
            (IngestStage::Unspecified, "unspecified"),
            (IngestStage::Clone, "clone"),
            (IngestStage::Expand, "expand"),
            (IngestStage::Parse, "parse"),
            (IngestStage::Typecheck, "typecheck"),
            (IngestStage::Extract, "extract"),
            (IngestStage::Embed, "embed"),
            (IngestStage::ProjectPg, "project_pg"),
            (IngestStage::ProjectNeo4j, "project_neo4j"),
            (IngestStage::ProjectQdrant, "project_qdrant"),
        ] {
            assert_eq!(stage.as_str(), expected);
        }
    }
}
