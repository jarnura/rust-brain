//! Common error types shared across rust-brain services.
//!
//! [`RustBrainError`] is the domain error enum used by the `rustbrain-common`
//! crate. Service-specific error types (e.g., [`crate`]-level `AppError` in
//! the API) typically wrap or convert from these variants.

use thiserror::Error;

/// Domain errors that can occur across rust-brain services.
///
/// Each variant wraps a descriptive message string. Used primarily by the
/// ingestion pipeline; the API server has its own [`AppError`] type that
/// maps to HTTP status codes.
#[derive(Debug, Error)]
pub enum RustBrainError {
    /// PostgreSQL operation failed
    #[error("Database error: {0}")]
    Database(String),

    /// Neo4j graph database operation failed
    #[error("Neo4j error: {0}")]
    Neo4j(String),

    /// Qdrant vector store operation failed
    #[error("Qdrant error: {0}")]
    Qdrant(String),

    /// Ollama embedding or chat model operation failed
    #[error("Ollama error: {0}")]
    Ollama(String),

    /// Source code parsing failed (syn or tree-sitter)
    #[error("Parse error: {0}")]
    Parse(String),

    /// Requested item was not found in any store
    #[error("Not found: {0}")]
    NotFound(String),

    /// Invalid or missing configuration value
    #[error("Configuration error: {0}")]
    Config(String),
}
