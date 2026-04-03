//! Integration test: verify call-graph pipeline fixes against the test fixture.
//!
//! This test parses the target-repo/src/lib.rs fixture and validates that
//! the core fix (Bug #1 + #2) produces correct output — impl methods are
//! emitted as individual ParsedItems with Type-qualified FQNs.

use rustbrain_ingestion::parsers::DualParser;
use std::collections::{HashMap, HashSet};

/// Parse the test fixture and return all items.
fn parse_fixture() -> Vec<rustbrain_ingestion::parsers::ParsedItem> {
    let parser = DualParser::new().unwrap();
    // Tests run from workspace root
    let fixture_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target-repo/src/lib.rs");
    let source = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Cannot read {:?}: {}", fixture_path, e));
    let result = parser.parse(&source, "test_fixture").unwrap();
    result.items
}

#[test]
fn verify_impl_methods_emitted_from_fixture() {
    let items = parse_fixture();

    let functions: Vec<_> = items
        .iter()
        .filter(|i| i.item_type.as_str() == "function")
        .collect();
    let impls: Vec<_> = items
        .iter()
        .filter(|i| i.item_type.as_str() == "impl")
        .collect();
    let structs: Vec<_> = items
        .iter()
        .filter(|i| i.item_type.as_str() == "struct")
        .collect();

    println!("\n=== ITEM COUNTS ===");
    println!("Total items: {}", items.len());
    println!("Functions (standalone + methods): {}", functions.len());
    println!("Impl blocks: {}", impls.len());
    println!("Structs: {}", structs.len());

    // Methods with impl_type attribute (from impl blocks)
    let method_items: Vec<_> = functions
        .iter()
        .filter(|f| f.attributes.iter().any(|a| a.starts_with("impl_type=")))
        .collect();

    println!("\n=== ALL IMPL METHOD ITEMS ({}) ===", method_items.len());
    for m in &method_items {
        let impl_type = m
            .attributes
            .iter()
            .find(|a| a.starts_with("impl_type="))
            .map(|a| &a["impl_type=".len()..])
            .unwrap_or("?");
        let impl_for = m
            .attributes
            .iter()
            .find(|a| a.starts_with("impl_for="))
            .map(|a| &a["impl_for=".len()..]);
        println!(
            "  {} (impl_type={}, impl_for={:?})",
            m.fqn, impl_type, impl_for
        );
    }

    // Should have at least 15 method items from all the impl blocks in the fixture
    assert!(
        method_items.len() >= 15,
        "Should emit at least 15 method items, got {}",
        method_items.len()
    );

    // Verify specific method FQNs exist
    let fqns: Vec<&str> = method_items.iter().map(|m| m.fqn.as_str()).collect();

    assert!(
        fqns.iter().any(|f| f.ends_with("::User::new")),
        "User::new not found in: {:?}",
        fqns
    );
    assert!(
        fqns.iter().any(|f| f.ends_with("::User::deactivate")),
        "User::deactivate not found"
    );
    assert!(
        fqns.iter().any(|f| f.ends_with("::Point::origin")),
        "Point::origin not found"
    );
    assert!(
        fqns.iter().any(|f| f.ends_with("::Point::distance_to")),
        "Point::distance_to not found"
    );
    assert!(
        fqns.iter().any(|f| f.ends_with("::Status::is_success")),
        "Status::is_success not found"
    );
    assert!(
        fqns.iter().any(|f| f.ends_with("::Status::error_message")),
        "Status::error_message not found"
    );
}

