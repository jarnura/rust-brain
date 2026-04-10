//! Node creation functions for Neo4j graph
//!
//! Provides functions to create and manage nodes for all Rust code elements:
//! Crate, Module, Function, Struct, Enum, Trait, Impl, Type, TypeAlias, Const, Static, Macro

use anyhow::{Context, Result};
use neo4rs::{query, BoltType, Graph};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

use super::NodeType;

/// Node data for creating Neo4j nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeData {
    /// Unique identifier (typically the FQN)
    pub id: String,
    /// Fully qualified name (e.g., "crate::module::function")
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Node type
    #[serde(rename = "type")]
    pub node_type: NodeType,
    /// Additional properties
    #[serde(flatten)]
    pub properties: HashMap<String, PropertyValue>,
}

/// Property value that can be stored in Neo4j
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<String>),
    Null,
}

impl PropertyValue {
    pub fn to_bolt_type(&self) -> Option<BoltType> {
        match self {
            PropertyValue::String(s) => Some(BoltType::from(s.as_str())),
            PropertyValue::Int(i) => Some(BoltType::from(*i)),
            PropertyValue::Float(f) => Some(BoltType::from(*f)),
            PropertyValue::Bool(b) => Some(BoltType::from(*b)),
            PropertyValue::Array(arr) => {
                let bolt_list: Vec<BoltType> =
                    arr.iter().map(|s| BoltType::from(s.as_str())).collect();
                Some(BoltType::from(bolt_list))
            }
            PropertyValue::Null => None,
        }
    }
}

impl From<String> for PropertyValue {
    fn from(s: String) -> Self {
        PropertyValue::String(s)
    }
}

impl From<&str> for PropertyValue {
    fn from(s: &str) -> Self {
        PropertyValue::String(s.to_string())
    }
}

impl From<i64> for PropertyValue {
    fn from(i: i64) -> Self {
        PropertyValue::Int(i)
    }
}

impl From<usize> for PropertyValue {
    fn from(i: usize) -> Self {
        PropertyValue::Int(i as i64)
    }
}

impl From<bool> for PropertyValue {
    fn from(b: bool) -> Self {
        PropertyValue::Bool(b)
    }
}

impl From<Vec<String>> for PropertyValue {
    fn from(arr: Vec<String>) -> Self {
        PropertyValue::Array(arr)
    }
}

/// Builder for creating nodes in Neo4j
pub struct NodeBuilder {
    graph: Arc<Graph>,
}

impl NodeBuilder {
    /// Create a new node builder
    pub fn new(graph: Arc<Graph>) -> Self {
        Self { graph }
    }

    /// Build a MERGE query for creating a node (idempotent)
    fn build_merge_query(&self, node: &NodeData) -> String {
        let label = node.node_type.label();
        format!("MERGE (n:{} {{id: $id}}) SET n += $props", label)
    }

    /// Create or update a node using MERGE (idempotent)
    pub async fn merge_node(&self, node: &NodeData) -> Result<()> {
        let query_str = self.build_merge_query(node);

        let mut props = HashMap::new();
        props.insert("id".to_string(), BoltType::from(node.id.as_str()));
        props.insert("fqn".to_string(), BoltType::from(node.fqn.as_str()));
        props.insert("name".to_string(), BoltType::from(node.name.as_str()));

        for (key, value) in &node.properties {
            if let Some(bolt_value) = value.to_bolt_type() {
                props.insert(key.clone(), bolt_value);
            }
        }

        self.graph
            .run(
                query(&query_str)
                    .param("id", node.id.as_str())
                    .param("props", props),
            )
            .await
            .context("Failed to merge node")?;

        debug!("Merged node: {} ({})", node.fqn, node.node_type);
        Ok(())
    }

    /// Create a Crate node
    pub fn create_crate(
        id: impl Into<String>,
        name: impl Into<String>,
        version: Option<&str>,
        description: Option<&str>,
    ) -> NodeData {
        let name = name.into();
        let id = id.into();
        let fqn = name.clone();

        let mut properties = HashMap::new();
        if let Some(v) = version {
            properties.insert("version".to_string(), PropertyValue::from(v));
        }
        if let Some(d) = description {
            properties.insert("description".to_string(), PropertyValue::from(d));
        }

        NodeData {
            id,
            fqn,
            name,
            node_type: NodeType::Crate,
            properties,
        }
    }

