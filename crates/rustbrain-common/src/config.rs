//! Shared configuration types for rust-brain services

use serde::{Deserialize, Serialize};

/// Database connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL
    pub postgres_url: String,
    /// Neo4j Bolt URI (e.g., bolt://neo4j:7687)
    pub neo4j_uri: String,
    /// Neo4j username
    pub neo4j_user: String,
    /// Neo4j password
    pub neo4j_password: String,
    /// Qdrant REST API URL (e.g., http://qdrant:6333)
    pub qdrant_url: String,
    /// Ollama API URL (e.g., http://ollama:11434)
    pub ollama_url: String,
}

impl DatabaseConfig {
    /// Load from environment variables.
    /// Panics with a clear message if required credentials are missing.
    pub fn from_env() -> Self {
        Self {
            postgres_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable must be set"),
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "bolt://neo4j:7687".to_string()),
            neo4j_user: std::env::var("NEO4J_USER")
                .unwrap_or_else(|_| "neo4j".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD")
                .expect("NEO4J_PASSWORD environment variable must be set"),
            qdrant_url: std::env::var("QDRANT_HOST")
                .unwrap_or_else(|_| "http://qdrant:6333".to_string()),
            ollama_url: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://ollama:11434".to_string()),
        }
    }
}

/// Embedding configuration
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
}

impl Default for EmbeddingModelConfig {
    fn default() -> Self {
        Self {
            model: "nomic-embed-text".to_string(),
            dimensions: 768,
            code_collection: "code_embeddings".to_string(),
            doc_collection: "doc_embeddings".to_string(),
        }
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
    }
}
