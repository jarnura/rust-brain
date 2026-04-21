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
    create_execution, get_agent_event_by_seq, get_execution, insert_agent_event, list_agent_events,
    list_agent_events_after, list_agent_events_after_seq, list_executions, AgentEvent,
    CreateExecutionParams, Execution,
};
pub use runner::{run_execution, RunParams};
pub use sweeper::start_sweeper;
