//! Relationship creation functions for Neo4j graph
//!
//! Provides functions to create relationships between code elements:
//! CONTAINS, CALLS, RETURNS, ACCEPTS, IMPLEMENTS, HAS_FIELD, HAS_VARIANT, MONOMORPHIZED_AS,
//! DEPENDS_ON, HAS_METHOD

use anyhow::{Context, Result};
use neo4rs::{query, BoltType, Graph};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

/// Relationship types supported by the graph
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum RelationshipType {
    /// Crate→Module, Module→Module, Module→Items containment
    CONTAINS,
    /// Function→Function call relationship
    CALLS,
    /// Function→Type return relationship
    RETURNS,
    /// Function→Type parameter relationship
    ACCEPTS,
    /// Impl→Trait implementation relationship
    IMPLEMENTS,
    /// Impl→Type "for" relationship
    FOR,
    /// Struct→Type field relationship
    HAS_FIELD,
    /// Enum→Type variant relationship
    HAS_VARIANT,
    /// Type→Type monomorphization relationship
    MONOMORPHIZED_AS,
    /// Trait→Trait inheritance
    EXTENDS,
    /// Function→Macro expansion
    EXPANDS_TO,
    /// Module→Module import
    IMPORTS,
    /// Function/Method/Struct/Enum→Type usage relationship
    USES_TYPE,
    /// Crate→Crate workspace dependency
    DEPENDS_ON,
    /// Trait→Function method relationship
    HAS_METHOD,
}

impl RelationshipType {
    /// Get the relationship type name for Neo4j
    pub fn name(&self) -> &'static str {
        match self {
            RelationshipType::CONTAINS => "CONTAINS",
            RelationshipType::CALLS => "CALLS",
            RelationshipType::RETURNS => "RETURNS",
            RelationshipType::ACCEPTS => "ACCEPTS",
            RelationshipType::IMPLEMENTS => "IMPLEMENTS",
            RelationshipType::FOR => "FOR",
            RelationshipType::HAS_FIELD => "HAS_FIELD",
            RelationshipType::HAS_VARIANT => "HAS_VARIANT",
            RelationshipType::MONOMORPHIZED_AS => "MONOMORPHIZED_AS",
            RelationshipType::EXTENDS => "EXTENDS",
            RelationshipType::EXPANDS_TO => "EXPANDS_TO",
            RelationshipType::IMPORTS => "IMPORTS",
            RelationshipType::USES_TYPE => "USES_TYPE",
            RelationshipType::DEPENDS_ON => "DEPENDS_ON",
            RelationshipType::HAS_METHOD => "HAS_METHOD",
        }
    }
}

