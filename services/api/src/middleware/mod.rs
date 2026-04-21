//! Axum middleware for the rust-brain API server.
//!
//! - [`workspace_metrics`] — Prometheus request metrics with workspace labels
//! - [`auth`] — API key authentication and tier enforcement

pub mod auth;
pub mod workspace_metrics;

pub use workspace_metrics::workspace_metrics;
