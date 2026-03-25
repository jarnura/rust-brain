//! Embedding service for rust-brain
//!
//! Provides vector embedding generation and storage for Rust code items:
//! - Text representation generation for semantic embedding
//! - Ollama integration for embedding generation
//! - Qdrant integration for vector storage
//! - Batch processing for large datasets
//! - Idempotent re-embedding via upsert

pub mod ollama_client;
pub mod qdrant_client;
pub mod text_representation;

use anyhow::{Context, Result};
use ollama_client::{OllamaClient, OllamaConfig};
use qdrant_client::{QdrantClient, QdrantConfig, Point, PayloadValue, SearchRequest};
use std::collections::HashMap;
use std::sync::Arc;
use text_representation::{generate_text_representation, extract_doc_chunks, DocChunk};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::parsers::ParsedItem;

/// Maximum chunk size for doc embedding
const MAX_DOC_CHUNK_SIZE: usize = 500;

/// Embedding service configuration
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Ollama configuration
    pub ollama: OllamaConfig,
    /// Qdrant configuration  
    pub qdrant: QdrantConfig,
    /// Maximum doc chunk size in characters
    pub max_doc_chunk_size: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            ollama: OllamaConfig::default(),
            qdrant: QdrantConfig::default(),
            max_doc_chunk_size: MAX_DOC_CHUNK_SIZE,
        }
    }
}

/// Result of embedding an item
#[derive(Debug)]
pub struct EmbeddedItem {
    /// Original item FQN
    pub fqn: String,
    /// Item type
    pub item_type: String,
    /// Point ID in Qdrant
    pub point_id: Uuid,
    /// Collection stored in
    pub collection: String,
}

/// Result of embedding doc chunks
#[derive(Debug)]
pub struct EmbeddedDoc {
    /// Source item FQN
    pub source_fqn: String,
    /// Source item type
    pub source_item_type: String,
    /// Point ID in Qdrant
    pub point_id: Uuid,
    /// Chunk index
    pub chunk_index: usize,
}

/// Embedding service for generating and storing vector embeddings
#[derive(Clone)]
pub struct EmbeddingService {
    ollama: Arc<OllamaClient>,
    qdrant: Arc<QdrantClient>,
    config: EmbeddingConfig,
}

impl EmbeddingService {
    /// Create a new embedding service
    pub fn new(config: EmbeddingConfig) -> Result<Self> {
        let ollama = OllamaClient::new(config.ollama.clone())
            .context("Failed to create Ollama client")?;
        
        let qdrant = QdrantClient::new(config.qdrant.clone())
            .context("Failed to create Qdrant client")?;
        
        Ok(Self {
            ollama: Arc::new(ollama),
            qdrant: Arc::new(qdrant),
            config,
        })
    }
    
    /// Create service with default configuration and custom URLs
    pub fn with_urls(ollama_url: String, qdrant_url: String) -> Result<Self> {
        let mut config = EmbeddingConfig::default();
        config.ollama.base_url = ollama_url;
        config.qdrant.base_url = qdrant_url;
        Self::new(config)
    }
    
    /// Initialize the service (ensure collections exist, check model)
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing embedding service...");
        
        // Check Ollama health and model
        let ollama_healthy = self.ollama.health_check().await
            .context("Ollama health check failed")?;
        
        if !ollama_healthy {
            anyhow::bail!("Ollama is not healthy");
        }
        
        let model_available = self.ollama.check_model().await?;
        if !model_available {
            warn!(
                "Embedding model '{}' may not be available. Run: ollama pull {}",
                self.ollama.model(),
                self.ollama.model()
            );
        }
        
        // Ensure Qdrant collections exist
        self.qdrant.ensure_collections().await
            .context("Failed to create Qdrant collections")?;
        