impl std::fmt::Display for RelationshipType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Relationship data for creating Neo4j relationships
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipData {
    /// Source node ID
    pub from_id: String,
    /// Target node ID
    pub to_id: String,
    /// Source node label (e.g. "Function", "Module") for indexed MATCH
    pub from_label: String,
    /// Target node label for indexed MATCH
    pub to_label: String,
    /// Relationship type
    pub rel_type: RelationshipType,
    /// Additional properties on the relationship
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

/// Builder for creating relationships in Neo4j
pub struct RelationshipBuilder {
    graph: Arc<Graph>,
}

impl RelationshipBuilder {
    /// Create a new relationship builder
    pub fn new(graph: Arc<Graph>) -> Self {
        Self { graph }
    }

    /// Create a relationship using MERGE for idempotency
    pub async fn merge_relationship(&self, rel: &RelationshipData) -> Result<()> {
        let query_str = format!(
            "MATCH (from:{} {{id: $from_id}}) \
             MATCH (to:{} {{id: $to_id}}) \
             MERGE (from)-[r:{}]->(to) \
             SET r += $props",
            rel.from_label,
            rel.to_label,
            rel.rel_type.name()
        );

        let props: HashMap<String, BoltType> = rel
            .properties
            .iter()
            .filter_map(|(k, v)| v.to_bolt_type().map(|bv| (k.clone(), bv)))
            .collect();

        self.graph
            .run(
                query(&query_str)
                    .param("from_id", rel.from_id.as_str())
                    .param("to_id", rel.to_id.as_str())
                    .param("props", props),
            )
            .await
            .context("Failed to merge relationship")?;

        debug!(
            "Merged relationship: {} -[{}]-> {}",
            rel.from_id, rel.rel_type, rel.to_id
        );
        Ok(())
    }

    /// Create CONTAINS relationship
    /// Used for: Crate→Module, Module→Module, Module→Items
    pub fn create_contains(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        from_label: impl Into<String>,
        to_label: impl Into<String>,
    ) -> RelationshipData {
        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: from_label.into(),
            to_label: to_label.into(),
            rel_type: RelationshipType::CONTAINS,
            properties: HashMap::new(),
        }
    }

    /// Create CALLS relationship
    /// Used for: Function→Function with call site information
    pub fn create_calls(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        line: usize,
        file: impl Into<String>,
        concrete_types: Vec<String>,
        is_static_dispatch: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert("line".to_string(), PropertyValue::from(line));
        properties.insert("file".to_string(), PropertyValue::from(file.into()));
        if !concrete_types.is_empty() {
            properties.insert(
                "concrete_types".to_string(),
                PropertyValue::from(concrete_types),
            );
        }
        properties.insert(
            "is_static_dispatch".to_string(),
            PropertyValue::from(is_static_dispatch),
        );

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Function".to_string(),
            to_label: "Function".to_string(),
            rel_type: RelationshipType::CALLS,
            properties,
        }
    }

    /// Create RETURNS relationship
    /// Used for: Function→Type
    pub fn create_returns(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        is_option: bool,
        is_result: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert("is_option".to_string(), PropertyValue::from(is_option));
        properties.insert("is_result".to_string(), PropertyValue::from(is_result));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Function".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::RETURNS,
            properties,
        }
    }

    /// Create ACCEPTS relationship
    /// Used for: Function→Type (parameter)
    pub fn create_accepts(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        param_name: impl Into<String>,
        param_position: usize,
        is_mut: bool,
        is_ref: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert(
            "param_name".to_string(),
            PropertyValue::from(param_name.into()),
        );
        properties.insert(
            "param_position".to_string(),
            PropertyValue::from(param_position),
        );
        properties.insert("is_mut".to_string(), PropertyValue::from(is_mut));
        properties.insert("is_ref".to_string(), PropertyValue::from(is_ref));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Function".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::ACCEPTS,
            properties,
        }
    }

    /// Create IMPLEMENTS relationship
    /// Used for: Impl→Trait
    pub fn create_implements(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
    ) -> RelationshipData {
        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Impl".to_string(),
            to_label: "Trait".to_string(),
            rel_type: RelationshipType::IMPLEMENTS,
            properties: HashMap::new(),
        }
    }

    /// Create FOR relationship
    /// Used for: Impl→Type (the type being implemented for)
    pub fn create_for(from_id: impl Into<String>, to_id: impl Into<String>) -> RelationshipData {
        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Impl".to_string(),
            to_label: "Struct".to_string(),
            rel_type: RelationshipType::FOR,
            properties: HashMap::new(),
        }
    }

    /// Create HAS_FIELD relationship
    /// Used for: Struct→Type
    pub fn create_has_field(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        field_name: impl Into<String>,
        field_position: usize,
        is_pub: bool,
        has_default: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert(
            "field_name".to_string(),
            PropertyValue::from(field_name.into()),
        );
        properties.insert(
            "field_position".to_string(),
            PropertyValue::from(field_position),
        );
        properties.insert("is_pub".to_string(), PropertyValue::from(is_pub));
        properties.insert("has_default".to_string(), PropertyValue::from(has_default));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Struct".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::HAS_FIELD,
            properties,
        }
    }

    /// Create HAS_VARIANT relationship
    /// Used for: Enum→Type (variant type)
    pub fn create_has_variant(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        variant_name: impl Into<String>,
        variant_position: usize,
        has_data: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert(
            "variant_name".to_string(),
            PropertyValue::from(variant_name.into()),
        );
        properties.insert(
            "variant_position".to_string(),
            PropertyValue::from(variant_position),
        );
        properties.insert("has_data".to_string(), PropertyValue::from(has_data));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Enum".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::HAS_VARIANT,
            properties,
        }
    }

    /// Create MONOMORPHIZED_AS relationship
    /// Used for: Type→Type (generic to concrete)
    pub fn create_monomorphized_as(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        generic_params: Vec<String>,
        concrete_params: Vec<String>,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        if !generic_params.is_empty() {
            properties.insert(
                "generic_params".to_string(),
                PropertyValue::from(generic_params),
            );
        }
        if !concrete_params.is_empty() {
            properties.insert(
                "concrete_params".to_string(),
                PropertyValue::from(concrete_params),
            );
        }

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Type".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::MONOMORPHIZED_AS,
            properties,
        }
    }

    /// Create EXTENDS relationship
    /// Used for: Trait→Trait (trait inheritance)
    pub fn create_extends(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
    ) -> RelationshipData {
        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Trait".to_string(),
            to_label: "Trait".to_string(),
            rel_type: RelationshipType::EXTENDS,
            properties: HashMap::new(),
        }
    }

    /// Create EXPANDS_TO relationship
    /// Used for: Function→Macro (macro expansion)
    pub fn create_expands_to(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
    ) -> RelationshipData {
        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Function".to_string(),
            to_label: "Macro".to_string(),
            rel_type: RelationshipType::EXPANDS_TO,
            properties: HashMap::new(),
        }
    }

    /// Create IMPORTS relationship
    /// Used for: Module→Module/Item (use statements)
    pub fn create_imports(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        alias: Option<impl Into<String>>,
        is_glob: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        if let Some(a) = alias {
            properties.insert("alias".to_string(), PropertyValue::from(a.into()));
        }
        properties.insert("is_glob".to_string(), PropertyValue::from(is_glob));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Module".to_string(),
            to_label: "Module".to_string(),
            rel_type: RelationshipType::IMPORTS,
            properties,
        }
    }

    /// Create DEPENDS_ON relationship
    /// Used for: Crate→Crate (workspace dependency)
    pub fn create_depends_on(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        is_dev: bool,
        is_build: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert("is_dev".to_string(), PropertyValue::from(is_dev));
        properties.insert("is_build".to_string(), PropertyValue::from(is_build));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Crate".to_string(),
            to_label: "Crate".to_string(),
            rel_type: RelationshipType::DEPENDS_ON,
            properties,
        }
    }

    /// Create HAS_METHOD relationship
    /// Used for: Trait→Function (trait method declaration)
    pub fn create_has_method(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        is_required: bool,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert("is_required".to_string(), PropertyValue::from(is_required));

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Trait".to_string(),
            to_label: "Function".to_string(),
            rel_type: RelationshipType::HAS_METHOD,
            properties,
        }
    }

    /// Create USES_TYPE relationship
    /// Used for: Function/Method/Struct/Enum→Type (type usage in code)
    pub fn create_uses_type(
        from_id: impl Into<String>,
        to_id: impl Into<String>,
        usage_context: impl Into<String>, // "parameter", "return", "body", "field", "generic"
        line: Option<usize>,
    ) -> RelationshipData {
        let mut properties = HashMap::new();
        properties.insert(
            "usage_context".to_string(),
            PropertyValue::from(usage_context.into()),
        );
        if let Some(l) = line {
            properties.insert("line".to_string(), PropertyValue::from(l));
        }

        RelationshipData {
            from_id: from_id.into(),
            to_id: to_id.into(),
            from_label: "Function".to_string(),
            to_label: "Type".to_string(),
            rel_type: RelationshipType::USES_TYPE,
            properties,
        }
    }
}

