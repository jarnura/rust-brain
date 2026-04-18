//! Workspace management module.
//!
//! Provides the state machine, CRUD operations, per-workspace schema creation,
//! Qdrant collection naming, and the [`WorkspaceManager`] handle stored in
//! [`crate::state::AppState`].
//!
//! # Module Layout
//!
//! - [`models`] — `Workspace` struct, `WorkspaceStatus` enum, Postgres CRUD
//! - [`lifecycle`] — atomic status transitions with DB guards, Qdrant collection lifecycle
//! - [`schema`] — per-workspace Postgres schema creation, Qdrant collection naming
//! - [`manager`] — `WorkspaceManager` stored in `AppState`

pub mod lifecycle;
pub mod manager;
pub mod models;
pub mod pg_conn;
pub mod schema;

pub use manager::WorkspaceManager;
pub use models::{
    create_workspace, get_workspace, list_workspaces, CreateWorkspaceParams, Workspace,
    WorkspaceSourceType, WorkspaceStatus,
};
pub use pg_conn::acquire_conn;
pub use schema::{
    collection_name_for, default_collection_name, resolve_code_collection, resolve_doc_collection,
    workspace_collections, WorkspaceCollections, COLLECTION_TYPE_CODE, COLLECTION_TYPE_CRATE_DOCS,
    COLLECTION_TYPE_DOC, COLLECTION_TYPE_EXTERNAL_DOCS,
};
