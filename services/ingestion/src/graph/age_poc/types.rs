//! AGE agtype parsing utilities
//!
//! Apache AGE returns results as PostgreSQL `agtype` (a JSONB-like type).
//! This module converts agtype strings into Rust types for the POC.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A node returned from an AGE query, parsed from agtype JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgeNode {
    pub id: String,
    pub label: String,
    pub properties: HashMap<String, AgeValue>,
}

/// A relationship returned from an AGE query, parsed from agtype JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgeRelationship {
    pub id: String,
    pub label: String,
    pub from_id: String,
    pub to_id: String,
    pub properties: HashMap<String, AgeValue>,
}

/// A value in an AGE result (mirrors agtype scalar types).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum AgeValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Null,
    Array(Vec<AgeValue>),
    Object(HashMap<String, AgeValue>),
}

/// Raw agtype row returned from a cypher() query.
/// AGE wraps results in a vertex/edge JSON structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgeRow {
    /// The agtype result as a serde_json::Value for flexible parsing
    pub result: serde_json::Value,
}

/// Parse an agtype vertex result from a cypher() query.
///
/// AGE returns vertices as JSON objects with structure:
/// ```json
/// {"id": "844424930131969", "label": "Function", "properties": {"id": "my::fqn", ...}}
/// ```
pub fn parse_age_vertex(agtype_str: &str) -> Option<AgeNode> {
    let val: serde_json::Value = serde_json::from_str(agtype_str).ok()?;

    let label = val.get("label")?.as_str()?.to_string();
    let id = val
        .get("properties")
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let properties = val
        .get("properties")
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    Some(AgeNode {
        id,
        label,
        properties,
    })
}

/// Parse an agtype edge result from a cypher() query.
pub fn parse_age_edge(agtype_str: &str) -> Option<AgeRelationship> {
    let val: serde_json::Value = serde_json::from_str(agtype_str).ok()?;

    let id = val.get("id")?.as_str()?.to_string();
    let label = val.get("label")?.as_str()?.to_string();
    let from_id = val.get("start_id")?.as_str()?.to_string();
    let to_id = val.get("end_id")?.as_str()?.to_string();

    let properties = val
        .get("properties")
        .and_then(|p| serde_json::from_value(p.clone()).ok())
        .unwrap_or_default();

    Some(AgeRelationship {
        id,
        label,
        from_id,
        to_id,
        properties,
    })
}

/// Build an agtype parameter map for passing to cypher() function.
///
/// Converts a HashMap of string→value into the JSON agtype format that AGE expects
/// as the third argument to cypher().
pub fn build_agtype_param(params: &HashMap<String, serde_json::Value>) -> String {
    serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string())
}

/// Build an agtype parameter for batch operations (UNWIND pattern).
///
/// Takes a list of items and wraps them as `{"items": [...]}` for
/// `UNWIND $items AS item` patterns.
pub fn build_unwind_param(items: &[serde_json::Value], key: &str) -> String {
    let param = serde_json::json!({ key: items });
    serde_json::to_string(&param).unwrap_or_else(|_| format!("{{\"{}\": []}}", key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_age_vertex() {
        let agtype = r#"{"id": "844424930131969", "label": "Function", "properties": {"id": "crate::my_func", "fqn": "crate::my_func", "name": "my_func"}}"#;
        let node = parse_age_vertex(agtype).unwrap();
        assert_eq!(node.id, "crate::my_func");
        assert_eq!(node.label, "Function");
        assert!(node.properties.contains_key("fqn"));
    }

    #[test]
    fn test_parse_age_vertex_missing_properties() {
        let agtype = r#"{"id": "123", "label": "Type", "properties": {}}"#;
        let node = parse_age_vertex(agtype).unwrap();
        assert_eq!(node.id, "");
        assert_eq!(node.label, "Type");
        assert!(node.properties.is_empty());
    }

    #[test]
    fn test_parse_age_edge() {
        let agtype = r#"{"id": "844424930132000", "label": "CALLS", "start_id": "844424930131969", "end_id": "844424930132100", "properties": {"line": 42}}"#;
        let rel = parse_age_edge(agtype).unwrap();
        assert_eq!(rel.label, "CALLS");
        assert_eq!(rel.from_id, "844424930131969");
        assert_eq!(rel.to_id, "844424930132100");
    }

    #[test]
    fn test_parse_invalid_agtype() {
        assert!(parse_age_vertex("not json").is_none());
        assert!(parse_age_edge("").is_none());
    }

    #[test]
    fn test_build_agtype_param() {
        let mut params = HashMap::new();
        params.insert("id".to_string(), serde_json::json!("my::fqn"));
        params.insert("name".to_string(), serde_json::json!("fqn"));

        let result = build_agtype_param(&params);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["id"], "my::fqn");
        assert_eq!(parsed["name"], "fqn");
    }

    #[test]
    fn test_build_unwind_param() {
        let items = vec![
            serde_json::json!({"id": "1", "name": "a"}),
            serde_json::json!({"id": "2", "name": "b"}),
        ];
        let result = build_unwind_param(&items, "nodes");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_age_value_deserialization() {
        let v: AgeValue = serde_json::from_str("\"hello\"").unwrap();
        assert_eq!(v, AgeValue::String("hello".to_string()));

        let v: AgeValue = serde_json::from_str("42").unwrap();
        assert_eq!(v, AgeValue::Integer(42));

        let v: AgeValue = serde_json::from_str("true").unwrap();
        assert_eq!(v, AgeValue::Bool(true));

        let v: AgeValue = serde_json::from_str("null").unwrap();
        assert_eq!(v, AgeValue::Null);
    }
}
