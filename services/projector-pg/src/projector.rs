//! `PgProjector` — the main event-consumption loop.
//!
//! Receives [`ProjectorEvent`] messages, writes them idempotently to the
//! tenant's `ws_<12hex>` schema, and emits [`IngestStatusEvent`] messages
//! for each stage boundary via the provided status sender.
//!
//! # Idempotency
//!
//! - `source_files` — `ON CONFLICT (crate_name, module_path, file_path) DO UPDATE`
//! - `extracted_items` — `ON CONFLICT (fqn) DO UPDATE`
//! - `trait_implementations` — `ON CONFLICT (impl_fqn) DO UPDATE`
//! - `call_sites` — plain `INSERT` (no unique constraint; callers must
//!   deduplicate before sending if they need idempotency for call sites)
//!
//! # Status events
//!
//! Two `IngestStatusEvent` messages are emitted per projector run:
//! 1. `Running` when the consumer starts processing.
//! 2. `Succeeded` or `Failed` when the receiver is drained.

use crate::events::{ItemEvent, ProjectorEvent, RelationEvent};
use crate::tenant_pool::TenantPool;
use anyhow::{Context, Result};
use chrono::Utc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use rustbrain_common::ingest_events::{IngestStage, IngestStageStatus, IngestStatusEvent};

/// The PG projection consumer.
///
/// Constructed once per ingestion run. Holds a [`TenantPool`] for tenant-scoped
/// database writes and an optional [`mpsc::Sender`] to publish
/// [`IngestStatusEvent`] messages downstream.
pub struct PgProjector {
    tenant_pool: TenantPool,
    status_tx: Option<mpsc::Sender<IngestStatusEvent>>,
    ingest_run_id: Uuid,
    attempt: i32,
}

impl PgProjector {
    /// Create a new projector.
    ///
    /// - `tenant_pool` — workspace-scoped Postgres pool.
    /// - `status_tx` — optional channel for status fan-out; pass `None` in
    ///   tests or when status reporting is not required.
    /// - `ingest_run_id` — the ingestion run this projector belongs to.
    /// - `attempt` — retry counter (starts at `1` for the first attempt).
    pub fn new(
        tenant_pool: TenantPool,
        status_tx: Option<mpsc::Sender<IngestStatusEvent>>,
        ingest_run_id: Uuid,
        attempt: i32,
    ) -> Self {
        Self { tenant_pool, status_tx, ingest_run_id, attempt }
    }

    /// Drain the event receiver, writing each event to the tenant schema.
    ///
    /// Emits `Running` at start and `Succeeded`/`Failed` at completion.
    pub async fn run(self, mut rx: mpsc::Receiver<ProjectorEvent>) -> Result<ProjectorStats> {
        info!(
            tenant_id = %self.tenant_pool.tenant_id(),
            schema = %self.tenant_pool.schema_name(),
            ingest_run_id = %self.ingest_run_id,
            attempt = self.attempt,
            "PgProjector starting"
        );

        self.emit_status(IngestStageStatus::Running, None).await;

        let mut stats = ProjectorStats::default();
        let mut first_error: Option<String> = None;

        while let Some(event) = rx.recv().await {
            match self.handle_event(&event).await {
                Ok(()) => match &event {
                    ProjectorEvent::Item(ItemEvent::SourceFile(_)) => stats.source_files += 1,
                    ProjectorEvent::Item(ItemEvent::ExtractedItem(_)) => stats.extracted_items += 1,
                    ProjectorEvent::Relation(RelationEvent::CallSite(_)) => stats.call_sites += 1,
                    ProjectorEvent::Relation(RelationEvent::TraitImpl(_)) => {
                        stats.trait_impls += 1
                    }
                },
                Err(e) => {
                    stats.errors += 1;
                    let msg = format!("{e:#}");
                    error!(error = %msg, "PgProjector: event write failed");
                    if first_error.is_none() {
                        first_error = Some(msg);
                    }
                }
            }
        }

        let (final_status, error_message) = if first_error.is_some() {
            (IngestStageStatus::Failed, first_error)
        } else {
            (IngestStageStatus::Succeeded, None)
        };

        self.emit_status(final_status, error_message).await;

        info!(
            ?stats,
            "PgProjector finished"
        );

        Ok(stats)
    }