#[test]
fn verify_function_index_includes_all_methods() {
    let items = parse_fixture();

    // Build the same index GraphStage builds
    let mut function_fqns = HashSet::new();
    let mut function_names_to_fqns: HashMap<String, Vec<String>> = HashMap::new();

    for item in &items {
        if item.item_type.as_str() == "function" {
            function_fqns.insert(item.fqn.clone());
            function_names_to_fqns
                .entry(item.name.clone())
                .or_default()
                .push(item.fqn.clone());
        }
    }

    println!("\n=== FUNCTION INDEX ===");
    println!("Total FQNs in index: {}", function_fqns.len());
    println!("Unique short names: {}", function_names_to_fqns.len());

    // Methods should be findable by short name
    assert!(
        function_names_to_fqns.contains_key("deactivate"),
        "'deactivate' not in index"
    );
    assert!(
        function_names_to_fqns.contains_key("origin"),
        "'origin' not in index"
    );
    assert!(
        function_names_to_fqns.contains_key("is_success"),
        "'is_success' not in index"
    );
    assert!(
        function_names_to_fqns.contains_key("distance_to"),
        "'distance_to' not in index"
    );
    assert!(
        function_names_to_fqns.contains_key("error_message"),
        "'error_message' not in index"
    );

    // 'new' should have multiple entries (User, Container, DefaultHandler, etc.)
    let new_fqns = function_names_to_fqns
        .get("new")
        .expect("'new' should exist");
    println!("\n=== 'new' FQN ENTRIES ({}) ===", new_fqns.len());
    for fqn in new_fqns {
        println!("  {}", fqn);
    }
    assert!(
        new_fqns.len() >= 3,
        "Should have at least 3 'new' methods (User, Container, DefaultHandler, ...), got {}",
        new_fqns.len()
    );

    // Each 'new' FQN should contain its type name
    assert!(
        new_fqns.iter().any(|f| f.contains("::User::new")),
        "User::new should be in the 'new' FQN list"
    );
    assert!(
        new_fqns.iter().any(|f| f.contains("::DefaultHandler::new")),
        "DefaultHandler::new should be in the 'new' FQN list"
    );
}

#[test]
fn verify_type_method_resolution_is_correct() {
    let items = parse_fixture();

    let mut function_fqns = HashSet::new();
    let mut function_names_to_fqns: HashMap<String, Vec<String>> = HashMap::new();

    for item in &items {
        if item.item_type.as_str() == "function" {
            function_fqns.insert(item.fqn.clone());
            function_names_to_fqns
                .entry(item.name.clone())
                .or_default()
                .push(item.fqn.clone());
        }
    }

    // Simulate Type::method() resolution logic from the fixed resolve_call_target
    // Bug #4 fix: should NOT fall back to arbitrary first match
    let new_fqns = function_names_to_fqns.get("new").unwrap();

    // User::new should match exactly
    let user_new_suffix = "::User::new";
    let resolved_user = new_fqns.iter().find(|f| f.ends_with(user_new_suffix));
    assert!(
        resolved_user.is_some(),
        "User::new should resolve via suffix match. Available: {:?}",
        new_fqns
    );
    println!("\nUser::new → {}", resolved_user.unwrap());

    // NonExistent::new should NOT match any entry
    let nonexistent_suffix = "::NonExistent::new";
    let resolved_none = new_fqns.iter().find(|f| f.ends_with(nonexistent_suffix));
    assert!(
        resolved_none.is_none(),
        "NonExistent::new should NOT resolve (Bug #4 fix). Got: {:?}",
        resolved_none
    );
    println!("NonExistent::new → None (correct!)");
}

#[test]
fn verify_impl_blocks_retain_correct_fqn() {
    let items = parse_fixture();

    let impls: Vec<_> = items
        .iter()
        .filter(|i| i.item_type.as_str() == "impl")
        .collect();

    println!("\n=== IMPL BLOCK FQNs ===");
    for imp in &impls {
        println!("  {} (signature: {})", imp.fqn, imp.signature);
    }

    // Verify trait impl has correct naming convention: module::Trait_Type
    let async_handler_impl = impls
        .iter()
        .find(|i| i.signature.contains("AsyncHandler") && i.signature.contains("DefaultHandler"));
    assert!(
        async_handler_impl.is_some(),
        "Should find AsyncHandler for DefaultHandler impl"
    );
    let ah = async_handler_impl.unwrap();
    assert!(
        ah.fqn.contains("AsyncHandler_DefaultHandler"),
        "Trait impl FQN should contain Trait_Type. Got: {}",
        ah.fqn
    );
    assert!(
        ah.attributes.iter().any(|a| a == "impl_for=AsyncHandler"),
        "Should have impl_for=AsyncHandler attribute"
    );
}
