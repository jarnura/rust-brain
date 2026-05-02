//! Event types produced by the parse/extract pipeline and consumed by projector-pg.
//!
//! Two top-level event categories:
//! - [`ItemEvent`] — source files and extracted code items.
//! - [`RelationEvent`] — call sites and trait implementations.
//!
//! Both are wrapped in [`ProjectorEvent`] for unified channel transport.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// =============================================================================
// Item events
// =============================================================================

/// A source file to upsert. Conflict key: `(crate_name, module_path, file_path)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceFileEvent {
    /// Stable identifier for this file within the run.
    pub id: Uuid,
    pub crate_name: String,
    pub module_path: String,
    pub file_path: String,
    pub original_source: String,
    pub expanded_source: Option<String>,
    pub content_hash: Option<String>,
    pub git_hash: Option<String>,
}

/// An extracted code item to upsert. Conflict key: `fqn` (UNIQUE).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractedItemEvent {
    /// Resolved source file UUID (may be `None` for items without a known file).
    pub source_file_id: Option<Uuid>,
    /// Item kind: `"function"`, `"struct"`, `"enum"`, etc.
    pub item_type: String,
    /// Fully qualified name — the idempotency key.
    pub fqn: String,
    pub name: String,
    /// Visibility string: `"pub"`, `"pub_crate"`, `"private"`, etc.
    pub visibility: String,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub start_line: i32,
    pub end_line: i32,
    pub body_source: Option<String>,
    /// JSON array of `GenericParam` objects.
    pub generic_params: serde_json::Value,
    /// JSON array of `WhereClause` objects.
    pub where_clauses: serde_json::Value,
    /// JSON array of attribute strings.
    pub attributes: serde_json::Value,
    pub generated_by: Option<String>,
}

/// Discriminated union of item-level events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemEvent {
    SourceFile(SourceFileEvent),
    ExtractedItem(ExtractedItemEvent),
}

// =============================================================================
// Relation events
// =============================================================================

/// A call-site relationship between two items. No unique constraint — each
/// (caller, callee, file, line) combination is independent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CallSiteEvent {
    pub caller_fqn: String,
    pub callee_fqn: String,
    pub file_path: String,
    pub line_number: i32,
    /// JSON array of concrete type argument strings.
    pub concrete_type_args: serde_json::Value,
    pub is_monomorphized: bool,
    /// Resolution confidence: `"analyzed"` or `"heuristic"`.
    pub quality: String,
}

/// A trait implementation relationship. Conflict key: `impl_fqn` (UNIQUE).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraitImplEvent {
    pub trait_fqn: String,
    pub self_type: String,
    /// Fully qualified impl identifier — the idempotency key.
    pub impl_fqn: String,
    pub file_path: String,
    pub line_number: i32,
    /// JSON array of `GenericParam` objects.
    pub generic_params: serde_json::Value,
    /// Resolution confidence: `"analyzed"` or `"heuristic"`.
    pub quality: String,
}

/// Discriminated union of relation-level events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelationEvent {
    CallSite(CallSiteEvent),
    TraitImpl(TraitImplEvent),
}

// =============================================================================
// Top-level projector event
// =============================================================================

/// Unified envelope for all events handled by the PG projector.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum ProjectorEvent {
    Item(ItemEvent),
    Relation(RelationEvent),
}

impl ProjectorEvent {
    /// Convenience constructor for source-file events.
    pub fn source_file(ev: SourceFileEvent) -> Self {
        Self::Item(ItemEvent::SourceFile(ev))
    }

    /// Convenience constructor for extracted-item events.
    pub fn extracted_item(ev: ExtractedItemEvent) -> Self {
        Self::Item(ItemEvent::ExtractedItem(ev))
    }

    /// Convenience constructor for call-site events.
    pub fn call_site(ev: CallSiteEvent) -> Self {
        Self::Relation(RelationEvent::CallSite(ev))
    }

    /// Convenience constructor for trait-impl events.
    pub fn trait_impl(ev: TraitImplEvent) -> Self {
        Self::Relation(RelationEvent::TraitImpl(ev))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_source_file() -> SourceFileEvent {
        SourceFileEvent {
            id: Uuid::new_v4(),
            crate_name: "my_crate".into(),
            module_path: "my_crate::lib".into(),
            file_path: "src/lib.rs".into(),
            original_source: "fn main() {}".into(),
            expanded_source: None,
            content_hash: Some("abc123".into()),
            git_hash: None,
        }
    }

    fn sample_extracted_item() -> ExtractedItemEvent {
        ExtractedItemEvent {
            source_file_id: Some(Uuid::new_v4()),
            item_type: "function".into(),
            fqn: "my_crate::lib::main".into(),
            name: "main".into(),
            visibility: "pub".into(),
            signature: Some("fn main()".into()),
            doc_comment: None,
            start_line: 1,
            end_line: 3,
            body_source: Some("{ }".into()),
            generic_params: serde_json::json!([]),
            where_clauses: serde_json::json!([]),
            attributes: serde_json::json!([]),
            generated_by: None,
        }
    }

    fn sample_call_site() -> CallSiteEvent {
        CallSiteEvent {
            caller_fqn: "my_crate::lib::main".into(),
            callee_fqn: "std::println".into(),
            file_path: "src/lib.rs".into(),
            line_number: 2,
            concrete_type_args: serde_json::json!([]),
            is_monomorphized: false,
            quality: "heuristic".into(),
        }
    }

    fn sample_trait_impl() -> TraitImplEvent {
        TraitImplEvent {
            trait_fqn: "std::fmt::Display".into(),
            self_type: "my_crate::MyStruct".into(),
            impl_fqn: "my_crate::MyStruct::impl_Display".into(),
            file_path: "src/lib.rs".into(),
            line_number: 10,
            generic_params: serde_json::json!([]),
            quality: "analyzed".into(),
        }
    }

    #[test]
    fn projector_event_source_file_roundtrip() {
        let ev = ProjectorEvent::source_file(sample_source_file());
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["category"], "item");
        assert_eq!(json["kind"], "source_file");
        let restored: ProjectorEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, restored);
    }

    #[test]
    fn projector_event_extracted_item_roundtrip() {
        let ev = ProjectorEvent::extracted_item(sample_extracted_item());
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["category"], "item");
        assert_eq!(json["kind"], "extracted_item");
        let restored: ProjectorEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, restored);
    }

    #[test]
    fn projector_event_call_site_roundtrip() {
        let ev = ProjectorEvent::call_site(sample_call_site());
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["category"], "relation");
        assert_eq!(json["kind"], "call_site");
        let restored: ProjectorEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, restored);
    }

    #[test]
    fn projector_event_trait_impl_roundtrip() {
        let ev = ProjectorEvent::trait_impl(sample_trait_impl());
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["category"], "relation");
        assert_eq!(json["kind"], "trait_impl");
        let restored: ProjectorEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, restored);
    }

    #[test]
    fn extracted_item_fqn_is_idempotency_key() {
        let item = sample_extracted_item();
        assert!(!item.fqn.is_empty(), "fqn must not be empty");
        assert!(item.fqn.contains("::"), "fqn must be fully qualified");
    }

    #[test]
    fn trait_impl_fqn_is_idempotency_key() {
        let ti = sample_trait_impl();
        assert!(!ti.impl_fqn.is_empty(), "impl_fqn must not be empty");
    }

    #[test]
    fn quality_values_are_valid() {
        for q in ["analyzed", "heuristic"] {
            let cs = CallSiteEvent {
                quality: q.into(),
                ..sample_call_site()
            };
            assert_eq!(cs.quality, q);
        }
    }
}
