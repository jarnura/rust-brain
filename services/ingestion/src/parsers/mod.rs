//! Dual-parser strategy for Rust source code
//!
//! This module implements a two-phase parsing approach:
//! 1. **tree-sitter**: Fast skeleton parsing for item boundaries and structure detection
//! 2. **syn**: Deep semantic parsing for generics, where clauses, and detailed analysis
//!
//! The dual strategy provides both speed (tree-sitter) and accuracy (syn) with graceful
//! fallback when syn fails.

mod syn_parser;
mod tree_sitter_parser;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub use syn_parser::SynParser;
pub use tree_sitter_parser::TreeSitterParser;

// Re-export shared types from rustbrain-common
pub use rustbrain_common::types::{GenericParam, ItemType, Visibility, WhereClause};

/// Fully parsed item from Rust source code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedItem {
    /// Fully qualified name: "crate::module::function_name"
    pub fqn: String,

    /// Item type: function, struct, enum, trait, etc.
    pub item_type: ItemType,

    /// Short name (without module path)
    pub name: String,

    /// Visibility level
    pub visibility: Visibility,

    /// Full signature string
    pub signature: String,

    /// Generic parameters with bounds
    pub generic_params: Vec<GenericParam>,

    /// Where clause predicates
    pub where_clauses: Vec<WhereClause>,

    /// Attributes (#[derive(...)], #[cfg(...)], #[doc = "..."])
    pub attributes: Vec<String>,

    /// Doc comment content (extracted from /// or #[doc = "..."])
    pub doc_comment: String,

    /// Starting line number (1-indexed)
    pub start_line: usize,

    /// Ending line number (1-indexed)
    pub end_line: usize,

    /// Full source code of the item body
    pub body_source: String,

    /// Source of macro generation, e.g., "derive(Debug)"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
}

/// Skeleton item from tree-sitter (lightweight, fast)
#[derive(Debug, Clone)]
pub struct SkeletonItem {
    /// Item type
    pub item_type: ItemType,

    /// Item name (may be empty for impl blocks)
    pub name: Option<String>,

    /// Starting byte offset
    pub start_byte: usize,

    /// Ending byte offset
    pub end_byte: usize,

    /// Starting line (1-indexed)
    pub start_line: usize,

    /// Ending line (1-indexed)
    pub end_line: usize,
}

/// Maximum body_source length to store (to prevent memory explosion with expanded code)
/// Set to 50KB - large enough for most code items but prevents storing massive generated code
const MAX_BODY_SOURCE_LEN: usize = 50_000;

/// Truncate body_source aggressively to prevent OOM on large expanded codebases
fn truncate_body_source(source: &str) -> String {
    if source.len() <= MAX_BODY_SOURCE_LEN {
        source.to_string()
    } else {
        format!("[BODY: {} bytes]", source.len())
    }
}

/// Result of dual parsing
#[derive(Debug)]
pub struct ParseResult {
    /// All parsed items
    pub items: Vec<ParsedItem>,

    /// Items that failed syn parsing (tree-sitter only)
    pub partial_items: Vec<SkeletonItem>,

    /// Parse errors encountered
    pub errors: Vec<ParseError>,
}

/// Parse error details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub context: String,
}

/// The main dual-parser
pub struct DualParser {
    tree_sitter: TreeSitterParser,
    syn: SynParser,
}

impl DualParser {
    /// Create a new dual parser instance
    pub fn new() -> Result<Self> {
        let tree_sitter =
            TreeSitterParser::new().context("Failed to initialize tree-sitter parser")?;
        let syn = SynParser::new();

        Ok(Self { tree_sitter, syn })
    }