/// Batch insert relationships using UNWIND for efficiency
// TODO: used by future bulk-ingestion stage
#[allow(dead_code)]
pub async fn batch_insert_relationships(
    graph: &Graph,
    relationships: &[RelationshipData],
    batch_size: usize,
) -> Result<()> {
    if relationships.is_empty() {
        return Ok(());
    }

    // Group by (rel_type, from_label, to_label) so MATCH uses node labels for index hits
    let mut grouped: HashMap<(RelationshipType, String, String), Vec<&RelationshipData>> =
        HashMap::new();
    for rel in relationships {
        grouped
            .entry((rel.rel_type, rel.from_label.clone(), rel.to_label.clone()))
            .or_default()
            .push(rel);
    }

    for ((rel_type, from_label, to_label), group_rels) in grouped {
        // Process in batches
        for chunk in group_rels.chunks(batch_size) {
            let query_str = format!(
                "UNWIND $rels AS rel_data \
                 MATCH (from:{} {{id: rel_data.from_id}}) \
                 MATCH (to:{} {{id: rel_data.to_id}}) \
                 MERGE (from)-[r:{}]->(to) \
                 SET r += rel_data.props",
                from_label,
                to_label,
                rel_type.name()
            );

            let rel_params: Vec<HashMap<String, BoltType>> = chunk
                .iter()
                .map(|rel| {
                    let props: HashMap<String, BoltType> = rel
                        .properties
                        .iter()
                        .filter_map(|(k, v)| v.to_bolt_type().map(|bv| (k.clone(), bv)))
                        .collect();

                    let mut rel_param = HashMap::new();
                    rel_param.insert("from_id".to_string(), BoltType::from(rel.from_id.as_str()));
                    rel_param.insert("to_id".to_string(), BoltType::from(rel.to_id.as_str()));
                    rel_param.insert("props".to_string(), BoltType::from(props));
                    rel_param
                })
                .collect();

            graph
                .run(query(&query_str).param("rels", rel_params))
                .await
                .context(format!(
                    "Failed to batch insert {} relationships",
                    rel_type.name()
                ))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_calls_relationship() {
        let rel = RelationshipBuilder::create_calls(
            "crate::module::caller",
            "crate::module::callee",
            42,
            "src/module.rs",
            vec!["String".to_string()],
            true,
        );

        assert_eq!(rel.rel_type, RelationshipType::CALLS);
        assert!(rel.properties.contains_key("line"));
        assert!(rel.properties.contains_key("file"));
        assert!(rel.properties.contains_key("concrete_types"));
    }

    #[test]
    fn test_create_has_field_relationship() {
        let rel = RelationshipBuilder::create_has_field(
            "crate::MyStruct",
            "crate::FieldType",
            "my_field",
            0,
            true,
            false,
        );

        assert_eq!(rel.rel_type, RelationshipType::HAS_FIELD);
        assert!(rel.properties.contains_key("field_name"));
        assert!(rel.properties.contains_key("is_pub"));
    }

    #[test]
    fn test_create_depends_on_relationship() {
        let rel = RelationshipBuilder::create_depends_on("my_crate", "dep_crate", false, false);

        assert_eq!(rel.rel_type, RelationshipType::DEPENDS_ON);
        assert_eq!(rel.from_label, "Crate");
        assert_eq!(rel.to_label, "Crate");
        assert_eq!(rel.from_id, "my_crate");
        assert_eq!(rel.to_id, "dep_crate");
        assert!(rel.properties.contains_key("is_dev"));
        assert!(rel.properties.contains_key("is_build"));
    }

    #[test]
    fn test_depends_on_properties() {
        let dev_dep = RelationshipBuilder::create_depends_on("my_crate", "dep_crate", true, false);
        match dev_dep.properties.get("is_dev") {
            Some(PropertyValue::Bool(v)) => assert!(*v),
            other => panic!("is_dev should be Bool(true), got {:?}", other),
        }
        match dev_dep.properties.get("is_build") {
            Some(PropertyValue::Bool(v)) => assert!(!*v),
            other => panic!("is_build should be Bool(false), got {:?}", other),
        }

        let build_dep =
            RelationshipBuilder::create_depends_on("my_crate", "build_crate", false, true);
        match build_dep.properties.get("is_dev") {
            Some(PropertyValue::Bool(v)) => assert!(!*v),
            other => panic!("is_dev should be Bool(false), got {:?}", other),
        }
        match build_dep.properties.get("is_build") {
            Some(PropertyValue::Bool(v)) => assert!(*v),
            other => panic!("is_build should be Bool(true), got {:?}", other),
        }
    }

    #[test]
    fn test_create_has_method_relationship() {
        let rel = RelationshipBuilder::create_has_method(
            "crate::MyTrait",
            "crate::MyTrait::my_method",
            true,
        );

        assert_eq!(rel.rel_type, RelationshipType::HAS_METHOD);
        assert_eq!(rel.from_label, "Trait");
        assert_eq!(rel.to_label, "Function");
        assert_eq!(rel.from_id, "crate::MyTrait");
        assert_eq!(rel.to_id, "crate::MyTrait::my_method");
        assert!(rel.properties.contains_key("is_required"));
    }

    #[test]
    fn test_has_method_properties() {
        let required = RelationshipBuilder::create_has_method(
            "crate::MyTrait",
            "crate::MyTrait::required_method",
            true,
        );
        match required.properties.get("is_required") {
            Some(PropertyValue::Bool(v)) => assert!(*v),
            other => panic!("is_required should be Bool(true), got {:?}", other),
        }

        let provided = RelationshipBuilder::create_has_method(
            "crate::MyTrait",
            "crate::MyTrait::provided_method",
            false,
        );
        match provided.properties.get("is_required") {
            Some(PropertyValue::Bool(v)) => assert!(!*v),
            other => panic!("is_required should be Bool(false), got {:?}", other),
        }
    }

    #[test]
    fn test_create_monomorphized_as_relationship() {
        let rel = RelationshipBuilder::create_monomorphized_as(
            "crate::Container<T>",
            "crate::Container<String>",
            vec!["T".to_string()],
            vec!["String".to_string()],
        );

        assert_eq!(rel.rel_type, RelationshipType::MONOMORPHIZED_AS);
        assert!(rel.properties.contains_key("generic_params"));
        assert!(rel.properties.contains_key("concrete_params"));
    }
}