        info!("Embedding service initialized successfully");
        Ok(())
    }
    
    /// Embed a single parsed item
    pub async fn embed_item(&self, item: &ParsedItem) -> Result<EmbeddedItem> {
        // Generate text representation
        let text_rep = generate_text_representation(item);
        
        // Get embedding from Ollama
        let embedding = self.ollama.embed(&text_rep.text).await
            .context("Failed to generate embedding")?;
        
        // Create point ID from item FQN (deterministic for idempotency)
        let point_id = self.fqn_to_point_id(&item.fqn);
        
        // Build payload
        let payload = self.build_item_payload(item, &text_rep.text);
        
        // Upsert to Qdrant
        let point = Point {
            id: point_id,
            vector: embedding,
            payload,
        };
        
        self.qdrant
            .upsert_point(self.qdrant.code_collection(), point)
            .await
            .context("Failed to upsert embedding to Qdrant")?;
        
        debug!("Embedded item: {} -> {}", item.fqn, point_id);
        
        Ok(EmbeddedItem {
            fqn: item.fqn.clone(),
            item_type: text_rep.item_type,
            point_id,
            collection: self.qdrant.code_collection().to_string(),
        })
    }
    
    /// Embed multiple items in batch
    pub async fn embed_items(&self, items: &[ParsedItem]) -> Result<Vec<EmbeddedItem>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        
        info!("Embedding {} items...", items.len());
        
        // Generate all text representations
        let texts: Vec<String> = items
            .iter()
            .map(|item| {
                let rep = generate_text_representation(item);
                rep.text
            })
            .collect();
        
        // Get embeddings in batch
        let embeddings = self.ollama.embed_all(&texts).await
            .context("Failed to generate batch embeddings")?;
        
        // Create points
        let mut points = Vec::with_capacity(items.len());
        let mut results = Vec::with_capacity(items.len());
        
        for (item, embedding) in items.iter().zip(embeddings.into_iter()) {
            let text_rep = generate_text_representation(item);
            let point_id = self.fqn_to_point_id(&item.fqn);
            let payload = self.build_item_payload(item, &text_rep.text);
            
            points.push(Point {
                id: point_id,
                vector: embedding,
                payload,
            });
            
            results.push(EmbeddedItem {
                fqn: item.fqn.clone(),
                item_type: text_rep.item_type,
                point_id,
                collection: self.qdrant.code_collection().to_string(),
            });
        }
        
        // Batch upsert to Qdrant
        self.qdrant
            .upsert_points(self.qdrant.code_collection(), points)
            .await
            .context("Failed to upsert batch embeddings to Qdrant")?;
        
        info!("Embedded {} items successfully", results.len());
        Ok(results)
    }
    
    /// Embed documentation chunks for an item
    pub async fn embed_doc_chunks(&self, item: &ParsedItem) -> Result<Vec<EmbeddedDoc>> {
        // Extract doc chunks
        let chunks = extract_doc_chunks(item, self.config.max_doc_chunk_size);
        
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        
        // Get embeddings for all chunks
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let embeddings = self.ollama.embed_all(&texts).await
            .context("Failed to generate doc chunk embeddings")?;
        
        let mut results = Vec::with_capacity(chunks.len());
        let mut points = Vec::with_capacity(chunks.len());
        
        for (chunk, embedding) in chunks.into_iter().zip(embeddings.into_iter()) {
            let point_id = self.doc_chunk_to_point_id(&chunk);
            let payload = self.build_doc_payload(&chunk);
            
            points.push(Point {
                id: point_id,
                vector: embedding,
                payload,
            });
            
            results.push(EmbeddedDoc {
                source_fqn: chunk.source_fqn,
                source_item_type: chunk.source_item_type,
                point_id,
                chunk_index: chunk.chunk_index,
            });
        }
        
        // Batch upsert to doc collection
        self.qdrant
            .upsert_points(self.qdrant.doc_collection(), points)
            .await
            .context("Failed to upsert doc embeddings to Qdrant")?;
        
        debug!("Embedded {} doc chunks for {}", results.len(), item.fqn);
        Ok(results)
    }
    
    /// Embed item and its documentation chunks
    pub async fn embed_item_with_docs(&self, item: &ParsedItem) -> Result<(EmbeddedItem, Vec<EmbeddedDoc>)> {
        let embedded_item = self.embed_item(item).await?;
        let embedded_docs = self.embed_doc_chunks(item).await?;
        Ok((embedded_item, embedded_docs))
    }
    
    /// Process a large batch of items (handles 1000+ items)
    pub async fn embed_batch(&self, items: &[ParsedItem]) -> Result<Vec<EmbeddedItem>> {
        const BATCH_SIZE: usize = 100;
        
        let mut all_results = Vec::with_capacity(items.len());
        let total = items.len();
        
        for (batch_num, chunk) in items.chunks(BATCH_SIZE).enumerate() {
            debug!("Processing batch {}/{} ({} items)", 
                batch_num + 1, 
                (total + BATCH_SIZE - 1) / BATCH_SIZE,
                chunk.len()
            );
            
            match self.embed_items(chunk).await {
                Ok(results) => all_results.extend(results),
                Err(e) => {
                    error!("Failed to embed batch {}: {}", batch_num, e);
                    // Continue with remaining batches
                }
            }
        }
        
        Ok(all_results)
    }
    
    /// Search for similar code items
    pub async fn search_code(
        &self,
        query: &str,
        limit: usize,
        score_threshold: Option<f32>,
        crate_filter: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        // Get embedding for query
        let embedding = self.ollama.embed(query).await
            .context("Failed to generate query embedding")?;
        
        // Build search request
        let mut request = SearchRequest {
            vector: embedding,
            limit: Some(limit),
            score_threshold,
            filter: None,
            with_payload: true,
        };
        
        // Add crate filter if specified
        if let Some(crate_name) = crate_filter {
            request.filter = Some(qdrant_client::Filter {
                must: vec![qdrant_client::Condition {
                    key: "crate_name".to_string(),
                    match_: qdrant_client::Match {
                        value: PayloadValue::from(crate_name),
                    },
                }],
            });
        }
        
        // Search in code collection
        let results = self.qdrant
            .search(self.qdrant.code_collection(), request)
            .await
            .context("Failed to search code embeddings")?;
        
        Ok(results.into_iter().map(|r| r.into()).collect())
    }
    
    /// Search for similar documentation
    pub async fn search_docs(
        &self,
        query: &str,
        limit: usize,
        score_threshold: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        // Get embedding for query
        let embedding = self.ollama.embed(query).await
            .context("Failed to generate query embedding")?;
        
        let request = SearchRequest {
            vector: embedding,
            limit: Some(limit),
            score_threshold,
            filter: None,
            with_payload: true,
        };
        
        // Search in doc collection
        let results = self.qdrant
            .search(self.qdrant.doc_collection(), request)
            .await
            .context("Failed to search doc embeddings")?;
        
        Ok(results.into_iter().map(|r| r.into()).collect())
    }
    
    /// Get collection statistics
    pub async fn get_stats(&self) -> Result<EmbeddingStats> {
        let code_info = self.qdrant.get_collection_info(self.qdrant.code_collection()).await
            .context("Failed to get code collection info")?;
        
        let doc_info = self.qdrant.get_collection_info(self.qdrant.doc_collection()).await
            .ok(); // Don't fail if doc collection doesn't exist
        
        Ok(EmbeddingStats {
            code_points: code_info.points_count,
            doc_points: doc_info.map(|i| i.points_count).unwrap_or(0),
            vector_dimensions: self.ollama.dimensions(),
            model: self.ollama.model().to_string(),
        })
    }
    
    // =========================================================================
    // Helper Methods
    // =========================================================================
    
    /// Convert FQN to a deterministic point ID (UUID v5)
    fn fqn_to_point_id(&self, fqn: &str) -> Uuid {
        // Use UUID v5 with a namespace for deterministic IDs
        // This ensures re-embedding is idempotent
        let namespace = Uuid::parse_str("6ba7b810-9dad-11d1-80b4-00c04fd430c8") // UUID namespace DNS
            .expect("Invalid namespace UUID");
        Uuid::new_v5(&namespace, fqn.as_bytes())
    }
    
    /// Convert doc chunk to a deterministic point ID
    fn doc_chunk_to_point_id(&self, chunk: &DocChunk) -> Uuid {
        // Include chunk index in the ID
        let key = format!("{}:doc:{}", chunk.source_fqn, chunk.chunk_index);
        self.fqn_to_point_id(&key)
    }
    
    /// Build payload for a code item
    fn build_item_payload(&self, item: &ParsedItem, text: &str) -> HashMap<String, PayloadValue> {
        let mut payload = HashMap::new();
        
        payload.insert("fqn".to_string(), PayloadValue::from(item.fqn.as_str()));
        payload.insert("name".to_string(), PayloadValue::from(item.name.as_str()));
        payload.insert("item_type".to_string(), PayloadValue::from(item.item_type.as_str()));
        payload.insert("visibility".to_string(), PayloadValue::from(item.visibility.as_str()));
        
        // Extract crate and module from FQN
        let parts: Vec<&str> = item.fqn.split("::").collect();
        if !parts.is_empty() {
            payload.insert("crate_name".to_string(), PayloadValue::from(parts[0]));
            // Module path is everything except the last component (item name)
            if parts.len() > 1 {
                let module_path = parts[..parts.len() - 1].join("::");
                payload.insert("module_path".to_string(), PayloadValue::from(module_path));
            }
        }
        
        payload.insert("signature".to_string(), PayloadValue::from(item.signature.as_str()));
        
        // Store doc comment (truncated if too long)
        let doc_preview = if item.doc_comment.len() > 500 {
            format!("{}...", &item.doc_comment[..500])
        } else {
            item.doc_comment.clone()
        };
        payload.insert("doc_comment".to_string(), PayloadValue::from(doc_preview));
        
        // Location info
        payload.insert("start_line".to_string(), PayloadValue::from(item.start_line as i64));
        payload.insert("end_line".to_string(), PayloadValue::from(item.end_line as i64));
        
        // Store text representation for reference
        let text_preview = if text.len() > 1000 {
            format!("{}...", &text[..1000])
        } else {
            text.to_string()
        };
        payload.insert("text_preview".to_string(), PayloadValue::from(text_preview));
        
        // Generic parameters info
        let has_generics = !item.generic_params.is_empty();
        payload.insert("has_generics".to_string(), PayloadValue::from(has_generics));
        
        if !item.generic_params.is_empty() {
            if let Ok(generics_json) = serde_json::to_string(&item.generic_params) {
                payload.insert("generic_params".to_string(), PayloadValue::from(generics_json));
            }
        }
        
        // Where clauses / trait bounds
        if !item.where_clauses.is_empty() {
            // Extract trait bounds from where clauses
            let trait_bounds: Vec<String> = item.where_clauses
                .iter()
                .flat_map(|wc| wc.bounds.iter().cloned())
                .collect();
            
            if !trait_bounds.is_empty() {
                if let Ok(bounds_json) = serde_json::to_string(&trait_bounds) {
                    payload.insert("trait_bounds".to_string(), PayloadValue::from(bounds_json));
                }
            }
            
            // Also store full where clauses
            if let Ok(wc_json) = serde_json::to_string(&item.where_clauses) {
                payload.insert("where_clauses".to_string(), PayloadValue::from(wc_json));
            }
        }
        
        // Attributes as JSON
        if !item.attributes.is_empty() {
            if let Ok(attrs_json) = serde_json::to_string(&item.attributes) {
                payload.insert("attributes".to_string(), PayloadValue::from(attrs_json));
            }
        }
        
        payload
    }
    
    /// Build payload for a doc chunk
    fn build_doc_payload(&self, chunk: &DocChunk) -> HashMap<String, PayloadValue> {
        let mut payload = HashMap::new();
        
        payload.insert("source_fqn".to_string(), PayloadValue::from(chunk.source_fqn.as_str()));
        payload.insert("source_item_type".to_string(), PayloadValue::from(chunk.source_item_type.as_str()));
        payload.insert("chunk_index".to_string(), PayloadValue::from(chunk.chunk_index as i64));
        payload.insert("text".to_string(), PayloadValue::from(chunk.text.as_str()));
        
        // Extract crate from source FQN
        let parts: Vec<&str> = chunk.source_fqn.split("::").collect();
        if !parts.is_empty() {
            payload.insert("crate_name".to_string(), PayloadValue::from(parts[0]));
        }
        
        payload
    }
}

