//! Ollama API client for embedding generation
//!
//! Provides HTTP client for Ollama's embedding API with:
//! - Batch processing (max 32 items)
//! - Exponential backoff on rate limiting
//! - Connection pooling

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, warn};

/// Maximum batch size for embedding requests
pub const MAX_BATCH_SIZE: usize = 32;

/// Default embedding model
pub const DEFAULT_MODEL: &str = "nomic-embed-text";

/// Expected embedding dimensions for nomic-embed-text
pub const EXPECTED_DIMENSIONS: usize = 2560;

/// Ollama client configuration
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Base URL for Ollama API (e.g., http://ollama:11434)
    pub base_url: String,
    /// Embedding model to use
    pub model: String,
    /// Maximum batch size
    pub max_batch_size: usize,
    /// Request timeout
    pub timeout: Duration,
    /// Maximum retries for rate limiting
    pub max_retries: u32,
    /// Initial backoff duration
    pub initial_backoff: Duration,
    /// Maximum backoff duration
    pub max_backoff: Duration,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        // Allow timeout and batch size override via environment
        let timeout_secs = std::env::var("OLLAMA_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300); // 5 minutes default for large codebases

        let batch_size = std::env::var("OLLAMA_BATCH_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(MAX_BATCH_SIZE);

        Self {
            base_url: "http://ollama:11434".to_string(),
            model: DEFAULT_MODEL.to_string(),
            max_batch_size: batch_size,
            timeout: Duration::from_secs(timeout_secs),
            max_retries: 5,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// Ollama API client
#[derive(Debug, Clone)]
pub struct OllamaClient {
    config: OllamaConfig,
    client: Client,
}

/// Embedding request for single text
#[derive(Debug, Serialize)]
struct EmbedRequest {
    model: String,
    prompt: String,
}

/// Embedding request for batch of texts
#[derive(Debug, Serialize)]
struct EmbedBatchRequest {
    model: String,
    input: Vec<String>,
}

/// Single embedding response
#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embedding: Vec<f64>,
}

/// Batch embedding response
#[derive(Debug, Deserialize)]
struct EmbedBatchResponse {
    embeddings: Vec<Vec<f64>>,
}

impl OllamaClient {
    /// Create a new Ollama client
    pub fn new(config: OllamaConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Some(Duration::from_secs(30)))
            .build()
            .context("Failed to create HTTP client for Ollama")?;

        Ok(Self { config, client })
    }

    /// Create client with default configuration
    pub fn with_base_url(base_url: String) -> Result<Self> {
        let config = OllamaConfig {
            base_url,
            ..OllamaConfig::default()
        };
        Self::new(config)
    }

    /// Get embedding for a single text
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let request = EmbedRequest {
            model: self.config.model.clone(),
            prompt: text.to_string(),
        };

        let url = format!("{}/api/embeddings", self.config.base_url);

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to send embedding request to Ollama")?;

        let status = response.status();

        if status.is_success() {
            let embed_response: EmbedResponse = response
                .json()
                .await
                .context("Failed to parse Ollama embedding response")?;

            let embedding: Vec<f32> = embed_response
                .embedding
                .into_iter()
                .map(|f| f as f32)
                .collect();

            if embedding.len() != EXPECTED_DIMENSIONS {
                warn!(
                    "Unexpected embedding dimensions: {} (expected {})",
                    embedding.len(),
                    EXPECTED_DIMENSIONS
                );
            }

            Ok(embedding)
        } else {
            let error_body = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama embedding failed: {} - {}", status, error_body)
        }
    }

    /// Get embeddings for a batch of texts with retry logic
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Use the batch embedding endpoint
        let request = EmbedBatchRequest {
            model: self.config.model.clone(),
            input: texts.to_vec(),
        };

        let url = format!("{}/api/embed", self.config.base_url);

        // Retry with exponential backoff
        let mut backoff = self.config.initial_backoff;
        let mut attempt = 0;

        loop {
            attempt += 1;

            let response = self.client.post(&url).json(&request).send().await;

            match response {
                Ok(resp) => {
                    let status = resp.status();

                    // Rate limited or service unavailable
                    if (status.as_u16() == 429 || status.as_u16() == 503)
                        && attempt <= self.config.max_retries
                    {
                        warn!(
                            "Ollama rate limited (attempt {}/{}), backing off for {:?}",
                            attempt, self.config.max_retries, backoff
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(backoff * 2, self.config.max_backoff);
                        continue;
                    }

                    if status.is_success() {
                        let batch_response: EmbedBatchResponse = resp
                            .json()
                            .await
                            .context("Failed to parse Ollama batch embedding response")?;

                        let embeddings: Vec<Vec<f32>> = batch_response
                            .embeddings
                            .into_iter()
                            .map(|e| e.into_iter().map(|f| f as f32).collect())
                            .collect();

                        debug!(
                            "Generated {} embeddings ({} dimensions each)",
                            embeddings.len(),
                            embeddings.first().map(|e| e.len()).unwrap_or(0)
                        );

                        return Ok(embeddings);
                    } else {
                        let error_body = resp.text().await.unwrap_or_default();
                        anyhow::bail!("Ollama batch embedding failed: {} - {}", status, error_body)
                    }
                }
                Err(e) => {
                    if attempt <= self.config.max_retries && e.is_timeout() {
                        warn!(
                            "Ollama request timeout (attempt {}/{}), retrying...",
                            attempt, self.config.max_retries
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = std::cmp::min(backoff * 2, self.config.max_backoff);
                        continue;
                    }
                    anyhow::bail!("Failed to send batch embedding request to Ollama: {}", e)
                }
            }
        }
    }

    /// Process a large number of texts in batches
    pub async fn embed_all(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(self.config.max_batch_size) {
            let batch_embeddings = self.embed_batch(chunk).await?;
            all_embeddings.extend(batch_embeddings);
        }

        Ok(all_embeddings)
    }

    /// Check if Ollama is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/api/tags", self.config.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to Ollama")?;

        Ok(response.status().is_success())
    }

    /// Check if the configured model is available
    pub async fn check_model(&self) -> Result<bool> {
        #[derive(Debug, Deserialize)]
        struct TagsResponse {
            models: Vec<ModelInfo>,
        }

        #[derive(Debug, Deserialize)]
        struct ModelInfo {
            name: String,
        }

        let url = format!("{}/api/tags", self.config.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get model list from Ollama")?;

        if !response.status().is_success() {
            return Ok(false);
        }

        let tags: TagsResponse = response
            .json()
            .await
            .context("Failed to parse Ollama tags response")?;

        // Check if our model is in the list (may have :latest suffix)
        let model_available = tags.models.iter().any(|m| {
            m.name == self.config.model
                || m.name == format!("{}:latest", self.config.model)
                || m.name.starts_with(&format!("{}:", self.config.model))
        });

        if !model_available {
            warn!(
                "Model '{}' not found in Ollama. Available models: {:?}",
                self.config.model,
                tags.models.iter().map(|m| &m.name).collect::<Vec<_>>()
            );
        }

        Ok(model_available)
    }

    /// Get the configured model name
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get expected embedding dimensions
    pub fn dimensions(&self) -> usize {
        EXPECTED_DIMENSIONS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[tokio::test]
    async fn test_embed_success() {
        let mut server = Server::new_async().await;
        let body = r#"{"embedding": [0.1, 0.2, 0.3]}"#;
        let mock = server
            .mock("POST", "/api/embeddings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.embed("hello world").await;

        assert!(result.is_ok());
        let embedding = result.unwrap();
        assert_eq!(embedding.len(), 3);
        assert!((embedding[0] - 0.1_f32).abs() < 1e-5);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_embed_error_status() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/embeddings")
            .with_status(400)
            .with_body("bad request")
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.embed("hello").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("400"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_embed_batch_success() {
        let mut server = Server::new_async().await;
        let body = r#"{"embeddings": [[0.1, 0.2], [0.3, 0.4]]}"#;
        let mock = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let texts = vec!["foo".to_string(), "bar".to_string()];
        let result = client.embed_batch(&texts).await;

        assert!(result.is_ok());
        let embeddings = result.unwrap();
        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0].len(), 2);
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_embed_batch_empty_returns_immediately() {
        // No mock needed — empty input must not make any HTTP call.
        let client = OllamaClient::with_base_url("http://127.0.0.1:1".to_string()).unwrap();
        let result = client.embed_batch(&[]).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_embed_batch_retries_on_429_then_succeeds() {
        let mut server = Server::new_async().await;

        // First call returns 429; once consumed the second mock takes over.
        let mock_429 = server
            .mock("POST", "/api/embed")
            .with_status(429)
            .with_body("rate limited")
            .expect(1)
            .create_async()
            .await;

        let success_body = r#"{"embeddings": [[0.5, 0.6]]}"#;
        let mock_ok = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(success_body)
            .expect(1)
            .create_async()
            .await;

        let client = OllamaClient::new(OllamaConfig {
            base_url: server.url(),
            initial_backoff: Duration::from_millis(1),
            max_retries: 2,
            ..OllamaConfig::default()
        })
        .unwrap();

        let result = client.embed_batch(&["hello".to_string()]).await;
        assert!(result.is_ok());
        let embeddings = result.unwrap();
        assert_eq!(embeddings.len(), 1);
        mock_429.assert_async().await;
        mock_ok.assert_async().await;
    }

    #[tokio::test]
    async fn test_embed_batch_non_retryable_error() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/embed")
            .with_status(500)
            .with_body("internal error")
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.embed_batch(&["text".to_string()]).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("500"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_embed_all_batches_multiple_chunks() {
        let mut server = Server::new_async().await;
        let body = r#"{"embeddings": [[0.1, 0.2]]}"#;

        // Two chunks → two POST calls; each mock consumed once.
        let mock1 = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .expect(1)
            .create_async()
            .await;
        let mock2 = server
            .mock("POST", "/api/embed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .expect(1)
            .create_async()
            .await;

        let client = OllamaClient::new(OllamaConfig {
            base_url: server.url(),
            max_batch_size: 1, // Force two chunks for two texts
            ..OllamaConfig::default()
        })
        .unwrap();

        let texts = vec!["a".to_string(), "b".to_string()];
        let result = client.embed_all(&texts).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
        mock1.assert_async().await;
        mock2.assert_async().await;
    }

    #[tokio::test]
    async fn test_health_check_healthy() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.health_check().await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_health_check_unhealthy() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(503)
            .with_body("unavailable")
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.health_check().await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_model_found() {
        let mut server = Server::new_async().await;
        let body = r#"{"models":[{"name":"nomic-embed-text"}]}"#;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.check_model().await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_model_found_with_latest_suffix() {
        let mut server = Server::new_async().await;
        let body = r#"{"models":[{"name":"nomic-embed-text:latest"}]}"#;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.check_model().await;

        assert!(result.is_ok());
        assert!(result.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_model_not_found() {
        let mut server = Server::new_async().await;
        let body = r#"{"models":[{"name":"llama3"}]}"#;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.check_model().await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_model_api_error() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("GET", "/api/tags")
            .with_status(500)
            .with_body("error")
            .create_async()
            .await;

        let client = OllamaClient::with_base_url(server.url()).unwrap();
        let result = client.check_model().await;

        assert!(result.is_ok());
        assert!(!result.unwrap());
        mock.assert_async().await;
    }

    #[test]
    fn test_config_default() {
        let config = OllamaConfig::default();
        assert_eq!(config.base_url, "http://ollama:11434");
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.max_batch_size, MAX_BATCH_SIZE);
    }

    #[test]
    fn test_expected_dimensions() {
        assert_eq!(EXPECTED_DIMENSIONS, 2560);
    }

    #[test]
    fn test_max_batch_size() {
        assert_eq!(MAX_BATCH_SIZE, 32);
    }

    #[test]
    fn test_default_model() {
        assert_eq!(DEFAULT_MODEL, "nomic-embed-text");
    }

    #[test]
    fn test_client_creation() {
        let client = OllamaClient::new(OllamaConfig::default());
        assert!(client.is_ok());
        let client = client.unwrap();
        assert_eq!(client.model(), DEFAULT_MODEL);
        assert_eq!(client.dimensions(), EXPECTED_DIMENSIONS);
    }

    #[test]
    fn test_client_with_base_url() {
        let client = OllamaClient::with_base_url("http://localhost:11434".to_string());
        assert!(client.is_ok());
    }

    #[test]
    fn test_config_custom_values() {
        let config = OllamaConfig {
            base_url: "http://custom:8080".to_string(),
            model: "custom-model".to_string(),
            max_batch_size: 16,
            timeout: Duration::from_secs(120),
            max_retries: 3,
            initial_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(60),
        };
        assert_eq!(config.base_url, "http://custom:8080");
        assert_eq!(config.model, "custom-model");
        assert_eq!(config.max_batch_size, 16);
        assert_eq!(config.max_retries, 3);
    }
}
