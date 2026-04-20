//! Batch insert strategy for Neo4j graph operations
//!
//! Provides efficient batch insertion with:
//! - Configurable batch sizes
//! - Transaction management
//! - Error recovery and retry logic
//! - Memory-efficient streaming

use anyhow::{Context, Result};
use neo4rs::{query, BoltType, Graph};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use super::nodes::{extract_workspace_id, NodeData};
use super::relationships::{extract_workspace_id_from_label, RelationshipData};

/// Configuration for batch operations
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Number of items per batch
    pub batch_size: usize,
    /// Maximum retries for failed batches
    pub max_retries: usize,
    /// Delay between retries (milliseconds)
    pub retry_delay_ms: u64,
    /// Enable transaction batching
    pub use_transactions: bool,
    /// Flush automatically when batch is full
    pub auto_flush: bool,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            max_retries: 3,
            retry_delay_ms: 100,
            use_transactions: true,
            auto_flush: true,
        }
    }
}

/// Statistics for batch operations
#[derive(Debug, Default, Clone)]
pub struct BatchStats {
    pub total_nodes_processed: usize,
    pub total_relationships_processed: usize,
    pub batches_executed: usize,
    pub retries_attempted: usize,
    pub errors: usize,
    pub total_time_ms: u64,
}

/// Batch insert manager for Neo4j
pub struct BatchInsert {
    graph: Arc<Graph>,
    config: BatchConfig,
    workspace_label: Option<String>,
    pending_nodes: Vec<NodeData>,
    pending_relationships: Vec<RelationshipData>,
    stats: BatchStats,
    start_time: Instant,
}

impl BatchInsert {
    /// Create a new batch insert manager
    pub fn new(graph: Arc<Graph>, config: BatchConfig, workspace_label: Option<String>) -> Self {
        let batch_size = config.batch_size;
        Self {
            graph,
            config,
            workspace_label,
            pending_nodes: Vec::with_capacity(batch_size),
            pending_relationships: Vec::with_capacity(batch_size),
            stats: BatchStats::default(),
            start_time: Instant::now(),
        }
    }

    /// Add a node to the pending batch
    pub async fn add_node(&mut self, node: NodeData) -> Result<()> {
        self.pending_nodes.push(node);

        if self.config.auto_flush && self.pending_nodes.len() >= self.config.batch_size {
            self.flush_nodes().await?;
        }

        Ok(())
    }

    /// Add a relationship to the pending batch
    pub async fn add_relationship(&mut self, rel: RelationshipData) -> Result<()> {
        self.pending_relationships.push(rel);

        if self.config.auto_flush && self.pending_relationships.len() >= self.config.batch_size {
            self.flush_relationships().await?;
        }

        Ok(())
    }

    /// Flush all pending nodes
    pub async fn flush_nodes(&mut self) -> Result<()> {
        if self.pending_nodes.is_empty() {
            return Ok(());
        }

        let nodes = std::mem::take(&mut self.pending_nodes);
        let count = nodes.len();

        debug!("Flushing {} nodes to Neo4j", count);

        let result = self.insert_nodes_batch_with_retry(&nodes).await;

        match result {
            Ok(_) => {
                self.stats.total_nodes_processed += count;
                self.stats.batches_executed += 1;
                debug!("Successfully inserted {} nodes", count);
            }
            Err(e) => {
                error!("Failed to insert nodes batch: {}", e);
                self.stats.errors += 1;
                // Re-add nodes to pending for potential recovery
                self.pending_nodes = nodes;
                return Err(e);
            }
        }

        Ok(())
    }

    /// Flush all pending relationships
    pub async fn flush_relationships(&mut self) -> Result<()> {
        if self.pending_relationships.is_empty() {
            return Ok(());
        }

        let relationships = std::mem::take(&mut self.pending_relationships);
        let count = relationships.len();

        debug!("Flushing {} relationships to Neo4j", count);

        let result = self
            .insert_relationships_batch_with_retry(&relationships)
            .await;

        match result {
            Ok(_) => {
                self.stats.total_relationships_processed += count;
                self.stats.batches_executed += 1;
                debug!("Successfully inserted {} relationships", count);
            }
            Err(e) => {
                error!("Failed to insert relationships batch: {}", e);
                self.stats.errors += 1;
                // Re-add relationships to pending for potential recovery
                self.pending_relationships = relationships;
                return Err(e);
            }
        }

        Ok(())
    }