    /// Create a Module node
    pub fn create_module(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        file_path: Option<&str>,
        is_public: bool,
    ) -> NodeData {
        let mut properties = HashMap::new();
        if let Some(fp) = file_path {
            properties.insert("file_path".to_string(), PropertyValue::from(fp));
        }
        properties.insert("is_public".to_string(), PropertyValue::from(is_public));

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Module,
            properties,
        }
    }

    /// Create a Function node
    #[allow(clippy::too_many_arguments)]
    pub fn create_function(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        signature: Option<&str>,
        visibility: &str,
        is_async: bool,
        is_unsafe: bool,
        is_const: bool,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        generic_params: Vec<String>,
        where_clauses: Vec<String>,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        if let Some(sig) = signature {
            properties.insert("signature".to_string(), PropertyValue::from(sig));
        }
        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert("is_async".to_string(), PropertyValue::from(is_async));
        properties.insert("is_unsafe".to_string(), PropertyValue::from(is_unsafe));
        properties.insert("is_const".to_string(), PropertyValue::from(is_const));
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        if !where_clauses.is_empty() {
            properties.insert(
                "where_clauses".to_string(),
                PropertyValue::from(where_clauses),
            );
        }
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Function,
            properties,
        }
    }

    /// Create a Struct node
    #[allow(clippy::too_many_arguments)]
    pub fn create_struct(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        is_pub_crate: bool,
        has_generics: bool,
        generic_params: Vec<String>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        attributes: Vec<String>,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert(
            "is_pub_crate".to_string(),
            PropertyValue::from(is_pub_crate),
        );
        properties.insert(
            "has_generics".to_string(),
            PropertyValue::from(has_generics),
        );
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if !attributes.is_empty() {
            properties.insert("attributes".to_string(), PropertyValue::from(attributes));
        }
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Struct,
            properties,
        }
    }

    /// Create an Enum node
    #[allow(clippy::too_many_arguments)]
    pub fn create_enum(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        has_generics: bool,
        generic_params: Vec<String>,
        variants: Vec<String>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        attributes: Vec<String>,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert(
            "has_generics".to_string(),
            PropertyValue::from(has_generics),
        );
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        if !variants.is_empty() {
            properties.insert("variants".to_string(), PropertyValue::from(variants));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if !attributes.is_empty() {
            properties.insert("attributes".to_string(), PropertyValue::from(attributes));
        }
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Enum,
            properties,
        }
    }

    /// Create a Trait node
    #[allow(clippy::too_many_arguments)]
    pub fn create_trait(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        is_unsafe: bool,
        has_generics: bool,
        generic_params: Vec<String>,
        required_methods: Vec<String>,
        provided_methods: Vec<String>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        attributes: Vec<String>,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert("is_unsafe".to_string(), PropertyValue::from(is_unsafe));
        properties.insert(
            "has_generics".to_string(),
            PropertyValue::from(has_generics),
        );
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        if !required_methods.is_empty() {
            properties.insert(
                "required_methods".to_string(),
                PropertyValue::from(required_methods),
            );
        }
        if !provided_methods.is_empty() {
            properties.insert(
                "provided_methods".to_string(),
                PropertyValue::from(provided_methods),
            );
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if !attributes.is_empty() {
            properties.insert("attributes".to_string(), PropertyValue::from(attributes));
        }
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Trait,
            properties,
        }
    }

    /// Create an Impl node
    #[allow(clippy::too_many_arguments)]
    pub fn create_impl(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        impl_type: &str, // "inherent" or "trait"
        trait_name: Option<&str>,
        for_type: Option<&str>,
        has_generics: bool,
        generic_params: Vec<String>,
        methods: Vec<String>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("impl_type".to_string(), PropertyValue::from(impl_type));
        if let Some(t) = trait_name {
            properties.insert("trait_name".to_string(), PropertyValue::from(t));
        }
        if let Some(t) = for_type {
            properties.insert("for_type".to_string(), PropertyValue::from(t));
        }
        properties.insert(
            "has_generics".to_string(),
            PropertyValue::from(has_generics),
        );
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        if !methods.is_empty() {
            properties.insert("methods".to_string(), PropertyValue::from(methods));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Impl,
            properties,
        }
    }

    /// Create a Type node (for type references in code)
    pub fn create_type(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        type_kind: &str, // "primitive", "generic", "user", "tuple", "reference", etc.
        is_generic_param: bool,
        concrete_type: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("type_kind".to_string(), PropertyValue::from(type_kind));
        properties.insert(
            "is_generic_param".to_string(),
            PropertyValue::from(is_generic_param),
        );
        if let Some(ct) = concrete_type {
            properties.insert("concrete_type".to_string(), PropertyValue::from(ct));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Type,
            properties,
        }
    }

    /// Create a TypeAlias node
    #[allow(clippy::too_many_arguments)]
    pub fn create_type_alias(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        target_type: &str,
        has_generics: bool,
        generic_params: Vec<String>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert("target_type".to_string(), PropertyValue::from(target_type));
        properties.insert(
            "has_generics".to_string(),
            PropertyValue::from(has_generics),
        );
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::TypeAlias,
            properties,
        }
    }

    /// Create a Const node
    #[allow(clippy::too_many_arguments)]
    pub fn create_const(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        const_type: &str,
        value: Option<&str>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert("const_type".to_string(), PropertyValue::from(const_type));
        if let Some(v) = value {
            properties.insert("value".to_string(), PropertyValue::from(v));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Const,
            properties,
        }
    }

    /// Create a Static node
    #[allow(clippy::too_many_arguments)]
    pub fn create_static(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        static_type: &str,
        is_mutable: bool,
        value: Option<&str>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert("static_type".to_string(), PropertyValue::from(static_type));
        properties.insert("is_mutable".to_string(), PropertyValue::from(is_mutable));
        if let Some(v) = value {
            properties.insert("value".to_string(), PropertyValue::from(v));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Static,
            properties,
        }
    }

    /// Create a Macro node
    #[allow(clippy::too_many_arguments)]
    pub fn create_macro(
        id: impl Into<String>,
        fqn: impl Into<String>,
        name: impl Into<String>,
        visibility: &str,
        is_proc_macro: bool,
        is_exported: bool,
        signature: Option<&str>,
        start_line: usize,
        end_line: usize,
        file_path: &str,
        attributes: Vec<String>,
        doc_comment: Option<&str>,
    ) -> NodeData {
        let mut properties = HashMap::new();

        properties.insert("visibility".to_string(), PropertyValue::from(visibility));
        properties.insert(
            "is_proc_macro".to_string(),
            PropertyValue::from(is_proc_macro),
        );
        properties.insert("is_exported".to_string(), PropertyValue::from(is_exported));
        if let Some(sig) = signature {
            properties.insert("signature".to_string(), PropertyValue::from(sig));
        }
        properties.insert("start_line".to_string(), PropertyValue::from(start_line));
        properties.insert("end_line".to_string(), PropertyValue::from(end_line));
        properties.insert("file_path".to_string(), PropertyValue::from(file_path));
        if !attributes.is_empty() {
            properties.insert("attributes".to_string(), PropertyValue::from(attributes));
        }
        if let Some(doc) = doc_comment {
            properties.insert("doc_comment".to_string(), PropertyValue::from(doc));
        }

        NodeData {
            id: id.into(),
            fqn: fqn.into(),
            name: name.into(),
            node_type: NodeType::Macro,
            properties,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_function_node() {
        let node = NodeBuilder::create_function(
            "test::module::my_func",
            "test::module::my_func",
            "my_func",
            Some("pub fn my_func<T: Clone>(x: T) -> T"),
            "public",
            false,
            false,
            false,
            10,
            15,
            "src/module.rs",
            vec!["T".to_string()],
            vec!["T: Clone".to_string()],
            Some("A test function"),
        );

        assert_eq!(node.id, "test::module::my_func");
        assert_eq!(node.node_type, NodeType::Function);
        assert!(node.properties.contains_key("signature"));
    }

    #[test]
    fn test_create_struct_node() {
        let node = NodeBuilder::create_struct(
            "test::MyStruct",
            "test::MyStruct",
            "MyStruct",
            "public",
            false,
            true,
            vec!["T".to_string()],
            20,
            30,
            "src/lib.rs",
            vec!["derive(Clone)".to_string()],
            Some("A generic struct"),
        );

        assert_eq!(node.node_type, NodeType::Struct);
        assert!(node.properties.contains_key("has_generics"));
    }
}
