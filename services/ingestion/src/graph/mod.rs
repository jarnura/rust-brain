//! Neo4j Graph Construction Service
//!
//! This module provides graph database integration for rust-brain, enabling
//! construction and querying of code relationships using Neo4j.

mod batch;
mod nodes;
mod relationships;

use anyhow::{Context, Result};
use neo4rs::{query, Graph};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

pub use batch::{BatchConfig, BatchInsert, BatchProcessor, BatchStats};
pub use nodes::{NodeBuilder, NodeData, PropertyValue};
pub use relationships::{RelationshipBuilder, RelationshipData, RelationshipType};

/// Default Neo4j connection URI
pub const DEFAULT_NEO4J_URI: &str = "bolt://neo4j:7687";

/// Default batch size for bulk operations
pub const DEFAULT_BATCH_SIZE: usize = 1000;

/// Redact password from database/connection URLs for safe logging
///
/// Examples:
/// - postgres://user:password@host/db → postgres://user:***@host/db
/// - bolt://user:password@host → bolt://user:***@host
fn redact_url(url: &str) -> String {
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

/// Graph builder configuration
#[derive(Debug, Clone)]
pub struct GraphConfig {
    /// Neo4j connection URI
    pub uri: String,
    /// Username for authentication
    pub username: String,
    /// Password for authentication
    pub password: String,
    /// Database name (Neo4j 4.x+ supports multiple databases)
    pub database: String,
    /// Maximum connections in the pool
    pub max_connections: usize,
    /// Batch size for bulk operations
    pub batch_size: usize,
    /// Neo4j label in format `Workspace_<12hex>` for multi-tenant isolation
    pub workspace_label: Option<String>,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            uri: std::env::var("NEO4J_URI").unwrap_or_else(|_| DEFAULT_NEO4J_URI.to_string()),
            username: std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
            password: std::env::var("NEO4J_PASSWORD").unwrap_or_else(|_| "password".to_string()),
            database: std::env::var("NEO4J_DATABASE").unwrap_or_else(|_| "neo4j".to_string()),
            max_connections: 10,
            batch_size: DEFAULT_BATCH_SIZE,
            workspace_label: None,
        }
    }
}

/// Main graph builder for Neo4j operations
pub struct GraphBuilder {
    /// Neo4j graph connection
    graph: Arc<Graph>,
    /// Configuration
    config: GraphConfig,
    /// Node builder for creating nodes
    node_builder: NodeBuilder,
    /// Relationship builder for creating relationships
    relationship_builder: RelationshipBuilder,
    /// Batch insert manager
    batch_insert: Arc<RwLock<BatchInsert>>,
    /// Statistics
    stats: Arc<RwLock<GraphStats>>,
}

/// Statistics for graph operations
#[derive(Debug, Default, Clone)]
pub struct GraphStats {
    pub nodes_created: usize,
    pub nodes_merged: usize,
    pub relationships_created: usize,
    pub batches_processed: usize,
    pub errors: usize,
}

impl GraphBuilder {
    /// Create a new graph builder with default configuration
    pub async fn new() -> Result<Self> {
        Self::with_config(GraphConfig::default()).await
    }

    /// Create a new graph builder with custom configuration
    pub async fn with_config(config: GraphConfig) -> Result<Self> {
        info!("Connecting to Neo4j at {}", redact_url(&config.uri));

        let graph = Arc::new(
            Graph::new(&config.uri, &config.username, &config.password)
                .await
                .context("Failed to connect to Neo4j")?,
        );

        info!("Successfully connected to Neo4j");

        let node_builder = NodeBuilder::new(Arc::clone(&graph), config.workspace_label.clone());
        let relationship_builder =
            RelationshipBuilder::new(Arc::clone(&graph), config.workspace_label.clone());
        let batch_insert = Arc::new(RwLock::new(BatchInsert::new(
            Arc::clone(&graph),
            BatchConfig {
                batch_size: config.batch_size,
                ..Default::default()
            },
            config.workspace_label.clone(),
        )));

        Ok(Self {
            graph,
            config,
            node_builder,
            relationship_builder,
            batch_insert,
            stats: Arc::new(RwLock::new(GraphStats::default())),
        })
    }

