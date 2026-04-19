//! Per-workspace Prometheus gauge collectors.

pub mod workspace_gauges;

pub use workspace_gauges::{start_workspace_gauge_collector, WorkspaceGauges};
