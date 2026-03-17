//! Configuration for the rust-brain API server.

/// Redact password from database/connection URLs for safe logging
pub fn redact_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let scheme_and_user = &url[..colon_pos + 1];
            let rest = &url[at_pos..];
            format!("{}***{}", scheme_and_user, rest)
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub neo4j_uri: String,
    pub neo4j_user: String,
    pub neo4j_password: String,
    pub qdrant_host: String,
    pub ollama_host: String,
    pub embedding_model: String,
    pub embedding_dimensions: usize,
    pub collection_name: String,
    pub chat_model: String,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable must be set"),
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "bolt://neo4j:7687".to_string()),
            neo4j_user: std::env::var("NEO4J_USER")
                .unwrap_or_else(|_| "neo4j".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD")
                .expect("NEO4J_PASSWORD environment variable must be set"),
            qdrant_host: std::env::var("QDRANT_HOST")
                .unwrap_or_else(|_| "http://qdrant:6333".to_string()),
            ollama_host: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://ollama:11434".to_string()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            embedding_dimensions: std::env::var("EMBEDDING_DIMENSIONS")
                .map(|s| s.parse().unwrap_or(768))
                .unwrap_or(768),
            collection_name: std::env::var("QDRANT_COLLECTION")
                .unwrap_or_else(|_| "rust_functions".to_string()),
            chat_model: std::env::var("CHAT_MODEL")
                .unwrap_or_else(|_| "codellama:7b".to_string()),
            port: std::env::var("API_PORT")
                .map(|s| s.parse().unwrap_or(8080))
                .unwrap_or(8080),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_fields() {
        let config = Config {
            database_url: "postgresql://user:pass@localhost:5432/db".to_string(),
            neo4j_uri: "bolt://neo4j:7687".to_string(),
            neo4j_user: "neo4j".to_string(),
            neo4j_password: "secret".to_string(),
            qdrant_host: "http://qdrant:6333".to_string(),
            ollama_host: "http://ollama:11434".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            embedding_dimensions: 768,
            collection_name: "rust_functions".to_string(),
            chat_model: "codellama:7b".to_string(),
            port: 8080,
        };

        assert_eq!(config.embedding_dimensions, 768);
        assert_eq!(config.port, 8080);
        assert_eq!(config.embedding_model, "nomic-embed-text");
    }
}