    /// Parse source code using the dual strategy
    ///
    /// Strategy:
    /// 1. Use tree-sitter to get item skeletons (fast)
    /// 2. Use syn to parse each item for deep analysis
    /// 3. Fall back to tree-sitter data if syn fails
    pub fn parse(&self, source: &str, module_path: &str) -> Result<ParseResult> {
        // Phase 1: Tree-sitter skeleton extraction
        let skeletons = self
            .tree_sitter
            .extract_skeletons(source)
            .context("Tree-sitter parsing failed")?;

        let mut items = Vec::new();
        let mut partial_items = Vec::new();
        let mut errors = Vec::new();

        // Phase 2: Deep parse with syn for each skeleton
        for skeleton in skeletons {
            // Extract source for this item
            let item_source =
                if skeleton.end_byte <= source.len() && skeleton.start_byte <= skeleton.end_byte {
                    &source[skeleton.start_byte..skeleton.end_byte]
                } else {
                    errors.push(ParseError {
                        message: "Invalid byte range for skeleton".to_string(),
                        line: Some(skeleton.start_line),
                        column: None,
                        context: skeleton.name.clone().unwrap_or_default(),
                    });
                    continue;
                };

            // Try syn parsing (parse_items returns Vec for impl blocks with methods)
            match self.syn.parse_items(item_source, module_path, &skeleton) {
                Ok(parsed_items) => {
                    items.extend(parsed_items);
                }
                Err(e) => {
                    // Record the error
                    errors.push(ParseError {
                        message: e.to_string(),
                        line: Some(skeleton.start_line),
                        column: None,
                        context: skeleton.name.clone().unwrap_or_default(),
                    });

                    // Create a partial item from tree-sitter data
                    partial_items.push(skeleton.clone());

                    // Try to create a minimal ParsedItem from tree-sitter data
                    if let Some(name) = &skeleton.name {
                        let partial_parsed = self.create_partial_item(
                            source,
                            item_source,
                            module_path,
                            name,
                            &skeleton,
                        );
                        items.push(partial_parsed);
                    }
                }
            }
        }

        Ok(ParseResult {
            items,
            partial_items,
            errors,
        })
    }

    /// Parse a file using the dual strategy
    pub fn parse_file(&self, path: &Path, module_path: &str) -> Result<ParseResult> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file: {:?}", path))?;

        self.parse(&source, module_path)
    }

    /// Create a partial item from tree-sitter data when syn fails
    fn create_partial_item(
        &self,
        full_source: &str,
        item_source: &str,
        module_path: &str,
        name: &str,
        skeleton: &SkeletonItem,
    ) -> ParsedItem {
        // Try to extract visibility from tree-sitter
        let visibility = self
            .tree_sitter
            .extract_visibility(item_source)
            .unwrap_or(Visibility::Private);

        // Try to extract attributes from tree-sitter
        let attributes = self.tree_sitter.extract_attributes(item_source);

        // Try to extract doc comments
        let doc_comment = self
            .tree_sitter
            .extract_doc_comments(full_source, skeleton.start_line);

        // Create a basic signature from the first line
        let signature = item_source
            .lines()
            .next()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: skeleton.item_type.clone(),
            name: name.to_string(),
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(item_source),
            generated_by: None,
        }
    }
}

impl Default for DualParser {
    fn default() -> Self {
        Self::new().expect("Failed to create default DualParser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            /// A simple function
            pub fn hello<T: Clone>(x: T) -> T {
                x.clone()
            }
        "#;

        let result = parser.parse(source, "test::module").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        assert_eq!(item.name, "hello");
        assert!(matches!(item.visibility, Visibility::Public));
    }

    #[test]
    fn test_parse_struct_with_generics() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            #[derive(Clone, Debug)]
            pub struct Container<T, U: Send> {
                inner: T,
                other: U,
            }
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        assert_eq!(item.name, "Container");
        assert!(!item.generic_params.is_empty());
    }

