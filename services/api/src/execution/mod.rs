//! Execution management module.
//!
//! Orchestrates the full lifecycle of a per-workspace OpenCode execution:
//!
//! | Sub-module | Responsibility |
//! |------------|----------------|
//! | [`models`]  | Postgres structs + CRUD (Execution, AgentEvent) |
//! | [`runner`]  | Container spawn, orchestrator flow, event bridge |
//! | [`sweeper`] | Background task to kill timed-out containers |

pub mod models;
pub mod runner;
pub mod sweeper;

pub use models::{
    AgentEvent, CreateExecutionParams, Execution,
    create_execution, get_execution, list_executions,
    list_agent_events, list_agent_events_after,
};
pub use runner::{RunParams, run_execution};
pub use sweeper::start_sweeper;