/// Statistics about embeddings
#[derive(Debug, Clone)]
pub struct EmbeddingStats {
    /// Number of code embedding points
    pub code_points: usize,
    /// Number of doc embedding points
    pub doc_points: usize,
    /// Vector dimensions
    pub vector_dimensions: usize,
    /// Embedding model name
    pub model: String,
}

/// Search result with parsed payload
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Score (0-1)
    pub score: f32,
    /// Item FQN
    pub fqn: String,
    /// Item name
    pub name: String,
    /// Item type
    pub item_type: String,
    /// Crate name
    pub crate_name: String,
    /// Start line
    pub start_line: i64,
    /// End line
    pub end_line: i64,
    /// Signature
    pub signature: String,
    /// Doc comment
    pub doc_comment: Option<String>,
}

impl From<qdrant_client::SearchResult> for SearchResult {
    fn from(result: qdrant_client::SearchResult) -> Self {
        let payload = result.payload;
        
        SearchResult {
            score: result.score,
            fqn: payload.get("fqn")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            name: payload.get("name")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            item_type: payload.get("item_type")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            crate_name: payload.get("crate_name")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            start_line: payload.get("start_line")
                .and_then(|v| match v {
                    PayloadValue::Integer(i) => Some(*i),
                    _ => None,
                })
                .unwrap_or(0),
            end_line: payload.get("end_line")
                .and_then(|v| match v {
                    PayloadValue::Integer(i) => Some(*i),
                    _ => None,
                })
                .unwrap_or(0),
            signature: payload.get("signature")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .unwrap_or_default(),
            doc_comment: payload.get("doc_comment")
                .and_then(|v| match v {
                    PayloadValue::String(s) => Some(s.clone()),
                    _ => None,
                }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::{ItemType, Visibility};
    
    #[test]
    fn test_fqn_to_point_id_deterministic() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        
        let id1 = service.fqn_to_point_id("crate::module::function");
        let id2 = service.fqn_to_point_id("crate::module::function");
        
        assert_eq!(id1, id2, "Point IDs should be deterministic");
    }
    
    #[test]
    fn test_different_fqns_different_ids() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        
        let id1 = service.fqn_to_point_id("crate::module::function1");
        let id2 = service.fqn_to_point_id("crate::module::function2");
        
        assert_ne!(id1, id2, "Different FQNs should have different IDs");
    }
    
    fn make_test_item() -> ParsedItem {
        ParsedItem {
            fqn: "test_crate::module::test_fn".to_string(),
            item_type: ItemType::Function,
            name: "test_fn".to_string(),
            visibility: Visibility::Public,
            signature: "pub fn test_fn(x: i32) -> bool".to_string(),
            generic_params: vec![],
            where_clauses: vec![],
            attributes: vec![],
            doc_comment: "A test function.\n\nThis function does something useful.".to_string(),
            start_line: 10,
            end_line: 15,
            body_source: "pub fn test_fn(x: i32) -> bool { x > 0 }".to_string(),
            generated_by: None,
        }
    }
    
    #[test]
    fn test_build_item_payload() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();
        
        let item = make_test_item();
        let payload = service.build_item_payload(&item, "test text");
        
        assert!(matches!(payload.get("fqn"), Some(PayloadValue::String(s)) if s == "test_crate::module::test_fn"));
        assert!(matches!(payload.get("name"), Some(PayloadValue::String(s)) if s == "test_fn"));
        assert!(matches!(payload.get("item_type"), Some(PayloadValue::String(s)) if s == "function"));
        assert!(matches!(payload.get("crate_name"), Some(PayloadValue::String(s)) if s == "test_crate"));
    }