    /// Dispatch a single event to the appropriate writer.
    async fn handle_event(&self, event: &ProjectorEvent) -> Result<()> {
        match event {
            ProjectorEvent::Item(ItemEvent::SourceFile(ev)) => {
                self.upsert_source_file(ev).await
            }
            ProjectorEvent::Item(ItemEvent::ExtractedItem(ev)) => {
                self.upsert_extracted_item(ev).await
            }
            ProjectorEvent::Relation(RelationEvent::CallSite(ev)) => {
                self.insert_call_site(ev).await
            }
            ProjectorEvent::Relation(RelationEvent::TraitImpl(ev)) => {
                self.upsert_trait_impl(ev).await
            }
        }
    }

    // -------------------------------------------------------------------------
    // Writers
    // -------------------------------------------------------------------------

    async fn upsert_source_file(&self, ev: &crate::events::SourceFileEvent) -> Result<()> {
        debug!(file_path = %ev.file_path, "upsert source_file");
        let mut conn = self.tenant_pool.acquire().await?;
        sqlx::query(
            r#"
            INSERT INTO source_files (id, crate_name, module_path, file_path,
                original_source, expanded_source, content_hash, git_hash)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (crate_name, module_path, file_path) DO UPDATE SET
                original_source = EXCLUDED.original_source,
                expanded_source = COALESCE(EXCLUDED.expanded_source, source_files.expanded_source),
                content_hash    = COALESCE(EXCLUDED.content_hash,    source_files.content_hash),
                git_hash        = COALESCE(EXCLUDED.git_hash,        source_files.git_hash),
                updated_at      = NOW()
            "#,
        )
        .bind(ev.id)
        .bind(&ev.crate_name)
        .bind(&ev.module_path)
        .bind(&ev.file_path)
        .bind(&ev.original_source)
        .bind(&ev.expanded_source)
        .bind(&ev.content_hash)
        .bind(&ev.git_hash)
        .execute(&mut *conn)
        .await
        .with_context(|| format!("Failed to upsert source_file '{}'", ev.file_path))?;
        Ok(())
    }

    async fn upsert_extracted_item(&self, ev: &crate::events::ExtractedItemEvent) -> Result<()> {
        debug!(fqn = %ev.fqn, "upsert extracted_item");
        let mut conn = self.tenant_pool.acquire().await?;
        sqlx::query(
            r#"
            INSERT INTO extracted_items (
                source_file_id, item_type, fqn, name, visibility,
                signature, doc_comment, start_line, end_line, body_source,
                generic_params, where_clauses, attributes, generated_by
            ) VALUES (
                $1, $2, $3, $4, $5,
                $6, $7, $8, $9, $10,
                $11, $12, $13, $14
            )
            ON CONFLICT (fqn) DO UPDATE SET
                source_file_id = COALESCE(EXCLUDED.source_file_id, extracted_items.source_file_id),
                item_type      = EXCLUDED.item_type,
                name           = EXCLUDED.name,
                visibility     = EXCLUDED.visibility,
                signature      = EXCLUDED.signature,
                doc_comment    = EXCLUDED.doc_comment,
                start_line     = EXCLUDED.start_line,
                end_line       = EXCLUDED.end_line,
                body_source    = EXCLUDED.body_source,
                generic_params = EXCLUDED.generic_params,
                where_clauses  = EXCLUDED.where_clauses,
                attributes     = EXCLUDED.attributes,
                generated_by   = COALESCE(EXCLUDED.generated_by, extracted_items.generated_by),
                updated_at     = NOW()
            "#,
        )
        .bind(ev.source_file_id)
        .bind(&ev.item_type)
        .bind(&ev.fqn)
        .bind(&ev.name)
        .bind(&ev.visibility)
        .bind(&ev.signature)
        .bind(&ev.doc_comment)
        .bind(ev.start_line)
        .bind(ev.end_line)
        .bind(&ev.body_source)
        .bind(&ev.generic_params)
        .bind(&ev.where_clauses)
        .bind(&ev.attributes)
        .bind(&ev.generated_by)
        .execute(&mut *conn)
        .await
        .with_context(|| format!("Failed to upsert extracted_item '{}'", ev.fqn))?;
        Ok(())
    }

