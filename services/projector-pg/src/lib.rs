//! `rustbrain-projector-pg` — PG projection consumer (REQ-IN-09).
//!
//! Consumes [`ProjectorEvent`] messages produced by the parse/extract pipeline,
//! writes them idempotently to the workspace-scoped Postgres schema
//! (`ws_<12hex>`) via a [`TenantPool`], and emits [`IngestStatusEvent`]
//! messages to the `rb.projector.events` channel for downstream fan-out.
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐  ProjectorEvent   ┌─────────────┐  IngestStatusEvent
//! │  ingestion   │ ────────────────► │ projector-pg│ ──────────────────►
//! │  pipeline    │   (mpsc channel)  │ PgProjector │   rb.projector.events
//! └──────────────┘                   └──────┬──────┘
//!                                           │ upsert by FQN
//!                                           ▼
//!                                  ┌─────────────────────┐
//!                                  │  ws_<12hex> schema  │
//!                                  │  source_files       │
//!                                  │  extracted_items    │
//!                                  │  call_sites         │
//!                                  │  trait_impls        │
//!                                  └─────────────────────┘
//! ```
//!
//! # Key types
//!
//! | Type | Role |
//! |------|------|
//! | [`TenantPool`] | Workspace-scoped `PgPool` — sets `search_path` on each connection |
//! | [`PgProjector`] | Main consumer struct — drives the event loop |
//! | [`ProjectorEvent`] | Top-level event envelope (item or relation) |
//! | [`ProjectorStats`] | Per-run write counters |

pub mod events;
pub mod projector;
pub mod tenant_pool;

pub use events::{
    CallSiteEvent, ExtractedItemEvent, ItemEvent, ProjectorEvent, RelationEvent, SourceFileEvent,
    TraitImplEvent,
};
pub use projector::{PgProjector, ProjectorStats};
pub use tenant_pool::{schema_name_for, TenantPool};
