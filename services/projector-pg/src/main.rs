//! rust-brain PG Projection Consumer (REQ-IN-09)
//!
//! Consumes item/relation events from the ingestion pipeline and writes them
//! idempotently to the workspace-scoped Postgres schema via [`TenantPool`].
//! Emits [`IngestStatusEvent`] messages per stage completion.
//!
//! # Usage
//!
//! ```bash
//! # Connect to Postgres and listen on the default mpsc channel
//! rustbrain-projector-pg \
//!     --database-url postgres://localhost/rustbrain \
//!     --tenant-id 550e8400-e29b-41d4-a716-446655440000 \
//!     --ingest-run-id <uuid>
//! ```
//!
//! # Environment variables
//!
//! | Variable | Description |
//! |----------|-------------|
//! | `DATABASE_URL` | Postgres connection string |
//! | `TENANT_ID` | Workspace UUID |
//! | `INGEST_RUN_ID` | Active ingestion run UUID |

pub mod events;
pub mod projector;
pub mod tenant_pool;

use anyhow::{Context, Result};
use clap::Parser;
use projector::PgProjector;
use rustbrain_common::ingest_events::IngestStatusEvent;
use tenant_pool::TenantPool;
use tokio::sync::mpsc;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;
use uuid::Uuid;

/// PG projection consumer — upserts items/relations into tenant schema tables.
#[derive(Parser, Debug)]
#[command(name = "rustbrain-projector-pg")]
#[command(version = "0.1.0")]
#[command(about = "Idempotent PG projection consumer for ingested items and relations (REQ-IN-09)")]
struct Args {
    /// Postgres connection URL.
    #[arg(short, long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Workspace (tenant) UUID. Sets the target schema to `ws_<12hex>`.
    #[arg(long, env = "TENANT_ID")]
    tenant_id: Option<String>,

    /// Ingestion run UUID for status event attribution.
    #[arg(long, env = "INGEST_RUN_ID")]
    ingest_run_id: Option<String>,

    /// Retry attempt counter (default 1).
    #[arg(long, default_value = "1")]
    attempt: i32,

    /// Status event channel URL / address (stub; future Kafka integration).
    #[arg(long, env = "STATUS_TOPIC", default_value = "rb.projector.events")]
    status_topic: String,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = if args.verbose { Level::DEBUG } else { Level::INFO };
    FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .pretty()
        .init();

    info!("rust-brain projector-pg starting");

    let database_url = args
        .database_url
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .context("DATABASE_URL must be set via --database-url or DATABASE_URL env var")?;

    let tenant_id: Uuid = args
        .tenant_id
        .or_else(|| std::env::var("TENANT_ID").ok())
        .context("TENANT_ID must be set via --tenant-id or TENANT_ID env var")?
        .parse()
        .context("TENANT_ID must be a valid UUID")?;

    let ingest_run_id: Uuid = args
        .ingest_run_id
        .or_else(|| std::env::var("INGEST_RUN_ID").ok())
        .context("INGEST_RUN_ID must be set via --ingest-run-id or INGEST_RUN_ID env var")?
        .parse()
        .context("INGEST_RUN_ID must be a valid UUID")?;

    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .context("Failed to connect to Postgres")?;

    let tenant_pool = TenantPool::new(pool, tenant_id)
        .context("Failed to create TenantPool")?;

    info!(
        schema = %tenant_pool.schema_name(),
        status_topic = %args.status_topic,
        "Projector configured"
    );

    let (status_tx, mut status_rx) = mpsc::channel::<IngestStatusEvent>(256);
    let (event_tx, event_rx) = mpsc::channel::<events::ProjectorEvent>(1024);

    // Spawn a task to log / forward status events.
    // In production, swap this for a Kafka producer once RUSAA-59 delivers.
    tokio::spawn(async move {
        while let Some(ev) = status_rx.recv().await {
            info!(
                stage = %ev.stage,
                status = %ev.status,
                tenant_id = %ev.tenant_id,
                ingest_run_id = %ev.ingest_run_id,
                "IngestStatusEvent"
            );
        }
    });

    // _event_tx must outlive the projector: dropping it signals end-of-stream.
    // Callers (pipeline stages) send into this channel; we expose it for wiring.
    let _event_tx = event_tx;

    let projector = PgProjector::new(tenant_pool, Some(status_tx), ingest_run_id, args.attempt);
    let stats = projector.run(event_rx).await?;

    info!(
        source_files = stats.source_files,
        extracted_items = stats.extracted_items,
        call_sites = stats.call_sites,
        trait_impls = stats.trait_impls,
        errors = stats.errors,
        "Projection run complete"
    );

    if stats.errors > 0 {
        std::process::exit(1);
    }

    Ok(())
}
