//! Workspace management module.
//!
//! Provides the state machine, CRUD operations, per-workspace schema creation,
//! and the [`WorkspaceManager`] handle stored in [`crate::state::AppState`].
//!
//! # Module Layout
//!
//! - [`models`] — `Workspace` struct, `WorkspaceStatus` enum, Postgres CRUD
//! - [`lifecycle`] — atomic status transitions with DB guards
//! - [`schema`] — per-workspace Postgres schema creation
//! - [`manager`] — `WorkspaceManager` stored in `AppState`

pub mod lifecycle;
pub mod manager;
pub mod models;
pub mod schema;

pub use manager::WorkspaceManager;
pub use models::{
    create_workspace, get_workspace, list_workspaces, CreateWorkspaceParams, Workspace,
    WorkspaceSourceType, WorkspaceStatus,
};
