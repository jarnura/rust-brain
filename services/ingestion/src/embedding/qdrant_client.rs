//! Qdrant API client for vector storage
//!
//! Provides HTTP client for Qdrant's REST API with:
//! - Collection management
//! - Point upsert (idempotent)
//! - Batch operations
//! - Search functionality

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Collection name for code embeddings
pub const CODE_COLLECTION: &str = "code_embeddings";

/// Collection name for documentation embeddings
pub const DOC_COLLECTION: &str = "doc_embeddings";

/// Qdrant client configuration
#[derive(Debug, Clone)]
pub struct QdrantConfig {
    /// Base URL for Qdrant REST API (e.g., http://qdrant:6333)
    pub base_url: String,
    /// Code embeddings collection name
    pub code_collection: String,
    /// Doc embeddings collection name
    pub doc_collection: String,
    /// Vector dimensions
    pub vector_size: usize,
    /// Request timeout
    pub timeout: Duration,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            base_url: "http://qdrant:6333".to_string(),
            code_collection: CODE_COLLECTION.to_string(),
            doc_collection: DOC_COLLECTION.to_string(),
            vector_size: 768,
            timeout: Duration::from_secs(30),
        }
    }
}

/// Qdrant API client
#[derive(Debug, Clone)]
pub struct QdrantClient {
    config: QdrantConfig,
    client: Client,
}

/// Point to upsert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    /// Point ID (UUID)
    pub id: Uuid,
    /// Vector data
    pub vector: Vec<f32>,
    /// Payload metadata
    pub payload: HashMap<String, PayloadValue>,
}

/// Payload value types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PayloadValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<PayloadValue>),
    Null,
}

impl From<String> for PayloadValue {
    fn from(s: String) -> Self {
        PayloadValue::String(s)
    }
}

impl From<&str> for PayloadValue {
    fn from(s: &str) -> Self {
        PayloadValue::String(s.to_string())
    }
}

impl From<i64> for PayloadValue {
    fn from(i: i64) -> Self {
        PayloadValue::Integer(i)
    }
}

impl From<i32> for PayloadValue {
    fn from(i: i32) -> Self {
        PayloadValue::Integer(i as i64)
    }
}

impl From<usize> for PayloadValue {
    fn from(i: usize) -> Self {
        PayloadValue::Integer(i as i64)
    }
}

impl From<bool> for PayloadValue {
    fn from(b: bool) -> Self {
        PayloadValue::Boolean(b)
    }
}

impl From<f64> for PayloadValue {
    fn from(f: f64) -> Self {
        PayloadValue::Float(f)
    }
}

/// Upsert request
#[derive(Debug, Serialize)]
struct UpsertRequest {
    points: Vec<PointStruct>,
}

/// Point structure for API
#[derive(Debug, Serialize)]
struct PointStruct {
    id: String,
    vector: Vec<f32>,
    payload: HashMap<String, PayloadValue>,
}

/// Search request
#[derive(Debug, Serialize)]
pub struct SearchRequest {
    pub vector: Vec<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_threshold: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<Filter>,
    pub with_payload: bool,
}

/// Filter for search
#[derive(Debug, Serialize)]
pub struct Filter {
    pub must: Vec<Condition>,
}

/// Condition for filter
#[derive(Debug, Serialize)]
pub struct Condition {
    pub key: String,
    pub match_: Match,
}

/// Match type
#[derive(Debug, Serialize)]
pub struct Match {
    pub value: PayloadValue,
}

/// Search response
#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub result: Vec<SearchResult>,
}

/// Single search result
#[derive(Debug, Deserialize)]
pub struct SearchResult {
    pub id: String,
    pub score: f32,
    pub payload: HashMap<String, PayloadValue>,
}

/// Collection info response
#[derive(Debug, Deserialize)]
struct CollectionInfoResponse {
    result: CollectionInfo,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CollectionInfo {
    pub status: String,
    pub points_count: usize,
    pub vectors_count: usize,
}

/// Create collection request
#[derive(Debug, Serialize)]
struct CreateCollectionRequest {
    vectors: VectorsConfig,
}

#[derive(Debug, Serialize)]
struct VectorsConfig {
    size: usize,
    distance: String,
}

/// Collection existence response
#[derive(Debug, Deserialize)]
struct CollectionsResponse {
    collections: Vec<CollectionDescription>,
}

#[derive(Debug, Deserialize)]
struct CollectionDescription {
    name: String,
}

impl QdrantClient {
    /// Create a new Qdrant client
    pub fn new(config: QdrantConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.timeout)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Some(Duration::from_secs(30)))
            .build()
            .context("Failed to create HTTP client for Qdrant")?;
        