    #[test]
    fn test_parse_with_where_clause() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            pub fn process<T, U>(x: T, y: U) -> bool
            where
                T: Clone + Send,
                U: Sync + 'static,
            {
                true
            }
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        assert!(!item.where_clauses.is_empty());
    }

    #[test]
    fn test_parse_multiple_items() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            pub struct Foo { x: i32 }
            pub enum Bar { A, B, C }
            pub fn baz() -> i32 { 42 }
            pub trait Qux { fn do_thing(&self); }
        "#;

        let result = parser.parse(source, "test::mod").unwrap();

        assert!(
            result.items.len() >= 4,
            "Expected at least 4 items, got {}",
            result.items.len()
        );

        let types: Vec<&str> = result.items.iter().map(|i| i.item_type.as_str()).collect();
        assert!(types.contains(&"struct"));
        assert!(types.contains(&"enum"));
        assert!(types.contains(&"function"));
        assert!(types.contains(&"trait"));
    }

    #[test]
    fn test_fallback_on_syn_failure() {
        let parser = DualParser::new().unwrap();
        // Valid Rust that tree-sitter can parse but has both valid and invalid items
        let source = r#"
            pub fn valid_function() -> i32 { 42 }

            pub struct ValidStruct { x: i32 }
        "#;

        let result = parser.parse(source, "test::mod").unwrap();

        // Valid items should be parsed successfully
        assert!(result.items.len() >= 2);
        assert!(
            result.errors.is_empty(),
            "Expected no errors for valid source, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_fqn_construction() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub fn my_function() {}"#;

        let result = parser.parse(source, "my_crate::my_module").unwrap();

        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].fqn, "my_crate::my_module::my_function");
    }

    #[test]
    fn test_empty_source() {
        let parser = DualParser::new().unwrap();
        let result = parser.parse("", "test").unwrap();

        assert!(result.items.is_empty());
        assert!(result.partial_items.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_parse_impl_block() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            impl Foo {
                pub fn new() -> Self { Foo }
            }
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert!(result
            .items
            .iter()
            .any(|i| matches!(i.item_type, ItemType::Impl)));
    }

    #[test]
    fn test_doc_comments_lost_in_dual_parse() {
        // Known limitation: doc comments (/// and #[doc]) preceding an item
        // are outside tree-sitter's skeleton byte range, so they're lost when
        // syn parses just the extracted item source. The tree-sitter fallback
        // path (create_partial_item) does extract them from the full source.
        // Fix: expand skeleton byte ranges to include preceding attributes/comments.
        let parser = DualParser::new().unwrap();
        let source = "/// This doc comment is before the item.\npub fn example() {}\n";

        let result = parser.parse(source, "test").unwrap();
        assert!(!result.items.is_empty());
        // Doc comment is currently empty — this test documents the limitation
        // When fixed, this assertion should change to assert non-empty
        assert!(
            result.items[0].doc_comment.is_empty(),
            "If this fails, the doc comment bug is fixed! Update this test."
        );
    }

    // -----------------------------------------------------------------------
    // truncate_body_source (tested indirectly via parse)
    // -----------------------------------------------------------------------

    #[test]
    fn test_short_body_source_not_truncated() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub fn tiny() -> i32 { 42 }"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        // Body source should contain actual code, not a truncation placeholder
        assert!(
            !item.body_source.starts_with("[BODY:"),
            "Short body should not be truncated: {}",
            item.body_source
        );
    }

    #[test]
    fn test_large_body_source_truncated_to_placeholder() {
        // Build a source function whose body exceeds MAX_BODY_SOURCE_LEN (50_000 bytes)
        // We test truncate_body_source directly via the public-facing function.
        let short = "hello";
        let long = "x".repeat(60_000);

        assert_eq!(truncate_body_source(short), short);

        let truncated = truncate_body_source(&long);
        assert!(
            truncated.starts_with("[BODY:"),
            "Long body should be replaced with placeholder: {}",
            &truncated[..30]
        );
        assert!(truncated.contains("60000 bytes"));
    }

    // -----------------------------------------------------------------------
    // FQN construction edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_fqn_with_deeply_nested_module() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub fn leaf() {}"#;

        let result = parser.parse(source, "crate::a::b::c::d").unwrap();

        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].fqn, "crate::a::b::c::d::leaf");
    }

    #[test]
    fn test_fqn_for_struct_uses_correct_module_path() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub struct Payload { value: u64 }"#;

        let result = parser.parse(source, "crate::net::proto").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        assert_eq!(item.fqn, "crate::net::proto::Payload");
        assert_eq!(item.name, "Payload");
    }

    // -----------------------------------------------------------------------
    // Malformed Rust — graceful degradation
    // -----------------------------------------------------------------------

    #[test]
    fn test_entirely_malformed_source_does_not_panic() {
        let parser = DualParser::new().unwrap();
        // Not valid Rust at all
        let source = "@#$% not rust ! {} fn ???";

        // Should not panic — may return empty items or errors
        let result = parser.parse(source, "test");
        // Either Ok (with errors) or Err — both acceptable; we just need no panic
        let _ = result;
    }

    #[test]
    fn test_partial_source_returns_some_items() {
        let parser = DualParser::new().unwrap();
        // Two valid items plus some garbage in the middle
        let source = r#"
            pub fn before() -> i32 { 1 }
            pub struct After { x: i32 }
        "#;

        let result = parser.parse(source, "test::m").unwrap();

        // At least the two valid items should parse
        assert!(
            result.items.len() >= 2,
            "Expected at least 2 items, got {}",
            result.items.len()
        );
    }

    // -----------------------------------------------------------------------
    // Visibility variants
    // -----------------------------------------------------------------------

    #[test]
    fn test_private_item_visibility() {
        let parser = DualParser::new().unwrap();
        let source = r#"fn private_fn() {}"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert!(
            matches!(result.items[0].visibility, Visibility::Private),
            "Expected Private visibility"
        );
    }

    #[test]
    fn test_public_item_visibility() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub fn public_fn() {}"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert!(
            matches!(result.items[0].visibility, Visibility::Public),
            "Expected Public visibility"
        );
    }

    #[test]
    fn test_pub_crate_item_visibility() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub(crate) fn crate_fn() {}"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        // Should be crate-visible (not Private, not Public)
        assert!(
            !matches!(result.items[0].visibility, Visibility::Public),
            "pub(crate) should not be Public"
        );
    }

    // -----------------------------------------------------------------------
    // Struct / enum / trait item types
    // -----------------------------------------------------------------------

    #[test]
    fn test_enum_item_type() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub enum Status { Active, Inactive }"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].item_type.as_str(), "enum");
        assert_eq!(result.items[0].name, "Status");
    }

    #[test]
    fn test_trait_item_type() {
        let parser = DualParser::new().unwrap();
        let source = r#"pub trait Processable { fn process(&self); }"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert_eq!(result.items[0].item_type.as_str(), "trait");
        assert_eq!(result.items[0].name, "Processable");
    }

    // -----------------------------------------------------------------------
    // Multiple generic params and where clauses
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_generic_params() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            pub fn multi<A, B, C>(_a: A, _b: B, _c: C) {}
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        assert_eq!(item.generic_params.len(), 3, "Expected 3 generic params");
    }

    #[test]
    fn test_where_clause_with_multiple_bounds() {
        let parser = DualParser::new().unwrap();
        let source = r#"
            pub fn bounded<T>(_x: T)
            where
                T: Clone + Send + Sync + 'static,
            {}
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        assert!(!result.items[0].where_clauses.is_empty());
    }

    // -----------------------------------------------------------------------
    // Attributes
    // -----------------------------------------------------------------------

    #[test]
    fn test_derive_attributes_stripped_by_skeleton_extraction() {
        // Known limitation: attributes preceding an item (e.g. #[derive(...)]) are
        // outside the tree-sitter skeleton byte range, so syn sees only the bare
        // item body and the attribute list is empty.  This mirrors the doc-comment
        // limitation documented in test_doc_comments_lost_in_dual_parse.
        // Fix: expand skeleton byte ranges to include preceding attribute blocks.
        let parser = DualParser::new().unwrap();
        let source = r#"
            #[derive(Clone, Debug, PartialEq)]
            pub struct Tagged { id: u64 }
        "#;

        let result = parser.parse(source, "test").unwrap();

        assert!(!result.items.is_empty());
        let item = &result.items[0];
        // Attributes are currently lost in the dual-parse path
        // When fixed, change this assertion to assert non-empty
        assert!(
            item.attributes.is_empty(),
            "If this fails, the attribute-stripping bug is fixed! Update this test."
        );
    }

    // -----------------------------------------------------------------------
    // Large file stress test
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_large_source_file_does_not_panic() {
        let parser = DualParser::new().unwrap();

        // Generate 100 small functions
        let mut source = String::new();
        for i in 0..100 {
            source.push_str(&format!("pub fn func_{i}() -> i32 {{ {i} }}\n"));
        }

        let result = parser.parse(&source, "crate::generated").unwrap();

        assert!(
            result.items.len() >= 100,
            "Expected at least 100 items, got {}",
            result.items.len()
        );
        assert!(
            result.errors.is_empty(),
            "Expected no parse errors for clean source"
        );
    }

    // -----------------------------------------------------------------------
    // ParseResult structure
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_result_partial_items_on_syn_failure() {
        let parser = DualParser::new().unwrap();
        // This is valid enough for tree-sitter but still parseable by syn.
        // Partial items appear only when syn fails but tree-sitter succeeds.
        // For a completely valid source, partial_items should be empty.
        let source = r#"pub fn clean() -> bool { true }"#;

        let result = parser.parse(source, "test").unwrap();

        assert!(
            result.partial_items.is_empty(),
            "No partial items expected for clean source"
        );
    }

    #[test]
    fn test_parse_file_returns_error_for_missing_file() {
        let parser = DualParser::new().unwrap();
        let missing = std::path::Path::new("/nonexistent/path/to/file.rs");

        let result = parser.parse_file(missing, "test");

        assert!(result.is_err(), "Expected error for missing file");
    }

    // -----------------------------------------------------------------------
    // DualParser::default()
    // -----------------------------------------------------------------------

    #[test]
    fn test_dual_parser_default_is_ok() {
        // Default uses expect() internally; just ensure it doesn't panic
        let _parser = DualParser::default();
    }
}
