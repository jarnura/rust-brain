//! IngestStatusEvent and related types — re-exported from `rustbrain-common`.
//!
//! The canonical definitions live in
//! `rustbrain_common::ingest_events` so that `projector-pg` and other services
//! can share the same types without a cross-service dependency.

pub use rustbrain_common::ingest_events::{
    IngestStage, IngestStageStatus, IngestStatusEvent,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

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
    }

    #[test]
    fn ingest_stage_status_display() {
        assert_eq!(IngestStageStatus::Running.to_string(), "running");
        assert_eq!(IngestStageStatus::Succeeded.to_string(), "succeeded");
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
    }
}