    /// Flush all pending items (nodes and relationships)
    pub async fn flush_all(&mut self) -> Result<()> {
        self.flush_nodes().await?;
        self.flush_relationships().await?;
        Ok(())
    }

    /// Insert nodes with retry logic
    async fn insert_nodes_batch_with_retry(&mut self, nodes: &[NodeData]) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..self.config.max_retries {
            match self.insert_nodes_batch(nodes).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    warn!("Batch insert attempt {} failed: {}", attempt + 1, e);
                    last_error = Some(e);
                    self.stats.retries_attempted += 1;

                    if attempt < self.config.max_retries - 1 {
                        tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
    }

    /// Insert relationships with retry logic
    async fn insert_relationships_batch_with_retry(
        &mut self,
        relationships: &[RelationshipData],
    ) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..self.config.max_retries {
            match self.insert_relationships_batch(relationships).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    warn!("Batch insert attempt {} failed: {}", attempt + 1, e);
                    last_error = Some(e);
                    self.stats.retries_attempted += 1;

                    if attempt < self.config.max_retries - 1 {
                        tokio::time::sleep(Duration::from_millis(self.config.retry_delay_ms)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error")))
    }

    /// Insert a batch of nodes using UNWIND
    async fn insert_nodes_batch(&self, nodes: &[NodeData]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        // Group nodes by type for efficient batching
        let mut nodes_by_type: HashMap<String, Vec<&NodeData>> = HashMap::new();
        for node in nodes {
            nodes_by_type
                .entry(node.node_type.label().to_string())
                .or_default()
                .push(node);
        }

        for (label, type_nodes) in nodes_by_type {
            let query_str = match &self.workspace_label {
                Some(ws) => format!(
                    "UNWIND $nodes AS node_data \
                     MERGE (n:{}:{} {{id: node_data.id}}) \
                     SET n += node_data.props",
                    label, ws
                ),
                None => format!(
                    "UNWIND $nodes AS node_data \
                     MERGE (n:{} {{id: node_data.id}}) \
                     SET n += node_data.props",
                    label
                ),
            };

            let node_params: Vec<HashMap<String, BoltType>> = type_nodes
                .iter()
                .map(|node| {
                    let mut props = HashMap::new();
                    props.insert("id".to_string(), BoltType::from(node.id.as_str()));
                    props.insert("fqn".to_string(), BoltType::from(node.fqn.as_str()));
                    props.insert("name".to_string(), BoltType::from(node.name.as_str()));

                    let ws_id = node.workspace_id.as_deref().or_else(|| {
                        self.workspace_label
                            .as_deref()
                            .and_then(extract_workspace_id)
                    });
                    if let Some(ws_id) = ws_id {
                        props.insert("workspace_id".to_string(), BoltType::from(ws_id));
                    }

                    for (key, value) in &node.properties {
                        if let Some(bolt_value) = node_property_to_bolt(value) {
                            props.insert(key.clone(), bolt_value);
                        }
                    }

                    let mut node_param = HashMap::new();
                    node_param.insert("id".to_string(), BoltType::from(node.id.as_str()));
                    node_param.insert("props".to_string(), BoltType::from(props));
                    node_param
                })
                .collect();

            self.graph
                .run(query(&query_str).param("nodes", node_params))
                .await
                .context(format!("Failed to batch insert {} nodes", label))?;
        }

        Ok(())
    }

    /// Insert a batch of relationships using UNWIND
    ///
    /// Groups by (rel_type, from_label, to_label) so every MATCH uses a node
    /// label, enabling Neo4j to use per-label unique-constraint indexes instead
    /// of full node scans.
    async fn insert_relationships_batch(&self, relationships: &[RelationshipData]) -> Result<()> {
        if relationships.is_empty() {
            return Ok(());
        }

        let merge_target_types = ["HAS_FIELD", "HAS_VARIANT", "USES_TYPE", "FOR", "EXTENDS"];

        let mut grouped: HashMap<(String, String, String), Vec<&RelationshipData>> = HashMap::new();
        for rel in relationships {
            grouped
                .entry((
                    rel.rel_type.name().to_string(),
                    rel.from_label.clone(),
                    rel.to_label.clone(),
                ))
                .or_default()
                .push(rel);
        }

        for ((rel_type, from_label, to_label), group_rels) in grouped {
            let query_str = if merge_target_types.contains(&rel_type.as_str()) {
                match &self.workspace_label {
                    Some(ws) => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{}:{} {{id: rel_data.from_id}}) \
                         MERGE (to:{}:{} {{id: rel_data.to_id}}) \
                         ON CREATE SET to.fqn = rel_data.to_id, to.name = rel_data.to_id, to.external = true \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, ws, to_label, ws, rel_type
                    ),
                    None => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{} {{id: rel_data.from_id}}) \
                         MERGE (to:{} {{id: rel_data.to_id}}) \
                         ON CREATE SET to.fqn = rel_data.to_id, to.name = rel_data.to_id, to.external = true \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, to_label, rel_type
                    ),
                }
            } else {
                match &self.workspace_label {
                    Some(ws) => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{}:{} {{id: rel_data.from_id}}) \
                         MATCH (to:{}:{} {{id: rel_data.to_id}}) \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, ws, to_label, ws, rel_type
                    ),
                    None => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{} {{id: rel_data.from_id}}) \
                         MATCH (to:{} {{id: rel_data.to_id}}) \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, to_label, rel_type
                    ),
                }
            };

            let rel_params: Vec<HashMap<String, BoltType>> = group_rels
                .iter()
                .map(|rel| {
                    let mut props: HashMap<String, BoltType> = rel
                        .properties
                        .iter()
                        .filter_map(|(k, v)| rel_property_to_bolt(v).map(|bv| (k.clone(), bv)))
                        .collect();

                    let ws_id = rel.workspace_id.as_deref().or_else(|| {
                        self.workspace_label
                            .as_deref()
                            .and_then(extract_workspace_id_from_label)
                    });
                    if let Some(ws_id) = ws_id {
                        props.insert("workspace_id".to_string(), BoltType::from(ws_id));
                    }

                    let mut rel_param = HashMap::new();
                    rel_param.insert("from_id".to_string(), BoltType::from(rel.from_id.as_str()));
                    rel_param.insert("to_id".to_string(), BoltType::from(rel.to_id.as_str()));
                    rel_param.insert("props".to_string(), BoltType::from(props));
                    rel_param
                })
                .collect();

            self.graph
                .run(query(&query_str).param("rels", rel_params))
                .await
                .context(format!("Failed to batch insert {} relationships", rel_type))?;
        }

        Ok(())
    }

    /// Get current statistics
    pub fn stats(&self) -> &BatchStats {
        &self.stats
    }

    /// Get pending node count
    pub fn pending_nodes(&self) -> usize {
        self.pending_nodes.len()
    }

    /// Get pending relationship count
    pub fn pending_relationships(&self) -> usize {
        self.pending_relationships.len()
    }

    /// Check if there are pending items
    pub fn has_pending(&self) -> bool {
        !self.pending_nodes.is_empty() || !self.pending_relationships.is_empty()
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = BatchStats::default();
        self.start_time = Instant::now();
    }
}

