use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    PipelineStarted,
    PipelineCompleted,
    PipelineFailed,
    StageStarted,
    StageCompleted,
    StageFailed,
    StageSkipped,
    CheckpointCreated,
    DegradationChanged,
    CircuitBreakerTripped,
}

impl AuditEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PipelineStarted => "pipeline_started",
            Self::PipelineCompleted => "pipeline_completed",
            Self::PipelineFailed => "pipeline_failed",
            Self::StageStarted => "stage_started",
            Self::StageCompleted => "stage_completed",
            Self::StageFailed => "stage_failed",
            Self::StageSkipped => "stage_skipped",
            Self::CheckpointCreated => "checkpoint_created",
            Self::DegradationChanged => "degradation_changed",
            Self::CircuitBreakerTripped => "circuit_breaker_tripped",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub pipeline_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub stage: Option<String>,
    pub detail: serde_json::Value,
    pub severity: Severity,
}

impl AuditEvent {
    pub fn new(
        pipeline_id: Uuid,
        event_type: AuditEventType,
        stage: Option<String>,
        detail: serde_json::Value,
        severity: Severity,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            pipeline_id,
            timestamp: Utc::now(),
            event_type,
            stage,
            detail,
            severity,
        }
    }
}

// ---------------------------------------------------------------------------
// AuditEmitter — async channel + batched Postgres writer
// ---------------------------------------------------------------------------

const DEFAULT_BATCH_SIZE: usize = 64;
const DEFAULT_CHANNEL_CAPACITY: usize = 4096;

/// Handle returned to callers for emitting audit events.
#[derive(Clone)]
pub struct AuditEmitter {
    tx: mpsc::Sender<AuditEvent>,
}

impl AuditEmitter {
    /// Create a new emitter + background flush task.
    ///
    /// Returns the emitter handle and a `JoinHandle` for the background
    /// writer.  Drop every `AuditEmitter` clone (or call [`Self::shutdown`])
    /// to signal the writer to flush remaining events and exit.
    pub fn spawn(pool: PgPool, batch_size: Option<usize>) -> (Self, tokio::task::JoinHandle<()>) {
        let batch_size = batch_size.unwrap_or(DEFAULT_BATCH_SIZE);
        let (tx, rx) = mpsc::channel(DEFAULT_CHANNEL_CAPACITY);

        let handle = tokio::spawn(flush_loop(rx, pool, batch_size));

        (Self { tx }, handle)
    }

    /// Emit an audit event (non-blocking best-effort).
    ///
    /// If the channel is full the event is dropped and a warning is logged,
    /// so callers on the hot path are never blocked.
    pub fn emit(&self, event: AuditEvent) {
        if let Err(mpsc::error::TrySendError::Full(ev)) = self.tx.try_send(event) {
            warn!(
                event_type = ev.event_type.as_str(),
                "audit channel full — dropping event"
            );
        }
    }

    /// Convenience: build + emit in one call.
    pub fn record(
        &self,
        pipeline_id: Uuid,
        event_type: AuditEventType,
        stage: Option<&str>,
        detail: serde_json::Value,
        severity: Severity,
    ) {
        self.emit(AuditEvent::new(
            pipeline_id,
            event_type,
            stage.map(String::from),
            detail,
            severity,
        ));
    }

    /// Close the sending side so the background writer can drain and exit.
    pub async fn shutdown(self, handle: tokio::task::JoinHandle<()>) {
        drop(self.tx);
        if let Err(e) = handle.await {
            error!("audit flush task panicked: {e}");
        }
    }

    /// Create a no-op emitter whose events are silently discarded.
    /// Useful in tests or when Postgres is unavailable.
    pub fn noop() -> Self {
        let (tx, _rx) = mpsc::channel(1);
        Self { tx }
    }
}

// ---------------------------------------------------------------------------
// Background flush loop
// ---------------------------------------------------------------------------

async fn flush_loop(mut rx: mpsc::Receiver<AuditEvent>, pool: PgPool, batch_size: usize) {
    let mut buf: Vec<AuditEvent> = Vec::with_capacity(batch_size);

    loop {
        // Wait for the first event (or channel close).
        match rx.recv().await {
            Some(event) => buf.push(event),
            None => break, // channel closed — drain and exit
        }

        // Drain up to batch_size without waiting.
        while buf.len() < batch_size {
            match rx.try_recv() {
                Ok(event) => buf.push(event),
                Err(_) => break,
            }
        }

        if let Err(e) = flush_batch(&pool, &buf).await {
            error!(count = buf.len(), "failed to flush audit batch: {e}");
        }
        buf.clear();
    }

    // Final flush for any remaining events.
    if !buf.is_empty() {
        if let Err(e) = flush_batch(&pool, &buf).await {
            error!(count = buf.len(), "failed to flush final audit batch: {e}");
        }
    }

    info!("audit flush loop exiting");
}

async fn flush_batch(pool: &PgPool, events: &[AuditEvent]) -> Result<(), sqlx::Error> {
    if events.is_empty() {
        return Ok(());
    }

    // Build a single INSERT with unnested arrays for efficiency.
    let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
    let pipeline_ids: Vec<Uuid> = events.iter().map(|e| e.pipeline_id).collect();
    let timestamps: Vec<DateTime<Utc>> = events.iter().map(|e| e.timestamp).collect();
    let event_types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    let stages: Vec<Option<&str>> = events
        .iter()
        .map(|e| e.stage.as_deref())
        .collect();
    let details: Vec<&serde_json::Value> = events.iter().map(|e| &e.detail).collect();
    let severities: Vec<&str> = events.iter().map(|e| e.severity.as_str()).collect();

    sqlx::query(
        r#"
        INSERT INTO audit_events (id, pipeline_id, timestamp, event_type, stage, detail, severity)
        SELECT * FROM UNNEST(
            $1::uuid[],
            $2::uuid[],
            $3::timestamptz[],
            $4::text[],
            $5::text[],
            $6::jsonb[],
            $7::text[]
        )
        "#,
    )
    .bind(&ids)
    .bind(&pipeline_ids)
    .bind(&timestamps)
    .bind(&event_types)
    .bind(&stages)
    .bind(&details)
    .bind(&severities)
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_event_new_populates_fields() {
        let pid = Uuid::new_v4();
        let ev = AuditEvent::new(
            pid,
            AuditEventType::StageStarted,
            Some("parse".into()),
            serde_json::json!({"files": 42}),
            Severity::Info,
        );

        assert_eq!(ev.pipeline_id, pid);
        assert!(matches!(ev.event_type, AuditEventType::StageStarted));
        assert_eq!(ev.stage.as_deref(), Some("parse"));
        assert_eq!(ev.severity.as_str(), "info");
    }

    #[test]
    fn event_type_round_trips() {
        let variants = [
            AuditEventType::PipelineStarted,
            AuditEventType::PipelineCompleted,
            AuditEventType::PipelineFailed,
            AuditEventType::StageStarted,
            AuditEventType::StageCompleted,
            AuditEventType::StageFailed,
            AuditEventType::StageSkipped,
            AuditEventType::CheckpointCreated,
            AuditEventType::DegradationChanged,
            AuditEventType::CircuitBreakerTripped,
        ];
        for v in &variants {
            let s = v.as_str();
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn noop_emitter_does_not_panic() {
        let emitter = AuditEmitter::noop();
        emitter.record(
            Uuid::new_v4(),
            AuditEventType::PipelineStarted,
            None,
            serde_json::json!({}),
            Severity::Info,
        );
    }
}
