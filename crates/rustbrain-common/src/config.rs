//! Shared configuration types for rust-brain services.
//!
//! Provides [`DatabaseConfig`] for multi-store connection credentials and
//! [`EmbeddingModelConfig`] for embedding pipeline settings. Both are
//! serializable and used by the ingestion and API services.
//!
//! # Examples
//!
//! ```no_run
//! use rustbrain_common::config::{DatabaseConfig, EmbeddingModelConfig};
//!
//! let db = DatabaseConfig::from_env();
//! let emb = EmbeddingModelConfig::default();
//! assert_eq!(emb.dimensions, 768);
//! ```

use serde::{Deserialize, Serialize};
use tracing::debug;

/// Connection credentials for all storage backends.
///
/// Holds URLs and authentication for PostgreSQL, Neo4j, Qdrant, and Ollama.
/// Construct via [`DatabaseConfig::from_env`] or deserialize from a config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL (e.g., `postgresql://user:pass@host:5432/db`)
    pub postgres_url: String,
    /// Neo4j Bolt URI (e.g., `bolt://neo4j:7687`)
    pub neo4j_uri: String,
    /// Neo4j username
    pub neo4j_user: String,
    /// Neo4j password
    pub neo4j_password: String,
    /// Qdrant REST API URL (e.g., `http://qdrant:6333`)
    pub qdrant_url: String,
    /// Ollama API URL (e.g., `http://ollama:11434`)
    pub ollama_url: String,
}

impl DatabaseConfig {
    /// Loads connection credentials from environment variables.
    ///
    /// Variables with defaults fall back silently; required variables cause a
    /// panic with a descriptive message.
    ///
    /// | Variable | Required | Default |
    /// |---|---|---|
    /// | `DATABASE_URL` | **yes** | — |
    /// | `NEO4J_URI` | no | `bolt://neo4j:7687` |
    /// | `NEO4J_USER` | no | `neo4j` |
    /// | `NEO4J_PASSWORD` | **yes** | — |
    /// | `QDRANT_HOST` | no | `http://qdrant:6333` |
    /// | `OLLAMA_HOST` | no | `http://ollama:11434` |
    ///
    /// # Panics
    ///
    /// Panics if `DATABASE_URL` or `NEO4J_PASSWORD` is not set.
    pub fn from_env() -> Self {
        Self {
            postgres_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable must be set"),
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "bolt://neo4j:7687".to_string()),
            neo4j_user: std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD")
                .expect("NEO4J_PASSWORD environment variable must be set"),
            qdrant_url: std::env::var("QDRANT_HOST")
                .unwrap_or_else(|_| "http://qdrant:6333".to_string()),
            ollama_url: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://ollama:11434".to_string()),
        }
    }
}

/// Configuration for the Ollama embedding model and Qdrant collections.
///
/// The [`Default`] implementation provides sensible values matching the
/// standard deployment (`nomic-embed-text`, 768 dimensions).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingModelConfig {
    /// Model name for Ollama
    pub model: String,
    /// Expected vector dimensions
    pub dimensions: usize,
    /// Collection name for code embeddings
    pub code_collection: String,
    /// Collection name for doc embeddings
    pub doc_collection: String,
    /// Collection name for crate documentation embeddings
    pub crate_docs_collection: String,
    /// Collection name for external documentation embeddings
    pub external_docs_collection: String,
}

impl Default for EmbeddingModelConfig {
    fn default() -> Self {
        debug!("EmbeddingModelConfig::default entry");
        let config = Self {
            model: "nomic-embed-text".to_string(),
            dimensions: 768,
            code_collection: "code_embeddings".to_string(),
            doc_collection: "doc_embeddings".to_string(),
            crate_docs_collection: "crate_docs".to_string(),
            external_docs_collection: "external_docs".to_string(),
        };
        debug!(model = %config.model, dimensions = config.dimensions, "EmbeddingModelConfig default created");
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_config_fields() {
        let config = DatabaseConfig {
            postgres_url: "postgresql://user:pass@localhost:5432/db".to_string(),
            neo4j_uri: "bolt://localhost:7687".to_string(),
            neo4j_user: "neo4j".to_string(),
            neo4j_password: "secret".to_string(),
            qdrant_url: "http://localhost:6333".to_string(),
            ollama_url: "http://localhost:11434".to_string(),
        };
        assert!(config.postgres_url.starts_with("postgresql://"));
        assert!(config.neo4j_uri.starts_with("bolt://"));
    }

    #[test]
    fn test_embedding_config_default() {
        let config = EmbeddingModelConfig::default();
        assert_eq!(config.model, "nomic-embed-text");
        assert_eq!(config.dimensions, 768);
        assert_eq!(config.crate_docs_collection, "crate_docs");
        assert_eq!(config.external_docs_collection, "external_docs");
    }
}