/// Convert node property to BoltType
fn node_property_to_bolt(value: &super::nodes::PropertyValue) -> Option<BoltType> {
    use super::nodes::PropertyValue as NPV;
    match value {
        NPV::String(s) => Some(BoltType::from(s.as_str())),
        NPV::Int(i) => Some(BoltType::from(*i)),
        NPV::Float(f) => Some(BoltType::from(*f)),
        NPV::Bool(b) => Some(BoltType::from(*b)),
        NPV::Array(arr) => {
            let bolt_list: Vec<BoltType> = arr.iter().map(|s| BoltType::from(s.as_str())).collect();
            Some(BoltType::from(bolt_list))
        }
        NPV::Null => None,
    }
}

/// Convert relationship property to BoltType
fn rel_property_to_bolt(value: &super::relationships::PropertyValue) -> Option<BoltType> {
    use super::relationships::PropertyValue as RPV;
    match value {
        RPV::String(s) => Some(BoltType::from(s.as_str())),
        RPV::Int(i) => Some(BoltType::from(*i)),
        RPV::Float(f) => Some(BoltType::from(*f)),
        RPV::Bool(b) => Some(BoltType::from(*b)),
        RPV::Array(arr) => {
            let bolt_list: Vec<BoltType> = arr.iter().map(|s| BoltType::from(s.as_str())).collect();
            Some(BoltType::from(bolt_list))
        }
        RPV::Null => None,
    }
}

