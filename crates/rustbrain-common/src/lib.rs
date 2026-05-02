//! Shared types and utilities for rust-brain code intelligence platform
//!
//! This crate provides common types used across the ingestion and API services,
//! ensuring type consistency across the triple-storage architecture
//! (Postgres, Neo4j, Qdrant).

pub mod config;
pub mod errors;
pub mod events;
pub mod ingest_events;
pub mod logging;
pub mod types;

pub use config::*;
pub use errors::*;
pub use events::*;
pub use types::*;
