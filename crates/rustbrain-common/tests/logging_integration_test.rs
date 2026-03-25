//! Integration tests verifying logging output for key public functions.
//!
//! Uses `tracing-test` to capture log records emitted during each call and
//! assert that the expected structured fields / messages are present.
//! Every test is independent: `#[traced_test]` installs a per-test subscriber
//! so there is no global-state cross-contamination between tests.

// ─────────────────────────────────────────────────────────────────────────────
// Sanity: verify tracing-test subscriber captures logs at all levels
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn sanity_info_captured() {
    tracing::info!("sanity info message");
    assert!(logs_contain("sanity info message"));
}

#[traced_test]
#[test]
fn sanity_debug_captured() {
    tracing::debug!("sanity debug message");
    assert!(logs_contain("sanity debug message"));
}

#[traced_test]
#[test]
fn sanity_trace_captured() {
    tracing::trace!("sanity trace message");
    assert!(logs_contain("sanity trace message"));
}

use rustbrain_common::{
    config::EmbeddingModelConfig,
    types::{ItemType, ResolutionQuality, StoreReference, Visibility},
};
use tracing_test::traced_test;

// ─────────────────────────────────────────────────────────────────────────────
// Visibility::as_str
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn visibility_as_str_public_emits_trace() {
    let s = Visibility::Public.as_str();
    assert_eq!(s, "pub");
    assert!(logs_contain("Visibility::as_str"));
}

#[traced_test]
#[test]
fn visibility_as_str_pub_crate_emits_trace() {
    let s = Visibility::PubCrate.as_str();
    assert_eq!(s, "pub_crate");
    assert!(logs_contain("Visibility::as_str"));
}

#[traced_test]
#[test]
fn visibility_as_str_pub_super_emits_trace() {
    let s = Visibility::PubSuper.as_str();
    assert_eq!(s, "pub_super");
    assert!(logs_contain("Visibility::as_str"));
}

#[traced_test]
#[test]
fn visibility_as_str_private_emits_trace() {
    let s = Visibility::Private.as_str();
    assert_eq!(s, "private");
    assert!(logs_contain("Visibility::as_str"));
}

#[traced_test]
#[test]
fn visibility_as_str_pub_in_emits_trace_with_path() {
    let v = Visibility::PubIn("some::module".to_string());
    let s = v.as_str();
    assert_eq!(s, "some::module");
    assert!(logs_contain("Visibility::as_str"));
}

// ─────────────────────────────────────────────────────────────────────────────
// ItemType::as_str
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn item_type_function_emits_trace() {
    let s = ItemType::Function.as_str();
    assert_eq!(s, "function");
    assert!(logs_contain("ItemType::as_str"));
}

#[traced_test]
#[test]
fn item_type_struct_emits_trace() {
    let s = ItemType::Struct.as_str();
    assert_eq!(s, "struct");
    assert!(logs_contain("ItemType::as_str"));
}

#[traced_test]
#[test]
fn item_type_unknown_emits_trace_with_custom_value() {
    let it = ItemType::Unknown("my_type".to_string());
    let s = it.as_str();
    assert_eq!(s, "my_type");
    assert!(logs_contain("ItemType::as_str"));
}

