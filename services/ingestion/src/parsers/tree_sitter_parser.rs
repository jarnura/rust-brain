//! Tree-sitter based fast skeleton parser
//!
//! Provides quick parsing for:
//! - Item boundary detection (start/end lines)
//! - Item type identification
//! - Name extraction
//! - Basic structure analysis

use crate::parsers::{ItemType, SkeletonItem, Visibility};
use anyhow::{Context, Result};
use std::sync::Mutex;
use tree_sitter::{Node, Parser, TreeCursor};

/// Tree-sitter based parser for fast skeleton extraction
pub struct TreeSitterParser {
    parser: Mutex<Parser>,
}

impl TreeSitterParser {
    /// Create a new tree-sitter parser with Rust grammar
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .context("Failed to set tree-sitter Rust language")?;

        Ok(Self {
            parser: Mutex::new(parser),
        })
    }

    /// Extract item skeletons from source code
    pub fn extract_skeletons(&self, source: &str) -> Result<Vec<SkeletonItem>> {
        let source_bytes = source.as_bytes();
        let tree = self
            .parser
            .lock()
            .unwrap()
            .parse(source, None)
            .context("Failed to parse source with tree-sitter")?;

        let root = tree.root_node();
        let mut skeletons = Vec::new();
        let mut cursor = root.walk();

        self.collect_items(&mut cursor, source_bytes, &mut skeletons);

        // Sort by start position
        skeletons.sort_by_key(|s| s.start_byte);

        Ok(skeletons)
    }

    /// Recursively collect items from the AST
    fn collect_items<'a>(
        &self,
        cursor: &mut TreeCursor<'a>,
        source: &'a [u8],
        skeletons: &mut Vec<SkeletonItem>,
    ) {
        loop {
            let node = cursor.node();
            let kind = node.kind();

            // Check if this is a top-level item
            if let Some(skeleton) = self.try_extract_skeleton(node, source) {
                skeletons.push(skeleton);

                // For items that might contain nested items (impl blocks, modules, traits)
                // we need to recurse
                if matches!(kind, "impl_item" | "trait_item" | "mod_item")
                    && cursor.goto_first_child()
                {
                    self.collect_items(cursor, source, skeletons);
                    cursor.goto_parent();
                }
            } else if cursor.goto_first_child() {
                self.collect_items(cursor, source, skeletons);
                cursor.goto_parent();
            }

            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }

    /// Try to extract a skeleton from a node
    fn try_extract_skeleton<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Option<SkeletonItem> {
        let kind = node.kind();
        let item_type = self.kind_to_item_type(kind)?;

        let name = self.extract_name(node, source, &item_type);

        Some(SkeletonItem {
            item_type,
            name,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_line: node.start_position().row + 1, // 1-indexed
            end_line: node.end_position().row + 1,
        })
    }

    /// Convert tree-sitter kind to ItemType
    fn kind_to_item_type(&self, kind: &str) -> Option<ItemType> {
        match kind {
            "function_item" | "function_signature_item" => Some(ItemType::Function),
            "struct_item" => Some(ItemType::Struct),
            "enum_item" => Some(ItemType::Enum),
            "trait_item" => Some(ItemType::Trait),
            "impl_item" => Some(ItemType::Impl),
            "type_item" => Some(ItemType::TypeAlias),
            "const_item" => Some(ItemType::Const),
            "static_item" => Some(ItemType::Static),
            "macro_invocation" | "macro_definition" => Some(ItemType::Macro),
            "mod_item" => Some(ItemType::Module),
            "use_declaration" => Some(ItemType::Use),
            "foreign_mod_item" | "extern_crate_declaration" => Some(ItemType::ExternBlock),
            _ => None,
        }
    }

    /// Extract name from an item node
    fn extract_name<'a>(
        &self,
        node: Node<'a>,
        source: &'a [u8],
        item_type: &ItemType,
    ) -> Option<String> {
        match item_type {
            ItemType::Function => {
                // Look for function name (identifier node)
                self.find_child_by_kind(node, "identifier")
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(|s| s.to_string())
            }
            ItemType::Struct | ItemType::Enum | ItemType::Trait | ItemType::TypeAlias => {
                // Look for type_identifier
                self.find_child_by_kind(node, "type_identifier")
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(|s| s.to_string())
            }
            ItemType::Impl => {
                // For impl blocks, extract the type being implemented
                self.extract_impl_name(node, source)
            }
            ItemType::Const | ItemType::Static => self
                .find_child_by_kind(node, "identifier")
                .and_then(|n| n.utf8_text(source).ok())
                .map(|s| s.to_string()),
            ItemType::Macro => {
                // Macro name can be identifier or scoped_identifier
                self.find_child_by_kind(node, "identifier")
                    .or_else(|| self.find_child_by_kind(node, "scoped_identifier"))
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(|s| s.to_string())
            }
            ItemType::Module => self
                .find_child_by_kind(node, "identifier")
                .and_then(|n| n.utf8_text(source).ok())
                .map(|s| s.to_string()),
            ItemType::Use => {
                // Use declarations don't have a simple name
                None
            }
            _ => None,
        }
    }

    /// Extract impl block name (trait name or self type)
    fn extract_impl_name<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Option<String> {
        // Look for trait name (if impl Trait for Type)
        if let Some(trait_type) = self.find_child_by_field(node, "trait") {
            return trait_type.utf8_text(source).ok().map(|s| s.to_string());
        }

        // Look for self type
        if let Some(self_type) = self.find_child_by_field(node, "type") {
            return self_type.utf8_text(source).ok().map(|s| s.to_string());
        }

        None
    }

    /// Find a child node by kind
    fn find_child_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        let mut cursor = node.walk();
        cursor.goto_first_child();

        loop {
            if cursor.node().kind() == kind {
                return Some(cursor.node());
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }

        None
    }

    /// Find a child node by field name
    fn find_child_by_field<'a>(&self, node: Node<'a>, field: &str) -> Option<Node<'a>> {
        node.child_by_field_name(field)
    }

    /// Extract visibility from item source
    pub fn extract_visibility(&self, source: &str) -> Option<Visibility> {
        let source_bytes = source.as_bytes();
        let tree = self.parser.lock().unwrap().parse(source, None)?;
        let root = tree.root_node();

        // First try direct children of root (unlikely)
        if let Some(vis) = self.find_visibility(root, source_bytes) {
            return Some(vis);
        }

        // Then try children of the first item node (e.g., function_item, struct_item)
        let mut cursor = root.walk();
        if cursor.goto_first_child() {
            if let Some(vis) = self.find_visibility(cursor.node(), source_bytes) {
                return Some(vis);
            }
        }

        None
    }

    /// Find visibility modifier in a node
    fn find_visibility<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Option<Visibility> {
        let mut cursor = node.walk();

        if cursor.goto_first_child() {
            loop {
                let child = cursor.node();
                if child.kind() == "visibility_modifier" {
                    let text = child.utf8_text(source).ok()?;
                    return Some(self.parse_visibility_text(text));
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }

        None
    }

    /// Parse visibility text into Visibility enum
    fn parse_visibility_text(&self, text: &str) -> Visibility {
        if text == "pub" {
            Visibility::Public
        } else if text == "pub(crate)" {
            Visibility::PubCrate
        } else if text == "pub(super)" {
            Visibility::PubSuper
        } else if text.starts_with("pub(in ") {
            // Extract the path from pub(in path)
            let path = text.trim_start_matches("pub(in ").trim_end_matches(')');
            Visibility::PubIn(path.to_string())
        } else {
            Visibility::Private
        }
    }

    /// Extract attributes from item source
    pub fn extract_attributes(&self, source: &str) -> Vec<String> {
        let source_bytes = source.as_bytes();
        let tree = match self.parser.lock().unwrap().parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let root = tree.root_node();
        self.find_attributes(root, source_bytes)
    }

    /// Find attributes in a node
    fn find_attributes<'a>(&self, node: Node<'a>, source: &'a [u8]) -> Vec<String> {
        let mut attributes = Vec::new();
        let mut cursor = node.walk();

        if cursor.goto_first_child() {
            loop {
                let child = cursor.node();
                if child.kind() == "attribute_item" || child.kind() == "inner_attribute_item" {
                    if let Ok(text) = child.utf8_text(source) {
                        attributes.push(text.to_string());
                    }
                } else if !child.kind().starts_with("attribute") && !child.kind().starts_with("#") {
                    // Stop looking after we've passed the attribute section
                    break;
                }
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }

        attributes
    }

    /// Extract doc comments from source around a line
    pub fn extract_doc_comments(&self, source: &str, item_start_line: usize) -> String {
        let lines: Vec<&str> = source.lines().collect();
        let mut doc_lines = Vec::new();

        // Look backwards from item start for doc comments
        if item_start_line > 1 {
            for i in (0..item_start_line.saturating_sub(1)).rev() {
                let line = lines.get(i).map(|s| s.trim()).unwrap_or("");

                if line.starts_with("///") || line.starts_with("//!") {
                    let doc_content = line
                        .trim_start_matches("///")
                        .trim_start_matches("//!")
                        .trim();
                    doc_lines.insert(0, doc_content.to_string());
                } else if line.starts_with("#[doc") || line.starts_with("#![doc") {
                    // Handle #[doc = "..."] attributes
                    doc_lines.insert(0, self.extract_doc_from_attr(line));
                } else if !line.is_empty() && !line.starts_with("//") && !line.starts_with("#") {
                    // Stop at non-comment content
                    break;
                }
            }
        }

        doc_lines.join("\n")
    }

    /// Extract doc content from #[doc = "..."] attribute
    fn extract_doc_from_attr(&self, attr: &str) -> String {
        // Simple extraction: find content between quotes
        if let Some(start) = attr.find('\"') {
            if let Some(end) = attr.rfind('\"') {
                if start < end {
                    return attr[start + 1..end].to_string();
                }
            }
        }
        String::new()
    }

    /// Get the byte range for an item at a specific line
    pub fn get_item_at_line(&self, source: &str, line: usize) -> Option<(usize, usize)> {
        let skeletons = self.extract_skeletons(source).ok()?;

        for skeleton in skeletons {
            if skeleton.start_line <= line && skeleton.end_line >= line {
                return Some((skeleton.start_byte, skeleton.end_byte));
            }
        }

        None
    }
}

impl Default for TreeSitterParser {
    fn default() -> Self {
        Self::new().expect("Failed to create default TreeSitterParser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let parser = TreeSitterParser::new().unwrap();
        let source = r#"
pub fn hello() -> &'static str {
    "Hello, world!"
}
"#;

        let skeletons = parser.extract_skeletons(source).unwrap();

        assert_eq!(skeletons.len(), 1);
        assert!(matches!(skeletons[0].item_type, ItemType::Function));
        assert_eq!(skeletons[0].name, Some("hello".to_string()));
    }

    #[test]
    fn test_parse_multiple_items() {
        let parser = TreeSitterParser::new().unwrap();
        let source = r#"
pub struct User {
    name: String,
}

pub enum Status {
    Active,
    Inactive,
}

pub fn get_status(user: &User) -> Status {
    Status::Active
}
"#;

        let skeletons = parser.extract_skeletons(source).unwrap();

        assert_eq!(skeletons.len(), 3);
        assert!(matches!(skeletons[0].item_type, ItemType::Struct));
        assert!(matches!(skeletons[1].item_type, ItemType::Enum));
        assert!(matches!(skeletons[2].item_type, ItemType::Function));
    }

    #[test]
    fn test_extract_visibility() {
        let parser = TreeSitterParser::new().unwrap();

        assert!(matches!(
            parser.extract_visibility("pub fn test() {}"),
            Some(Visibility::Public)
        ));

        assert!(matches!(
            parser.extract_visibility("pub(crate) fn test() {}"),
            Some(Visibility::PubCrate)
        ));

        assert!(parser.extract_visibility("fn test() {}").is_none());
    }

    #[test]
    fn test_extract_attributes() {
        let parser = TreeSitterParser::new().unwrap();
        let source = r#"
#[derive(Clone, Debug)]
#[cfg(feature = "test")]
pub struct Point {
    x: i32,
    y: i32,
}
"#;

        let attrs = parser.extract_attributes(source);
        assert_eq!(attrs.len(), 2);
        assert!(attrs[0].contains("derive"));
        assert!(attrs[1].contains("cfg"));
    }

    #[test]
    fn test_extract_doc_comments() {
        let parser = TreeSitterParser::new().unwrap();
        let source = r#"
/// This is a function
/// that does something.
pub fn do_thing() {}
"#;

        let doc = parser.extract_doc_comments(source, 4);
        assert!(doc.contains("This is a function"));
        assert!(doc.contains("that does something"));
    }

    #[test]
    fn test_parse_extern_crate() {
        let parser = TreeSitterParser::new().unwrap();
        let source = r#"
extern crate serde;
"#;
        let skeletons = parser.extract_skeletons(source).unwrap();
        assert!(!skeletons.is_empty());
        let extern_item = skeletons
            .iter()
            .find(|s| matches!(s.item_type, ItemType::ExternBlock));
        assert!(
            extern_item.is_some(),
            "Should detect extern crate as ExternBlock"
        );
    }
}