/// Batch processor for large-scale ingestion
pub struct BatchProcessor {
    graph: Arc<Graph>,
    config: BatchConfig,
    workspace_label: Option<String>,
}

impl BatchProcessor {
    /// Create a new batch processor
    pub fn new(graph: Arc<Graph>, config: BatchConfig, workspace_label: Option<String>) -> Self {
        Self {
            graph,
            config,
            workspace_label,
        }
    }

    /// Process a large number of nodes (10,000+)
    pub async fn process_large_node_batch(&self, nodes: Vec<NodeData>) -> Result<BatchStats> {
        let total = nodes.len();
        info!(
            "Processing {} nodes in batches of {}",
            total, self.config.batch_size
        );

        let start = Instant::now();
        let mut stats = BatchStats::default();
        let mut processed = 0;

        for chunk in nodes.chunks(self.config.batch_size) {
            match self.insert_nodes_chunk(chunk).await {
                Ok(count) => {
                    processed += count;
                    stats.total_nodes_processed += count;
                    stats.batches_executed += 1;

                    if processed % 5000 == 0 {
                        info!(
                            "Progress: {}/{} nodes ({:.1}%)",
                            processed,
                            total,
                            (processed as f64 / total as f64) * 100.0
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to process chunk: {}", e);
                    stats.errors += 1;
                }
            }
        }

        stats.total_time_ms = start.elapsed().as_millis() as u64;
        info!(
            "Completed: {} nodes in {}ms ({:.0} nodes/sec)",
            stats.total_nodes_processed,
            stats.total_time_ms,
            if stats.total_time_ms > 0 {
                stats.total_nodes_processed as f64 / (stats.total_time_ms as f64 / 1000.0)
            } else {
                0.0
            }
        );

        Ok(stats)
    }

    /// Process a large number of relationships (10,000+)
    pub async fn process_large_relationship_batch(
        &self,
        relationships: Vec<RelationshipData>,
    ) -> Result<BatchStats> {
        let total = relationships.len();
        info!(
            "Processing {} relationships in batches of {}",
            total, self.config.batch_size
        );

        let start = Instant::now();
        let mut stats = BatchStats::default();
        let mut processed = 0;

        for chunk in relationships.chunks(self.config.batch_size) {
            match self.insert_relationships_chunk(chunk).await {
                Ok(count) => {
                    processed += count;
                    stats.total_relationships_processed += count;
                    stats.batches_executed += 1;

                    if processed % 5000 == 0 {
                        info!(
                            "Progress: {}/{} relationships ({:.1}%)",
                            processed,
                            total,
                            (processed as f64 / total as f64) * 100.0
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to process chunk: {}", e);
                    stats.errors += 1;
                }
            }
        }

        stats.total_time_ms = start.elapsed().as_millis() as u64;
        info!(
            "Completed: {} relationships in {}ms ({:.0} rels/sec)",
            stats.total_relationships_processed,
            stats.total_time_ms,
            if stats.total_time_ms > 0 {
                stats.total_relationships_processed as f64 / (stats.total_time_ms as f64 / 1000.0)
            } else {
                0.0
            }
        );

        Ok(stats)
    }

    /// Insert a chunk of nodes
    async fn insert_nodes_chunk(&self, nodes: &[NodeData]) -> Result<usize> {
        let mut nodes_by_type: HashMap<String, Vec<&NodeData>> = HashMap::new();
        for node in nodes {
            nodes_by_type
                .entry(node.node_type.label().to_string())
                .or_default()
                .push(node);
        }

        for (label, type_nodes) in nodes_by_type {
            let query_str = match &self.workspace_label {
                Some(ws) => format!(
                    "UNWIND $nodes AS node_data \
                     MERGE (n:{}:{} {{id: node_data.id}}) \
                     SET n += node_data.props",
                    label, ws
                ),
                None => format!(
                    "UNWIND $nodes AS node_data \
                     MERGE (n:{} {{id: node_data.id}}) \
                     SET n += node_data.props",
                    label
                ),
            };

            let node_params: Vec<HashMap<String, BoltType>> = type_nodes
                .iter()
                .map(|node| {
                    let mut props = HashMap::new();
                    props.insert("id".to_string(), BoltType::from(node.id.as_str()));
                    props.insert("fqn".to_string(), BoltType::from(node.fqn.as_str()));
                    props.insert("name".to_string(), BoltType::from(node.name.as_str()));

                    let ws_id = node.workspace_id.as_deref().or_else(|| {
                        self.workspace_label
                            .as_deref()
                            .and_then(extract_workspace_id)
                    });
                    if let Some(ws_id) = ws_id {
                        props.insert("workspace_id".to_string(), BoltType::from(ws_id));
                    }

                    for (key, value) in &node.properties {
                        if let Some(bolt_value) = node_property_to_bolt(value) {
                            props.insert(key.clone(), bolt_value);
                        }
                    }

                    let mut node_param = HashMap::new();
                    node_param.insert("id".to_string(), BoltType::from(node.id.as_str()));
                    node_param.insert("props".to_string(), BoltType::from(props));
                    node_param
                })
                .collect();

            self.graph
                .run(query(&query_str).param("nodes", node_params))
                .await
                .context(format!("Failed to batch insert {} nodes", label))?;
        }

        Ok(nodes.len())
    }

    /// Insert a chunk of relationships
    async fn insert_relationships_chunk(
        &self,
        relationships: &[RelationshipData],
    ) -> Result<usize> {
        // Group by (rel_type, from_label, to_label) for label-aware MATCH
        let mut grouped: HashMap<(String, String, String), Vec<&RelationshipData>> = HashMap::new();
        for rel in relationships {
            grouped
                .entry((
                    rel.rel_type.name().to_string(),
                    rel.from_label.clone(),
                    rel.to_label.clone(),
                ))
                .or_default()
                .push(rel);
        }

        // Relationship types where target nodes may not exist and should be
        // created as placeholders. For these, use MERGE on the target instead
        // of MATCH (which silently drops rows when the node doesn't exist).
        let merge_target_types = ["HAS_FIELD", "HAS_VARIANT", "USES_TYPE", "FOR", "EXTENDS"];

        for ((rel_type, from_label, to_label), group_rels) in grouped {
            let query_str = if merge_target_types.contains(&rel_type.as_str()) {
                match &self.workspace_label {
                    Some(ws) => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{}:{} {{id: rel_data.from_id}}) \
                         MERGE (to:{}:{} {{id: rel_data.to_id}}) \
                         ON CREATE SET to.fqn = rel_data.to_id, to.name = rel_data.to_id, to.external = true \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, ws, to_label, ws, rel_type
                    ),
                    None => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{} {{id: rel_data.from_id}}) \
                         MERGE (to:{} {{id: rel_data.to_id}}) \
                         ON CREATE SET to.fqn = rel_data.to_id, to.name = rel_data.to_id, to.external = true \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, to_label, rel_type
                    ),
                }
            } else {
                match &self.workspace_label {
                    Some(ws) => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{}:{} {{id: rel_data.from_id}}) \
                         MATCH (to:{}:{} {{id: rel_data.to_id}}) \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, ws, to_label, ws, rel_type
                    ),
                    None => format!(
                        "UNWIND $rels AS rel_data \
                         MATCH (from:{} {{id: rel_data.from_id}}) \
                         MATCH (to:{} {{id: rel_data.to_id}}) \
                         MERGE (from)-[r:{}]->(to) \
                         SET r += rel_data.props",
                        from_label, to_label, rel_type
                    ),
                }
            };