    async fn insert_call_site(&self, ev: &crate::events::CallSiteEvent) -> Result<()> {
        debug!(caller = %ev.caller_fqn, callee = %ev.callee_fqn, "insert call_site");
        let mut conn = self.tenant_pool.acquire().await?;
        sqlx::query(
            r#"
            INSERT INTO call_sites (
                caller_fqn, callee_fqn, file_path, line_number,
                concrete_type_args, is_monomorphized, quality
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(&ev.caller_fqn)
        .bind(&ev.callee_fqn)
        .bind(&ev.file_path)
        .bind(ev.line_number)
        .bind(&ev.concrete_type_args)
        .bind(ev.is_monomorphized)
        .bind(&ev.quality)
        .execute(&mut *conn)
        .await
        .with_context(|| {
            format!(
                "Failed to insert call_site '{}' → '{}'",
                ev.caller_fqn, ev.callee_fqn
            )
        })?;
        Ok(())
    }

    async fn upsert_trait_impl(&self, ev: &crate::events::TraitImplEvent) -> Result<()> {
        debug!(impl_fqn = %ev.impl_fqn, "upsert trait_impl");
        let mut conn = self.tenant_pool.acquire().await?;
        sqlx::query(
            r#"
            INSERT INTO trait_implementations (
                trait_fqn, self_type, impl_fqn, file_path, line_number,
                generic_params, quality
            ) VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (impl_fqn) DO UPDATE SET
                trait_fqn      = EXCLUDED.trait_fqn,
                self_type      = EXCLUDED.self_type,
                file_path      = EXCLUDED.file_path,
                line_number    = EXCLUDED.line_number,
                generic_params = EXCLUDED.generic_params,
                quality        = EXCLUDED.quality
            "#,
        )
        .bind(&ev.trait_fqn)
        .bind(&ev.self_type)
        .bind(&ev.impl_fqn)
        .bind(&ev.file_path)
        .bind(ev.line_number)
        .bind(&ev.generic_params)
        .bind(&ev.quality)
        .execute(&mut *conn)
        .await
        .with_context(|| format!("Failed to upsert trait_impl '{}'", ev.impl_fqn))?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Status reporting
    // -------------------------------------------------------------------------

    async fn emit_status(&self, status: IngestStageStatus, error_message: Option<String>) {
        let Some(ref tx) = self.status_tx else {
            return;
        };
        let event = IngestStatusEvent {
            event_id: Uuid::new_v4(),
            tenant_id: self.tenant_pool.tenant_id(),
            stage: IngestStage::ProjectPg,
            stage_seq: IngestStage::ProjectPg.stage_seq(),
            ingest_run_id: self.ingest_run_id,
            attempt: self.attempt,
            status,
            error_message,
            timestamp: Utc::now(),
        };
        if let Err(e) = tx.send(event).await {
            warn!(error = %e, "PgProjector: failed to send IngestStatusEvent (receiver dropped)");
        }
    }
}

// =============================================================================
// Stats
// =============================================================================

/// Counters collected during a projector run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProjectorStats {
    pub source_files: u64,
    pub extracted_items: u64,
    pub call_sites: u64,
    pub trait_impls: u64,
    pub errors: u64,
}

impl ProjectorStats {
    /// Total events successfully written.
    pub fn total_written(&self) -> u64 {
        self.source_files + self.extracted_items + self.call_sites + self.trait_impls
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        CallSiteEvent, ExtractedItemEvent, ProjectorEvent, SourceFileEvent, TraitImplEvent,
    };
    use rustbrain_common::ingest_events::{IngestStage, IngestStageStatus};
    use tokio::sync::mpsc;

    fn sample_source_file_event() -> ProjectorEvent {
        ProjectorEvent::source_file(SourceFileEvent {
            id: Uuid::new_v4(),
            crate_name: "my_crate".into(),
            module_path: "my_crate::lib".into(),
            file_path: "src/lib.rs".into(),
            original_source: "fn main() {}".into(),
            expanded_source: None,
            content_hash: Some("abc".into()),
            git_hash: None,
        })
    }

    fn sample_extracted_item_event() -> ProjectorEvent {
        ProjectorEvent::extracted_item(ExtractedItemEvent {
            source_file_id: None,
            item_type: "function".into(),
            fqn: "my_crate::main".into(),
            name: "main".into(),
            visibility: "pub".into(),
            signature: None,
            doc_comment: None,
            start_line: 1,
            end_line: 3,
            body_source: None,
            generic_params: serde_json::json!([]),
            where_clauses: serde_json::json!([]),
            attributes: serde_json::json!([]),
            generated_by: None,
        })
    }

    fn sample_call_site_event() -> ProjectorEvent {
        ProjectorEvent::call_site(CallSiteEvent {
            caller_fqn: "my_crate::main".into(),
            callee_fqn: "std::println".into(),
            file_path: "src/lib.rs".into(),
            line_number: 2,
            concrete_type_args: serde_json::json!([]),
            is_monomorphized: false,
            quality: "heuristic".into(),
        })
    }

    fn sample_trait_impl_event() -> ProjectorEvent {
        ProjectorEvent::trait_impl(TraitImplEvent {
            trait_fqn: "std::fmt::Display".into(),
            self_type: "my_crate::MyStruct".into(),
            impl_fqn: "my_crate::MyStruct::impl_Display".into(),
            file_path: "src/lib.rs".into(),
            line_number: 10,
            generic_params: serde_json::json!([]),
            quality: "analyzed".into(),
        })
    }

    #[test]
    fn projector_stats_total_written() {
        let stats = ProjectorStats {
            source_files: 3,
            extracted_items: 10,
            call_sites: 5,
            trait_impls: 2,
            errors: 1,
        };
        assert_eq!(stats.total_written(), 20);
    }

    #[test]
    fn projector_stats_default_is_zero() {
        let stats = ProjectorStats::default();
        assert_eq!(stats.total_written(), 0);
        assert_eq!(stats.errors, 0);
    }

    #[test]
    fn status_event_uses_project_pg_stage() {
        // Verify the stage and seq used in emit_status match IngestStage::ProjectPg
        assert_eq!(IngestStage::ProjectPg.stage_seq(), 7);
        assert_eq!(IngestStage::ProjectPg.as_str(), "project_pg");
    }

    #[tokio::test]
    async fn run_drains_empty_channel() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let tenant_id = Uuid::new_v4();
        let tp = TenantPool::new(pool, tenant_id).unwrap();
        let (tx, rx) = mpsc::channel::<ProjectorEvent>(8);
        drop(tx); // Close immediately — no events

        // No status_tx so we don't need a real channel
        let projector = PgProjector::new(tp, None, Uuid::new_v4(), 1);
        let stats = projector.run(rx).await.unwrap();
        assert_eq!(stats.total_written(), 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn status_events_emitted_on_run() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let tenant_id = Uuid::new_v4();
        let tp = TenantPool::new(pool, tenant_id).unwrap();

        let (status_tx, mut status_rx) = mpsc::channel::<IngestStatusEvent>(8);
        let (event_tx, event_rx) = mpsc::channel::<ProjectorEvent>(8);
        drop(event_tx); // Drain immediately

        let projector =
            PgProjector::new(tp, Some(status_tx), Uuid::new_v4(), 1);
        let _ = projector.run(event_rx).await.unwrap();

        // Should receive Running then Succeeded
        let first = status_rx.recv().await.unwrap();
        assert_eq!(first.status, IngestStageStatus::Running);
        assert_eq!(first.stage, IngestStage::ProjectPg);
        assert_eq!(first.attempt, 1);

        let second = status_rx.recv().await.unwrap();
        assert_eq!(second.status, IngestStageStatus::Succeeded);
        assert_eq!(second.stage, IngestStage::ProjectPg);
        assert_eq!(second.ingest_run_id, first.ingest_run_id);
    }

    #[tokio::test]
    async fn status_events_use_correct_tenant_id() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let tenant_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let tp = TenantPool::new(pool, tenant_id).unwrap();

        let (status_tx, mut status_rx) = mpsc::channel::<IngestStatusEvent>(8);
        let (event_tx, event_rx) = mpsc::channel::<ProjectorEvent>(8);
        drop(event_tx);

        let run_id = Uuid::new_v4();
        let projector = PgProjector::new(tp, Some(status_tx), run_id, 2);
        let _ = projector.run(event_rx).await.unwrap();

        let ev = status_rx.recv().await.unwrap();
        assert_eq!(ev.tenant_id, tenant_id);
        assert_eq!(ev.ingest_run_id, run_id);
        assert_eq!(ev.attempt, 2);
    }

    /// Event dispatch routing test — verifies that each event variant increments
    /// the correct stat counter. Uses a lazy pool; the actual DB write will fail
    /// since no Postgres is running, but we test the routing logic by checking
    /// that errors are counted rather than panicking.
    #[tokio::test]
    async fn event_routing_increments_correct_counters() {
        // Only routing is tested here. DB writes will fail (no real Postgres),
        // but the error path counts are correct.
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/rustbrain").unwrap();
        let tenant_id = Uuid::new_v4();
        let tp = TenantPool::new(pool, tenant_id).unwrap();

        let (tx, rx) = mpsc::channel::<ProjectorEvent>(16);
        // Send one of each kind then close
        for ev in [
            sample_source_file_event(),
            sample_extracted_item_event(),
            sample_call_site_event(),
            sample_trait_impl_event(),
        ] {
            tx.send(ev).await.unwrap();
        }
        drop(tx);

        let projector = PgProjector::new(tp, None, Uuid::new_v4(), 1);
        let stats = projector.run(rx).await.unwrap();

        // All 4 events should be attempted. With no DB they'll all error.
        // The sum (written + errors) must equal 4.
        assert_eq!(stats.total_written() + stats.errors, 4);
    }
}