    #[test]
    fn test_build_item_payload_has_module_path() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let item = make_test_item();
        let payload = service.build_item_payload(&item, "test text");

        assert!(matches!(payload.get("module_path"), Some(PayloadValue::String(s)) if s == "test_crate::module"));
        assert!(matches!(payload.get("signature"), Some(PayloadValue::String(s)) if s.contains("test_fn")));
        assert!(matches!(payload.get("start_line"), Some(PayloadValue::Integer(10))));
        assert!(matches!(payload.get("end_line"), Some(PayloadValue::Integer(15))));
        assert!(matches!(payload.get("has_generics"), Some(PayloadValue::Boolean(false))));
    }

    #[test]
    fn test_build_item_payload_with_generics() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let mut item = make_test_item();
        item.generic_params = vec![crate::parsers::GenericParam {
            name: "T".to_string(),
            kind: "type".to_string(),
            bounds: vec!["Clone".to_string()],
            default: None,
        }];

        let payload = service.build_item_payload(&item, "text");
        assert!(matches!(payload.get("has_generics"), Some(PayloadValue::Boolean(true))));
        assert!(payload.contains_key("generic_params"));
    }

    #[test]
    fn test_build_item_payload_with_where_clauses() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let mut item = make_test_item();
        item.where_clauses = vec![crate::parsers::WhereClause {
            subject: "T".to_string(),
            bounds: vec!["Send".to_string(), "Sync".to_string()],
        }];

        let payload = service.build_item_payload(&item, "text");
        assert!(payload.contains_key("trait_bounds"));
        assert!(payload.contains_key("where_clauses"));
    }

    #[test]
    fn test_build_doc_payload() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let chunk = text_representation::DocChunk {
            text: "This is doc text".to_string(),
            source_fqn: "my_crate::module::item".to_string(),
            source_item_type: "function".to_string(),
            chunk_index: 0,
        };

        let payload = service.build_doc_payload(&chunk);
        assert!(matches!(payload.get("source_fqn"), Some(PayloadValue::String(s)) if s == "my_crate::module::item"));
        assert!(matches!(payload.get("crate_name"), Some(PayloadValue::String(s)) if s == "my_crate"));
        assert!(matches!(payload.get("chunk_index"), Some(PayloadValue::Integer(0))));
        assert!(matches!(payload.get("text"), Some(PayloadValue::String(s)) if s == "This is doc text"));
    }

    #[test]
    fn test_doc_chunk_point_id_deterministic() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let chunk = text_representation::DocChunk {
            text: "text".to_string(),
            source_fqn: "crate::fn".to_string(),
            source_item_type: "function".to_string(),
            chunk_index: 0,
        };

        let id1 = service.doc_chunk_to_point_id(&chunk);
        let id2 = service.doc_chunk_to_point_id(&chunk);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_doc_chunk_different_indices_different_ids() {
        let config = EmbeddingConfig::default();
        let service = EmbeddingService::new(config).unwrap();

        let chunk0 = text_representation::DocChunk {
            text: "text".to_string(),
            source_fqn: "crate::fn".to_string(),
            source_item_type: "function".to_string(),
            chunk_index: 0,
        };
        let chunk1 = text_representation::DocChunk {
            text: "text".to_string(),
            source_fqn: "crate::fn".to_string(),
            source_item_type: "function".to_string(),
            chunk_index: 1,
        };

        assert_ne!(
            service.doc_chunk_to_point_id(&chunk0),
            service.doc_chunk_to_point_id(&chunk1)
        );
    }

    #[test]
    fn test_search_result_from_qdrant_result() {
        let mut payload = HashMap::new();
        payload.insert("fqn".to_string(), PayloadValue::from("crate::my_fn"));
        payload.insert("name".to_string(), PayloadValue::from("my_fn"));
        payload.insert("item_type".to_string(), PayloadValue::from("function"));
        payload.insert("crate_name".to_string(), PayloadValue::from("crate"));
        payload.insert("start_line".to_string(), PayloadValue::Integer(5));
        payload.insert("end_line".to_string(), PayloadValue::Integer(10));
        payload.insert("signature".to_string(), PayloadValue::from("fn my_fn()"));
        payload.insert("doc_comment".to_string(), PayloadValue::from("A function"));

        let qdrant_result = qdrant_client::SearchResult {
            id: "some-id".to_string(),
            score: 0.95,
            payload,
        };

        let result: SearchResult = qdrant_result.into();
        assert_eq!(result.fqn, "crate::my_fn");
        assert_eq!(result.name, "my_fn");
        assert_eq!(result.item_type, "function");
        assert_eq!(result.crate_name, "crate");
        assert_eq!(result.start_line, 5);
        assert_eq!(result.end_line, 10);
        assert_eq!(result.signature, "fn my_fn()");
        assert_eq!(result.doc_comment, Some("A function".to_string()));
        assert!((result.score - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn test_search_result_from_missing_payload_fields() {
        let payload = HashMap::new();
        let qdrant_result = qdrant_client::SearchResult {
            id: "id".to_string(),
            score: 0.5,
            payload,
        };

        let result: SearchResult = qdrant_result.into();
        assert_eq!(result.fqn, "");
        assert_eq!(result.name, "");
        assert_eq!(result.start_line, 0);
        assert!(result.doc_comment.is_none());
    }

    #[test]
    fn test_embedding_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.max_doc_chunk_size, MAX_DOC_CHUNK_SIZE);
        assert_eq!(config.ollama.model, "nomic-embed-text");
        assert_eq!(config.qdrant.vector_size, 2560);
    }

    #[test]
    fn test_service_creation() {
        let service = EmbeddingService::new(EmbeddingConfig::default());
        assert!(service.is_ok());
    }

    #[test]
    fn test_service_with_urls() {
        let service = EmbeddingService::with_urls(
            "http://localhost:11434".to_string(),
            "http://localhost:6333".to_string(),
        );
        assert!(service.is_ok());
    }
}