        Ok(Self { config, client })
    }
    
    /// Create client with default configuration
    pub fn with_base_url(base_url: String) -> Result<Self> {
        let mut config = QdrantConfig::default();
        config.base_url = base_url;
        Self::new(config)
    }
    
    /// Check if Qdrant is healthy
    pub async fn health_check(&self) -> Result<bool> {
        let url = format!("{}/healthz", self.config.base_url);
        
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to connect to Qdrant")?;
        
        Ok(response.status().is_success())
    }
    
    /// List all collections
    pub async fn list_collections(&self) -> Result<Vec<String>> {
        let url = format!("{}/collections", self.config.base_url);
        
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to list Qdrant collections")?;
        
        if !response.status().is_success() {
            anyhow::bail!("Failed to list collections: {}", response.status());
        }
        
        let collections: CollectionsResponse = response
            .json()
            .await
            .context("Failed to parse collections response")?;
        
        Ok(collections.collections.into_iter().map(|c| c.name).collect())
    }
    
    /// Create a collection if it doesn't exist
    pub async fn ensure_collection(&self, collection: &str) -> Result<()> {
        let collections = self.list_collections().await?;
        
        if collections.contains(&collection.to_string()) {
            debug!("Collection '{}' already exists", collection);
            return Ok(());
        }
        
        info!("Creating collection '{}' with vector size {}", collection, self.config.vector_size);
        
        let url = format!("{}/collections/{}", self.config.base_url, collection);
        
        let request = CreateCollectionRequest {
            vectors: VectorsConfig {
                size: self.config.vector_size,
                distance: "Cosine".to_string(),
            },
        };
        
        let response = self
            .client
            .put(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to create Qdrant collection")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to create collection '{}': {}", collection, body);
        }
        
        info!("Created collection '{}'", collection);
        Ok(())
    }
    
    /// Ensure both code and doc collections exist
    pub async fn ensure_collections(&self) -> Result<()> {
        self.ensure_collection(&self.config.code_collection).await?;
        self.ensure_collection(&self.config.doc_collection).await?;
        Ok(())
    }
    
    /// Get collection info
    pub async fn get_collection_info(&self, collection: &str) -> Result<CollectionInfo> {
        let url = format!("{}/collections/{}", self.config.base_url, collection);
        
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get collection info")?;
        
        if !response.status().is_success() {
            anyhow::bail!("Collection '{}' not found", collection);
        }
        
        let info: CollectionInfoResponse = response
            .json()
            .await
            .context("Failed to parse collection info")?;
        
        Ok(info.result)
    }
    
    /// Upsert a single point (idempotent)
    pub async fn upsert_point(&self, collection: &str, point: Point) -> Result<()> {
        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.config.base_url, collection
        );
        
        let point_struct = PointStruct {
            id: point.id.to_string(),
            vector: point.vector,
            payload: point.payload,
        };
        
        let request = UpsertRequest {
            points: vec![point_struct],
        };
        
        let response = self
            .client
            .put(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to upsert point to Qdrant")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to upsert point: {}", body);
        }
        
        Ok(())
    }
    
    /// Upsert multiple points in batch (idempotent)
    pub async fn upsert_points(&self, collection: &str, points: Vec<Point>) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        
        let url = format!(
            "{}/collections/{}/points?wait=true",
            self.config.base_url, collection
        );
        
        let point_structs: Vec<PointStruct> = points
            .into_iter()
            .map(|p| PointStruct {
                id: p.id.to_string(),
                vector: p.vector,
                payload: p.payload,
            })
            .collect();
        
        let request = UpsertRequest {
            points: point_structs,
        };
        
        let response = self
            .client
            .put(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to upsert points to Qdrant")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to upsert points: {}", body);
        }
        
        debug!("Upserted batch of points to collection '{}'", collection);
        Ok(())
    }
    
    /// Search for similar vectors
    pub async fn search(
        &self,
        collection: &str,
        request: SearchRequest,
    ) -> Result<Vec<SearchResult>> {
        let url = format!(
            "{}/collections/{}/points/search",
            self.config.base_url, collection
        );
        
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to search Qdrant")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Search failed: {}", body);
        }
        
        let search_response: SearchResponse = response
            .json()
            .await
            .context("Failed to parse search response")?;
        
        Ok(search_response.result)
    }
    
    /// Delete a point by ID
    pub async fn delete_point(&self, collection: &str, id: Uuid) -> Result<()> {
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.config.base_url, collection
        );
        
        #[derive(Debug, Serialize)]
        struct DeleteRequest {
            points: Vec<String>,
        }
        
        let request = DeleteRequest {
            points: vec![id.to_string()],
        };
        
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to delete point from Qdrant")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to delete point: {}", body);
        }
        
        Ok(())
    }
    
    /// Delete points by filter
    pub async fn delete_by_filter(&self, collection: &str, filter: Filter) -> Result<()> {
        let url = format!(
            "{}/collections/{}/points/delete?wait=true",
            self.config.base_url, collection
        );
        
        #[derive(Debug, Serialize)]
        struct DeleteByFilterRequest {
            filter: Filter,
        }
        
        let request = DeleteByFilterRequest { filter };
        
        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to delete points by filter from Qdrant")?;
        
        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Failed to delete points by filter: {}", body);
        }
        
        Ok(())
    }
    
    /// Get the code collection name
    pub fn code_collection(&self) -> &str {
        &self.config.code_collection
    }
    
    /// Get the doc collection name
    pub fn doc_collection(&self) -> &str {
        &self.config.doc_collection
    }
    
    /// Get vector size
    pub fn vector_size(&self) -> usize {
        self.config.vector_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_default() {
        let config = QdrantConfig::default();
        assert_eq!(config.base_url, "http://qdrant:6333");
        assert_eq!(config.code_collection, CODE_COLLECTION);
        assert_eq!(config.doc_collection, DOC_COLLECTION);
        assert_eq!(config.vector_size, 768);
    }
    
    #[test]
    fn test_payload_value_from_string() {
        let v: PayloadValue = "test".into();
        match v {
            PayloadValue::String(s) => assert_eq!(s, "test"),
            _ => panic!("Expected String variant"),
        }
    }
    
    #[test]
    fn test_payload_value_from_int() {
        let v: PayloadValue = 42i64.into();
        match v {
            PayloadValue::Integer(i) => assert_eq!(i, 42),
            _ => panic!("Expected Integer variant"),
        }
    }
}