/// Verify every non-Unknown variant returns the correct canonical string
/// and that each call emits a trace record.
#[traced_test]
#[test]
fn item_type_all_standard_variants_log_and_return_correct_str() {
    let cases = [
        (ItemType::Function, "function"),
        (ItemType::Struct, "struct"),
        (ItemType::Enum, "enum"),
        (ItemType::Trait, "trait"),
        (ItemType::Impl, "impl"),
        (ItemType::TypeAlias, "type_alias"),
        (ItemType::Const, "const"),
        (ItemType::Static, "static"),
        (ItemType::Macro, "macro"),
        (ItemType::Module, "module"),
        (ItemType::Use, "use"),
        (ItemType::ExternBlock, "extern_block"),
    ];
    for (item_type, expected) in cases {
        assert_eq!(item_type.as_str(), expected, "mismatch for {:?}", expected);
    }
    assert!(logs_contain("ItemType::as_str"));
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReference::new
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn store_reference_new_emits_debug_log() {
    let r = StoreReference::new("my_crate::Foo".to_string(), "my_crate".to_string());
    assert_eq!(r.fqn, "my_crate::Foo");
    assert_eq!(r.crate_name, "my_crate");
    assert!(r.postgres_id.is_none());
    assert!(r.neo4j_node_id.is_none());
    assert!(r.qdrant_point_id.is_none());
    assert!(logs_contain("StoreReference::new entry"));
}

#[traced_test]
#[test]
fn store_reference_new_log_contains_fqn() {
    let fqn = "some_crate::Bar::method";
    let _ = StoreReference::new(fqn.to_string(), "some_crate".to_string());
    assert!(logs_contain(fqn));
}

#[traced_test]
#[test]
fn store_reference_new_log_contains_crate_name() {
    let _ = StoreReference::new("unique_crate::Baz".to_string(), "unique_crate".to_string());
    assert!(logs_contain("unique_crate"));
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReference::is_fully_synced
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn store_reference_is_fully_synced_true_emits_debug_log() {
    let mut r = StoreReference::new("crate::Item".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    r.neo4j_node_id = Some("neo-1".to_string());
    r.qdrant_point_id = Some("qdrant-1".to_string());
    assert!(r.is_fully_synced());
    assert!(logs_contain("StoreReference::is_fully_synced exit"));
}

#[traced_test]
#[test]
fn store_reference_is_fully_synced_false_when_stores_missing() {
    let mut r = StoreReference::new("crate::Partial".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    // neo4j and qdrant deliberately absent
    assert!(!r.is_fully_synced());
    assert!(logs_contain("StoreReference::is_fully_synced exit"));
}

#[traced_test]
#[test]
fn store_reference_is_fully_synced_false_when_all_absent() {
    let r = StoreReference::new("crate::Empty".to_string(), "crate".to_string());
    assert!(!r.is_fully_synced());
    assert!(logs_contain("StoreReference::is_fully_synced exit"));
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReference::is_orphaned
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn store_reference_is_orphaned_true_emits_warn() {
    let r = StoreReference::new("crate::Orphan".to_string(), "crate".to_string());
    assert!(r.is_orphaned());
    assert!(logs_contain("StoreReference is orphaned"));
}

#[traced_test]
#[test]
fn store_reference_is_orphaned_false_no_warn_when_in_one_store() {
    let mut r = StoreReference::new("crate::Present".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    assert!(!r.is_orphaned());
    assert!(!logs_contain("StoreReference is orphaned"));
}

#[traced_test]
#[test]
fn store_reference_is_orphaned_false_when_fully_synced() {
    let mut r = StoreReference::new("crate::Full".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    r.neo4j_node_id = Some("neo-1".to_string());
    r.qdrant_point_id = Some("q-1".to_string());
    assert!(!r.is_orphaned());
    assert!(!logs_contain("StoreReference is orphaned"));
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReference::missing_stores
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn store_reference_missing_stores_all_absent_emits_warn() {
    let r = StoreReference::new("crate::Missing".to_string(), "crate".to_string());
    let missing = r.missing_stores();
    assert_eq!(missing.len(), 3);
    assert!(missing.contains(&"postgres"));
    assert!(missing.contains(&"neo4j"));
    assert!(missing.contains(&"qdrant"));
    assert!(logs_contain("StoreReference has missing stores"));
}

#[traced_test]
#[test]
fn store_reference_missing_stores_partial_emits_warn() {
    let mut r = StoreReference::new("crate::Partial".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    let missing = r.missing_stores();
    assert_eq!(missing.len(), 2);
    assert!(!missing.contains(&"postgres"));
    assert!(missing.contains(&"neo4j"));
    assert!(missing.contains(&"qdrant"));
    assert!(logs_contain("StoreReference has missing stores"));
}

#[traced_test]
#[test]
fn store_reference_missing_stores_none_missing_no_warn() {
    let mut r = StoreReference::new("crate::Complete".to_string(), "crate".to_string());
    r.postgres_id = Some("pg-1".to_string());
    r.neo4j_node_id = Some("neo-1".to_string());
    r.qdrant_point_id = Some("q-1".to_string());
    let missing = r.missing_stores();
    assert!(missing.is_empty());
    assert!(!logs_contain("StoreReference has missing stores"));
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolutionQuality::from_str
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn resolution_quality_analyzed_emits_debug_on_success() {
    let q: ResolutionQuality = "analyzed".parse().unwrap();
    assert_eq!(q, ResolutionQuality::Analyzed);
    assert!(logs_contain("ResolutionQuality::from_str success"));
}

#[traced_test]
#[test]
fn resolution_quality_heuristic_emits_debug_on_success() {
    let q: ResolutionQuality = "heuristic".parse().unwrap();
    assert_eq!(q, ResolutionQuality::Heuristic);
    assert!(logs_contain("ResolutionQuality::from_str success"));
}

#[traced_test]
#[test]
fn resolution_quality_unknown_variant_emits_debug_on_success() {
    let q: ResolutionQuality = "unknown".parse().unwrap();
    assert_eq!(q, ResolutionQuality::Unknown);
    assert!(logs_contain("ResolutionQuality::from_str success"));
}

#[traced_test]
#[test]
fn resolution_quality_invalid_input_emits_warn() {
    let result = "not_a_quality".parse::<ResolutionQuality>();
    assert!(result.is_err());
    assert!(logs_contain("ResolutionQuality::from_str failed"));
}

#[traced_test]
#[test]
fn resolution_quality_from_str_entry_trace_always_emitted() {
    // The entry trace! is emitted before the match, regardless of outcome.
    let _ = "analyzed".parse::<ResolutionQuality>();
    assert!(logs_contain("ResolutionQuality::from_str entry"));
}

// ─────────────────────────────────────────────────────────────────────────────
// EmbeddingModelConfig::default
// ─────────────────────────────────────────────────────────────────────────────

#[traced_test]
#[test]
fn embedding_config_default_emits_entry_debug_log() {
    let _ = EmbeddingModelConfig::default();
    assert!(logs_contain("EmbeddingModelConfig::default entry"));
}

#[traced_test]
#[test]
fn embedding_config_default_emits_creation_debug_log() {
    let config = EmbeddingModelConfig::default();
    assert_eq!(config.model, "nomic-embed-text");
    assert_eq!(config.dimensions, 768);
    assert_eq!(config.code_collection, "code_embeddings");
    assert_eq!(config.doc_collection, "doc_embeddings");
    assert!(logs_contain("EmbeddingModelConfig default created"));
}

#[traced_test]
#[test]
fn embedding_config_default_log_contains_model_name() {
    let _ = EmbeddingModelConfig::default();
    assert!(logs_contain("nomic-embed-text"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-cutting: logging does not break return values
// ─────────────────────────────────────────────────────────────────────────────

/// Confirm that every logged call still returns the correct value; logging
/// must be side-effect-free with respect to the public API contract.
#[traced_test]
#[test]
fn logging_does_not_alter_return_values() {
    // Visibility
    assert_eq!(Visibility::Public.as_str(), "pub");
    assert_eq!(Visibility::PubCrate.as_str(), "pub_crate");
    assert_eq!(Visibility::PubSuper.as_str(), "pub_super");
    assert_eq!(Visibility::Private.as_str(), "private");
    let v_pub_in = Visibility::PubIn("a::b".to_string());
    assert_eq!(v_pub_in.as_str(), "a::b");

    // ItemType
    assert_eq!(ItemType::Trait.as_str(), "trait");
    let it_unknown = ItemType::Unknown("x".to_string());
    assert_eq!(it_unknown.as_str(), "x");

    // StoreReference
    let mut r = StoreReference::new("ns::T".to_string(), "ns".to_string());
    assert!(!r.is_fully_synced());
    assert!(r.is_orphaned());
    r.postgres_id = Some("pg".to_string());
    r.neo4j_node_id = Some("neo".to_string());
    r.qdrant_point_id = Some("q".to_string());
    assert!(r.is_fully_synced());
    assert!(!r.is_orphaned());
    assert!(r.missing_stores().is_empty());

    // ResolutionQuality round-trip
    assert_eq!("analyzed".parse::<ResolutionQuality>().unwrap(), ResolutionQuality::Analyzed);
    assert!("invalid".parse::<ResolutionQuality>().is_err());

    // EmbeddingModelConfig defaults
    let cfg = EmbeddingModelConfig::default();
    assert_eq!(cfg.dimensions, 768);
}
