//! Shared types and utilities for rust-brain code intelligence platform
//!
//! This crate provides common types used across the ingestion and API services,
//! ensuring type consistency across the triple-storage architecture
//! (Postgres, Neo4j, Qdrant).

pub mod types;
pub mod errors;
pub mod config;

pub use types::*;
pub use errors::*;
pub use config::*;