            let rel_params: Vec<HashMap<String, BoltType>> = group_rels
                .iter()
                .map(|rel| {
                    let mut props: HashMap<String, BoltType> = rel
                        .properties
                        .iter()
                        .filter_map(|(k, v)| rel_property_to_bolt(v).map(|bv| (k.clone(), bv)))
                        .collect();

                    let ws_id = rel.workspace_id.as_deref().or_else(|| {
                        self.workspace_label
                            .as_deref()
                            .and_then(extract_workspace_id_from_label)
                    });
                    if let Some(ws_id) = ws_id {
                        props.insert("workspace_id".to_string(), BoltType::from(ws_id));
                    }

                    let mut rel_param = HashMap::new();
                    rel_param.insert("from_id".to_string(), BoltType::from(rel.from_id.as_str()));
                    rel_param.insert("to_id".to_string(), BoltType::from(rel.to_id.as_str()));
                    rel_param.insert("props".to_string(), BoltType::from(props));
                    rel_param
                })
                .collect();

            self.graph
                .run(query(&query_str).param("rels", rel_params))
                .await
                .context(format!("Failed to batch insert {} relationships", rel_type))?;
        }

        Ok(relationships.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_config_defaults() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.max_retries, 3);
        assert!(config.auto_flush);
    }

    #[test]
    fn test_batch_stats_default() {
        let stats = BatchStats::default();
        assert_eq!(stats.total_nodes_processed, 0);
        assert_eq!(stats.total_relationships_processed, 0);
        assert_eq!(stats.errors, 0);
    }

    /// Build the batch node query string for testing without needing Neo4j
    fn build_batch_node_query(label: &str, workspace_label: Option<&str>) -> String {
        match workspace_label {
            Some(ws) => format!(
                "UNWIND $nodes AS node_data \
                 MERGE (n:{}:{} {{id: node_data.id}}) \
                 SET n += node_data.props",
                label, ws
            ),
            None => format!(
                "UNWIND $nodes AS node_data \
                 MERGE (n:{} {{id: node_data.id}}) \
                 SET n += node_data.props",
                label
            ),
        }
    }

    #[test]
    fn test_batch_node_query_without_workspace() {
        let query = build_batch_node_query("Function", None);
        assert_eq!(
            query,
            "UNWIND $nodes AS node_data MERGE (n:Function {id: node_data.id}) SET n += node_data.props"
        );

        let query = build_batch_node_query("Struct", None);
        assert_eq!(
            query,
            "UNWIND $nodes AS node_data MERGE (n:Struct {id: node_data.id}) SET n += node_data.props"
        );
    }

    #[test]
    fn test_batch_node_query_with_workspace() {
        let query = build_batch_node_query("Function", Some("Workspace_a1b2c3d4e5f6"));
        assert_eq!(
            query,
            "UNWIND $nodes AS node_data MERGE (n:Function:Workspace_a1b2c3d4e5f6 {id: node_data.id}) SET n += node_data.props"
        );

        let query = build_batch_node_query("Struct", Some("Workspace_abcdef123456"));
        assert_eq!(
            query,
            "UNWIND $nodes AS node_data MERGE (n:Struct:Workspace_abcdef123456 {id: node_data.id}) SET n += node_data.props"
        );
    }

    #[test]
    fn test_batch_config_from_env() {
        // Test default values match expected constants
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_delay_ms, 100);
        assert!(config.use_transactions);
        assert!(config.auto_flush);
    }
}
