//! Axum middleware for the rust-brain API server.
//!
//! - [`auth`] — API key authentication and tier enforcement
//! - [`rate_limit`] — Per-key rate limiting with fixed-window token buckets
//! - [`workspace_metrics`] — Prometheus request metrics with workspace labels

pub mod auth;
pub mod rate_limit;
pub mod workspace_metrics;

pub use rate_limit::PerKeyRateLimiter;
pub use workspace_metrics::workspace_metrics;