    /// Test the connection to Neo4j
    pub async fn test_connection(&self) -> Result<bool> {
        let mut result = self
            .graph
            .execute(query("RETURN 1 as test"))
            .await
            .context("Failed to execute test query")?;

        match result.next().await {
            Ok(Some(row)) => {
                let test: i64 = row.get("test").unwrap_or(0);
                Ok(test == 1)
            }
            Ok(None) => Ok(false),
            Err(e) => {
                error!("Connection test failed: {}", e);
                Ok(false)
            }
        }
    }

    /// Create indexes for better query performance
    pub async fn create_indexes(&self) -> Result<()> {
        info!("Creating indexes for graph nodes...");

        let index_queries = [
            // Unique constraints on id for each node type
            "CREATE CONSTRAINT crate_id_unique IF NOT EXISTS FOR (c:Crate) REQUIRE c.id IS UNIQUE",
            "CREATE CONSTRAINT module_id_unique IF NOT EXISTS FOR (m:Module) REQUIRE m.id IS UNIQUE",
            "CREATE CONSTRAINT function_id_unique IF NOT EXISTS FOR (f:Function) REQUIRE f.id IS UNIQUE",
            "CREATE CONSTRAINT struct_id_unique IF NOT EXISTS FOR (s:Struct) REQUIRE s.id IS UNIQUE",
            "CREATE CONSTRAINT enum_id_unique IF NOT EXISTS FOR (e:Enum) REQUIRE e.id IS UNIQUE",
            "CREATE CONSTRAINT trait_id_unique IF NOT EXISTS FOR (t:Trait) REQUIRE t.id IS UNIQUE",
            "CREATE CONSTRAINT impl_id_unique IF NOT EXISTS FOR (i:Impl) REQUIRE i.id IS UNIQUE",
            "CREATE CONSTRAINT type_id_unique IF NOT EXISTS FOR (t:Type) REQUIRE t.id IS UNIQUE",
            "CREATE CONSTRAINT type_alias_id_unique IF NOT EXISTS FOR (t:TypeAlias) REQUIRE t.id IS UNIQUE",
            "CREATE CONSTRAINT const_id_unique IF NOT EXISTS FOR (c:Const) REQUIRE c.id IS UNIQUE",
            "CREATE CONSTRAINT static_id_unique IF NOT EXISTS FOR (s:Static) REQUIRE s.id IS UNIQUE",
            "CREATE CONSTRAINT macro_id_unique IF NOT EXISTS FOR (m:Macro) REQUIRE m.id IS UNIQUE",
            // Indexes on name for faster lookups (one per label — Neo4j 5.x doesn't support multi-label indexes)
            "CREATE INDEX crate_name_index IF NOT EXISTS FOR (n:Crate) ON (n.name)",
            "CREATE INDEX module_name_index IF NOT EXISTS FOR (n:Module) ON (n.name)",
            "CREATE INDEX function_name_index IF NOT EXISTS FOR (n:Function) ON (n.name)",
            "CREATE INDEX struct_name_index IF NOT EXISTS FOR (n:Struct) ON (n.name)",
            "CREATE INDEX enum_name_index IF NOT EXISTS FOR (n:Enum) ON (n.name)",
            "CREATE INDEX trait_name_index IF NOT EXISTS FOR (n:Trait) ON (n.name)",
            "CREATE INDEX impl_name_index IF NOT EXISTS FOR (n:Impl) ON (n.name)",
            "CREATE INDEX type_name_index IF NOT EXISTS FOR (n:Type) ON (n.name)",
            "CREATE INDEX type_alias_name_index IF NOT EXISTS FOR (n:TypeAlias) ON (n.name)",
            "CREATE INDEX const_name_index IF NOT EXISTS FOR (n:Const) ON (n.name)",
            "CREATE INDEX static_name_index IF NOT EXISTS FOR (n:Static) ON (n.name)",
            "CREATE INDEX macro_name_index IF NOT EXISTS FOR (n:Macro) ON (n.name)",
            // Indexes on fqn for faster lookups
            "CREATE INDEX crate_fqn_index IF NOT EXISTS FOR (n:Crate) ON (n.fqn)",
            "CREATE INDEX module_fqn_index IF NOT EXISTS FOR (n:Module) ON (n.fqn)",
            "CREATE INDEX function_fqn_index IF NOT EXISTS FOR (n:Function) ON (n.fqn)",
            "CREATE INDEX struct_fqn_index IF NOT EXISTS FOR (n:Struct) ON (n.fqn)",
            "CREATE INDEX enum_fqn_index IF NOT EXISTS FOR (n:Enum) ON (n.fqn)",
            "CREATE INDEX trait_fqn_index IF NOT EXISTS FOR (n:Trait) ON (n.fqn)",
            "CREATE INDEX impl_fqn_index IF NOT EXISTS FOR (n:Impl) ON (n.fqn)",
            "CREATE INDEX type_fqn_index IF NOT EXISTS FOR (n:Type) ON (n.fqn)",
            "CREATE INDEX type_alias_fqn_index IF NOT EXISTS FOR (n:TypeAlias) ON (n.fqn)",
            "CREATE INDEX const_fqn_index IF NOT EXISTS FOR (n:Const) ON (n.fqn)",
            "CREATE INDEX static_fqn_index IF NOT EXISTS FOR (n:Static) ON (n.fqn)",
            "CREATE INDEX macro_fqn_index IF NOT EXISTS FOR (n:Macro) ON (n.fqn)",
        ];

        for query_str in &index_queries {
            match self.graph.run(query(query_str)).await {
                Ok(_) => debug!(
                    "Created index: {}",
                    query_str
                        .split_whitespace()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                Err(e) => warn!("Failed to create index (may already exist): {}", e),
            }
        }

        info!("Index creation complete");
        Ok(())
    }

    /// Create workspace-scoped unique constraints for a specific workspace label
    pub async fn create_workspace_constraints(&self, workspace_label: &str) -> Result<()> {
        info!(
            "Creating workspace-scoped constraints for {}",
            workspace_label
        );

        let node_labels = [
            "Crate",
            "Module",
            "Function",
            "Struct",
            "Enum",
            "Trait",
            "Impl",
            "Type",
            "TypeAlias",
            "Const",
            "Static",
            "Macro",
        ];

        // Derive a safe constraint name suffix from workspace label
        // e.g. "Workspace_a1b2c3d4e5f6" -> "ws_a1b2c3d4e5f6"
        let ws_suffix = workspace_label
            .strip_prefix("Workspace_")
            .unwrap_or(workspace_label);

        for label in &node_labels {
            let constraint_name = format!("ws_{}_{}_fqn_unique", ws_suffix, label.to_lowercase());
            let constraint_query = format!(
                "CREATE CONSTRAINT {constraint_name} IF NOT EXISTS \
                 FOR (n:{label}:{workspace_label}) REQUIRE n.fqn IS UNIQUE"
            );
            match self.graph.run(query(&constraint_query)).await {
                Ok(_) => debug!("Created workspace constraint: {}", constraint_name),
                Err(e) => warn!(
                    "Failed to create workspace constraint (may already exist): {}",
                    e
                ),
            }
        }

        info!(
            "Workspace constraint creation complete for {}",
            workspace_label
        );
        Ok(())
    }

    /// Clear all graph data (use with caution!)
    pub async fn clear_all(&self) -> Result<()> {
        warn!("Clearing all graph data!");

        self.graph
            .run(query("MATCH (n) DETACH DELETE n"))
            .await
            .context("Failed to clear graph data")?;

        info!("All graph data cleared");
        Ok(())
    }

    /// Get node builder
    pub fn nodes(&self) -> &NodeBuilder {
        &self.node_builder
    }

    /// Get relationship builder
    pub fn relationships(&self) -> &RelationshipBuilder {
        &self.relationship_builder
    }

    /// Get batch insert manager
    pub fn batch(&self) -> Arc<RwLock<BatchInsert>> {
        Arc::clone(&self.batch_insert)
    }

    /// Get current statistics
    pub async fn stats(&self) -> GraphStats {
        self.stats.read().await.clone()
    }

    /// Create a single node (MERGE for idempotency)
    pub async fn create_node(&self, node: &NodeData) -> Result<()> {
        self.node_builder.merge_node(node).await?;

        let mut stats = self.stats.write().await;
        stats.nodes_merged += 1;

        Ok(())
    }

    /// Create multiple nodes in batch
    pub async fn create_nodes_batch(&self, nodes: Vec<NodeData>) -> Result<()> {
        let count = nodes.len();

        let mut batch = self.batch_insert.write().await;
        for node in nodes {
            batch.add_node(node).await?;
        }
        batch.flush_nodes().await?;

        let mut stats = self.stats.write().await;
        stats.nodes_merged += count;

        Ok(())
    }

    /// Create a single relationship
    pub async fn create_relationship(&self, rel: &RelationshipData) -> Result<()> {
        self.relationship_builder.merge_relationship(rel).await?;

        let mut stats = self.stats.write().await;
        stats.relationships_created += 1;

        Ok(())
    }

    /// Create multiple relationships in batch
    pub async fn create_relationships_batch(
        &self,
        relationships: Vec<RelationshipData>,
    ) -> Result<()> {
        let count = relationships.len();

        let mut batch = self.batch_insert.write().await;
        for rel in relationships {
            batch.add_relationship(rel).await?;
        }
        batch.flush_relationships().await?;

        let mut stats = self.stats.write().await;
        stats.relationships_created += count;

        Ok(())
    }

    /// Flush all pending batch operations
    pub async fn flush(&self) -> Result<()> {
        let mut batch = self.batch_insert.write().await;
        batch.flush_all().await?;

        let mut stats = self.stats.write().await;
        stats.batches_processed += 1;

        Ok(())
    }

    /// Find a node by its FQN
    pub async fn find_node_by_fqn(&self, fqn: &str) -> Result<Option<HashMap<String, String>>> {
        let mut result = self
            .graph
            .execute(query("MATCH (n {fqn: $fqn}) RETURN n").param("fqn", fqn))
            .await
            .context("Failed to find node by FQN")?;

        if let Ok(Some(row)) = result.next().await {
            let node: neo4rs::Node = row.get("n")?;
            let mut map = HashMap::new();
            for key in node.keys() {
                if let Ok(value) = node.get::<String>(key) {
                    map.insert(key.to_string(), value);
                }
            }
            Ok(Some(map))
        } else {
            Ok(None)
        }
    }

    /// Find all nodes of a specific type
    pub async fn find_nodes_by_type(&self, label: &str) -> Result<Vec<HashMap<String, String>>> {
        let query_str = format!("MATCH (n:{}) RETURN n", label);
        let mut result = self
            .graph
            .execute(query(&query_str))
            .await
            .context("Failed to find nodes by type")?;

        let mut nodes = Vec::new();
        while let Ok(Some(row)) = result.next().await {
            let node: neo4rs::Node = row.get("n")?;
            let mut map = HashMap::new();
            for key in node.keys() {
                if let Ok(value) = node.get::<String>(key) {
                    map.insert(key.to_string(), value);
                }
            }
            nodes.push(map);
        }

        Ok(nodes)
    }

    /// Get the underlying graph connection
    pub fn graph(&self) -> Arc<Graph> {
        Arc::clone(&self.graph)
    }

    /// Get the configuration
    pub fn config(&self) -> &GraphConfig {
        &self.config
    }
}

/// Node types supported by the graph
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    Crate,
    Module,
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Type,
    TypeAlias,
    Const,
    Static,
    Macro,
}

impl NodeType {
    /// Get the label string for Neo4j
    pub fn label(&self) -> &'static str {
        match self {
            NodeType::Crate => "Crate",
            NodeType::Module => "Module",
            NodeType::Function => "Function",
            NodeType::Struct => "Struct",
            NodeType::Enum => "Enum",
            NodeType::Trait => "Trait",
            NodeType::Impl => "Impl",
            NodeType::Type => "Type",
            NodeType::TypeAlias => "TypeAlias",
            NodeType::Const => "Const",
            NodeType::Static => "Static",
            NodeType::Macro => "Macro",
        }
    }
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires Neo4j running
    async fn test_connection() {
        let builder = GraphBuilder::new().await.unwrap();
        let connected = builder.test_connection().await.unwrap();
        assert!(connected);
    }

    #[tokio::test]
    #[ignore] // Requires Neo4j running
    async fn test_create_indexes() {
        let builder = GraphBuilder::new().await.unwrap();
        builder.create_indexes().await.unwrap();
    }
}
