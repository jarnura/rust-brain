//! Common error types shared across rust-brain services

use thiserror::Error;

/// Errors that can occur across rust-brain services
#[derive(Debug, Error)]
pub enum RustBrainError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("Neo4j error: {0}")]
    Neo4j(String),

    #[error("Qdrant error: {0}")]
    Qdrant(String),

    #[error("Ollama error: {0}")]
    Ollama(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),
}
