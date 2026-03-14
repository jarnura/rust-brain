//! Text representation templates for embedding
//!
//! Converts parsed Rust items into human-readable text representations
//! optimized for semantic embedding.

use crate::parsers::{GenericParam, ItemType, ParsedItem, Visibility, WhereClause};

/// Maximum lines to include in body preview
const MAX_BODY_PREVIEW_LINES: usize = 10;

/// Text representation of a parsed item for embedding
#[derive(Debug, Clone)]
pub struct TextRepresentation {
    /// The text to embed
    pub text: String,
    /// Item type for categorization
    pub item_type: String,
    /// Whether this is a doc chunk (vs code item)
    pub is_doc_chunk: bool,
}

/// Chunk of documentation for separate embedding
#[derive(Debug, Clone)]
pub struct DocChunk {
    /// The documentation text
    pub text: String,
    /// Source item FQN
    pub source_fqn: String,
    /// Source item type
    pub source_item_type: String,
    /// Chunk index
    pub chunk_index: usize,
}

/// Generate text representation for a parsed item
pub fn generate_text_representation(item: &ParsedItem) -> TextRepresentation {
    let text = match &item.item_type {
        ItemType::Function => generate_function_text(item),
        ItemType::Struct => generate_struct_text(item),
        ItemType::Enum => generate_enum_text(item),
        ItemType::Trait => generate_trait_text(item),
        ItemType::Impl => generate_impl_text(item),
        ItemType::TypeAlias => generate_type_alias_text(item),
        ItemType::Const => generate_const_text(item),
        ItemType::Static => generate_static_text(item),
        ItemType::Macro => generate_macro_text(item),
        ItemType::Module => generate_module_text(item),
        ItemType::Use => generate_use_text(item),
        ItemType::Unknown(s) => generate_unknown_text(item, s),
    };
    
    TextRepresentation {
        text,
        item_type: item.item_type.as_str().to_string(),
        is_doc_chunk: false,
    }
}

/// Extract doc chunks from an item's documentation
pub fn extract_doc_chunks(item: &ParsedItem, max_chunk_size: usize) -> Vec<DocChunk> {
    if item.doc_comment.is_empty() {
        return Vec::new();
    }
    
    // Split by paragraph boundaries (double newlines)
    let paragraphs: Vec<&str> = item.doc_comment
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .collect();
    
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut chunk_index = 0;
    
    for paragraph in paragraphs {
        // If adding this paragraph would exceed max size, start a new chunk
        if !current_chunk.is_empty() && current_chunk.len() + paragraph.len() + 2 > max_chunk_size {
            if !current_chunk.trim().is_empty() {
                chunks.push(DocChunk {
                    text: current_chunk.trim().to_string(),
                    source_fqn: item.fqn.clone(),
                    source_item_type: item.item_type.as_str().to_string(),
                    chunk_index,
                });
                chunk_index += 1;
            }
            current_chunk = String::new();
        }
        
        // If paragraph itself is too large, split by sentences
        if paragraph.len() > max_chunk_size {
            // First, add any accumulated content
            if !current_chunk.trim().is_empty() {
                chunks.push(DocChunk {
                    text: current_chunk.trim().to_string(),
                    source_fqn: item.fqn.clone(),
                    source_item_type: item.item_type.as_str().to_string(),
                    chunk_index,
                });
                chunk_index += 1;
                current_chunk = String::new();
            }
            
            // Split by sentences (rough heuristic)
            for sentence_chunk in split_by_sentences(paragraph, max_chunk_size) {
                chunks.push(DocChunk {
                    text: sentence_chunk,
                    source_fqn: item.fqn.clone(),
                    source_item_type: item.item_type.as_str().to_string(),
                    chunk_index,
                });
                chunk_index += 1;
            }
        } else {
            if !current_chunk.is_empty() {
                current_chunk.push_str("\n\n");
            }
            current_chunk.push_str(paragraph);
        }
    }
    
    // Add final chunk
    if !current_chunk.trim().is_empty() {
        chunks.push(DocChunk {
            text: current_chunk.trim().to_string(),
            source_fqn: item.fqn.clone(),
            source_item_type: item.item_type.as_str().to_string(),
            chunk_index,
        });
    }
    
    chunks
}

/// Split text by sentences when it exceeds max size
fn split_by_sentences(text: &str, max_size: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    
    // Simple sentence splitting on common punctuation
    for sentence in text.split_inclusive(&['.', '!', '?', '\n']) {
        if current.len() + sentence.len() > max_size && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = String::new();
        }
        current.push_str(sentence);
    }
    
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    
    chunks
}

/// Generate text for functions
fn generate_function_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let generics = generics_to_string(&item.generic_params);
    let params = extract_params_from_signature(&item.signature);
    let return_type = extract_return_type_from_signature(&item.signature);
    let trait_bounds = where_clauses_to_string(&item.where_clauses);
    let body_preview = extract_body_preview(&item.body_source, MAX_BODY_PREVIEW_LINES);
    
    // Extract module and crate from FQN
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} fn {name}{generics}({params}) -> {return_type}
{doc_comment}
Module: {module_path}
Crate: {crate_name}
Traits used: {trait_bounds}
Body preview:
{body_preview}"#,
        visibility = visibility,
        name = item.name,
        generics = generics,
        params = params,
        return_type = return_type,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
        trait_bounds = trait_bounds,
        body_preview = body_preview,
    )
}

/// Generate text for structs
fn generate_struct_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let generics = generics_to_string(&item.generic_params);
    let field_summary = extract_struct_fields(&item.body_source);
    let derive_traits = extract_derive_traits(&item.attributes);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} struct {name}{generics} {{ {field_summary} }}
{doc_comment}
Derives: {derive_traits}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        generics = generics,
        field_summary = field_summary,
        doc_comment = format_doc_comment(&item.doc_comment),
        derive_traits = derive_traits,
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for enums
fn generate_enum_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let generics = generics_to_string(&item.generic_params);
    let variant_summary = extract_enum_variants(&item.body_source);
    let derive_traits = extract_derive_traits(&item.attributes);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} enum {name}{generics} {{ {variant_summary} }}
{doc_comment}
Derives: {derive_traits}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        generics = generics,
        variant_summary = variant_summary,
        doc_comment = format_doc_comment(&item.doc_comment),
        derive_traits = derive_traits,
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for traits
fn generate_trait_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let generics = generics_to_string(&item.generic_params);
    let supertraits = extract_supertraits(&item.signature);
    let method_summary = extract_trait_methods(&item.body_source);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} trait {name}{generics}{supertraits} {{ {method_summary} }}
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        generics = generics,
        supertraits = if supertraits.is_empty() { String::new() } else { format!(": {}", supertraits) },
        method_summary = method_summary,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for impl blocks
fn generate_impl_text(item: &ParsedItem) -> String {
    let generics = generics_to_string(&item.generic_params);
    let impl_signature = extract_impl_signature(&item.signature);
    let method_summary = extract_impl_methods(&item.body_source);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"impl{generics} {impl_signature} {{ {method_summary} }}
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        generics = generics,
        impl_signature = impl_signature,
        method_summary = method_summary,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for type aliases
fn generate_type_alias_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let generics = generics_to_string(&item.generic_params);
    let target_type = extract_type_alias_target(&item.signature);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} type {name}{generics} = {target_type};
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        generics = generics,
        target_type = target_type,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for const items
fn generate_const_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let (ty, value) = extract_const_info(&item.signature);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} const {name}: {ty} = {value};
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        ty = ty,
        value = value,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for static items
fn generate_static_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let (ty, value) = extract_static_info(&item.signature);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} static {name}: {ty} = {value};
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        ty = ty,
        value = value,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for macros
fn generate_macro_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let macro_rules = extract_macro_rules(&item.body_source);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} macro_rules! {name} {{ {macro_rules} }}
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        macro_rules = macro_rules,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for modules
fn generate_module_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} mod {name}
{doc_comment}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        name = item.name,
        doc_comment = format_doc_comment(&item.doc_comment),
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for use declarations
fn generate_use_text(item: &ParsedItem) -> String {
    let visibility = visibility_to_string(&item.visibility);
    let import_path = extract_use_path(&item.signature);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} use {import_path}
Module: {module_path}
Crate: {crate_name}"#,
        visibility = visibility,
        import_path = import_path,
        module_path = module_path,
        crate_name = crate_name,
    )
}

/// Generate text for unknown item types
fn generate_unknown_text(item: &ParsedItem, type_name: &str) -> String {
    let visibility = visibility_to_string(&item.visibility);
    
    let (crate_name, module_path) = split_fqn(&item.fqn);
    
    format!(
        r#"{visibility} {type_name} {name}
{doc_comment}
Signature: {signature}
Module: {module_path}
Crate: {crate_name}
Body:
{body}"#,
        visibility = visibility,
        type_name = type_name,
        name = item.name,
        doc_comment = format_doc_comment(&item.doc_comment),
        signature = item.signature,
        module_path = module_path,
        crate_name = crate_name,
        body = item.body_source,
    )
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Convert visibility to string
fn visibility_to_string(vis: &Visibility) -> &'static str {
    match vis {
        Visibility::Public => "pub",
        Visibility::PubCrate => "pub(crate)",
        Visibility::PubSuper => "pub(super)",
        Visibility::PubIn(_) => "pub(in ...)",
        Visibility::Private => "",
    }
}

/// Format generic parameters
fn generics_to_string(generics: &[GenericParam]) -> String {
    if generics.is_empty() {
        return String::new();
    }
    
    let params: Vec<String> = generics
        .iter()
        .map(|g| {
            let mut s = match g.kind.as_str() {
                "lifetime" => format!("'{}", g.name),
                _ => g.name.clone(),
            };
            
            if !g.bounds.is_empty() {
                s.push_str(": ");
                s.push_str(&g.bounds.join(" + "));
            }
            
            if let Some(default) = &g.default {
                s.push_str(" = ");
                s.push_str(default);
            }
            
            s
        })
        .collect();
    
    format!("<{}>", params.join(", "))
}

/// Format where clauses
fn where_clauses_to_string(where_clauses: &[WhereClause]) -> String {
    if where_clauses.is_empty() {
        return "none".to_string();
    }
    
    let clauses: Vec<String> = where_clauses
        .iter()
        .map(|wc| {
            format!("{}: {}", wc.subject, wc.bounds.join(" + "))
        })
        .collect();
    
    clauses.join(", ")
}

/// Extract function parameters from signature
fn extract_params_from_signature(signature: &str) -> String {
    // Find content between parentheses
    if let Some(start) = signature.find('(') {
        if let Some(end) = signature.rfind(')') {
            let params = &signature[start + 1..end];
            // Clean up and format
            return params
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
        }
    }
    String::new()
}

/// Extract return type from signature
fn extract_return_type_from_signature(signature: &str) -> String {
    // Find -> and extract return type
    if let Some(pos) = signature.find("->") {
        let after_arrow = &signature[pos + 2..];
        // Take up to where clause or opening brace
        let end = after_arrow
            .find("where")
            .or_else(|| after_arrow.find('{'))
            .unwrap_or(after_arrow.len());
        after_arrow[..end].trim().to_string()
    } else {
        "()".to_string()
    }
}

/// Extract body preview (first N lines)
fn extract_body_preview(body: &str, max_lines: usize) -> String {
    body.lines()
        .take(max_lines)
        .enumerate()
        .map(|(i, line)| format!("{:4}: {}", i + 1, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format doc comment for embedding
fn format_doc_comment(doc: &str) -> String {
    if doc.is_empty() {
        return "No documentation".to_string();
    }
    format!("Documentation: {}", doc.trim())
}

/// Split FQN into crate and module path
fn split_fqn(fqn: &str) -> (String, String) {
    let parts: Vec<&str> = fqn.split("::").collect();
    if parts.is_empty() {
        return (String::new(), String::new());
    }
    
    let crate_name = parts[0].to_string();
    let module_path = if parts.len() > 1 {
        parts[..parts.len() - 1].join("::")
    } else {
        crate_name.clone()
    };
    
    (crate_name, module_path)
}

/// Extract struct fields from body
fn extract_struct_fields(body: &str) -> String {
    let mut fields = Vec::new();
    let mut in_field = false;
    let mut current_field = String::new();
    let mut brace_count = 0;
    
    for line in body.lines() {
        // Skip attributes and doc comments
        let trimmed = line.trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        
        // Track braces
        for c in trimmed.chars() {
            match c {
                '{' => brace_count += 1,
                '}' => brace_count -= 1,
                _ => {}
            }
        }
        
        // After opening brace, we're in fields
        if brace_count > 0 {
            in_field = true;
        }
        
        if in_field && trimmed.contains(':') && !trimmed.starts_with("//") {
            // Extract field name and type
            if let Some(colon_pos) = trimmed.find(':') {
                let field_part = trimmed[..colon_pos].trim();
                if !field_part.is_empty() && !field_part.starts_with("where") {
                    fields.push(field_part.to_string());
                }
            }
        }
    }
    
    if fields.is_empty() {
        "no fields".to_string()
    } else {
        fields.join(", ")
    }
}

/// Extract enum variants from body
fn extract_enum_variants(body: &str) -> String {
    let mut variants = Vec::new();
    let mut in_enum = false;
    let mut brace_count = 0;
    
    for line in body.lines() {
        let trimmed = line.trim();
        
        // Skip attributes and doc comments
        if trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        
        // Track braces
        for c in trimmed.chars() {
            match c {
                '{' => brace_count += 1,
                '}' => brace_count -= 1,
                _ => {}
            }
        }
        
        // After opening brace
        if brace_count > 0 {
            in_enum = true;
        }
        
        if in_enum {
            // Look for variant names (identifiers before { or ( or = or ,)
            if let Some(first_char) = trimmed.chars().next() {
                if first_char.is_ascii_uppercase() || first_char == '_' {
                    // Extract variant name
                    let end_pos = trimmed
                        .find('{')
                        .or_else(|| trimmed.find('('))
                        .or_else(|| trimmed.find('='))
                        .or_else(|| trimmed.find(','))
                        .unwrap_or(trimmed.len());
                    
                    let variant_name = trimmed[..end_pos].trim();
                    if !variant_name.is_empty() && variant_name.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) {
                        variants.push(variant_name.to_string());
                    }
                }
            }
        }
    }
    
    if variants.is_empty() {
        "no variants".to_string()
    } else {
        variants.join(", ")
    }
}

/// Extract derive traits from attributes
fn extract_derive_traits(attributes: &[String]) -> String {
    for attr in attributes {
        if attr.contains("derive") {
            // Extract content from #[derive(...)]
            if let Some(start) = attr.find('(') {
                if let Some(end) = attr.rfind(')') {
                    return attr[start + 1..end].trim().to_string();
                }
            }
        }
    }
    "none".to_string()
}

/// Extract supertraits from trait signature
fn extract_supertraits(signature: &str) -> String {
    if let Some(colon_pos) = signature.find(':') {
        // Find where supertraits end (before { or where)
        let after_colon = &signature[colon_pos + 1..];
        let end = after_colon
            .find('{')
            .or_else(|| after_colon.find("where"))
            .unwrap_or(after_colon.len());
        after_colon[..end].trim().to_string()
    } else {
        String::new()
    }
}

/// Extract trait methods from body
fn extract_trait_methods(body: &str) -> String {
    let mut methods = Vec::new();
    
    for line in body.lines() {
        let trimmed = line.trim();
        
        // Skip attributes and doc comments
        if trimmed.starts_with("#[") || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        
        // Look for fn declarations
        if trimmed.starts_with("fn ") || trimmed.starts_with("async fn ") {
            // Extract method name
            let fn_part = trimmed.trim_start_matches("async").trim();
            if let Some(name_start) = fn_part.find("fn ") {
                let after_fn = &fn_part[name_start + 3..];
                let name_end = after_fn
                    .find('<')
                    .or_else(|| after_fn.find('('))
                    .unwrap_or(after_fn.len());
                let name = after_fn[..name_end].trim();
                if !name.is_empty() {
                    methods.push(name.to_string());
                }
            }
        }
    }
    
    if methods.is_empty() {
        "no methods".to_string()
    } else {
        methods.join(", ")
    }
}

/// Extract impl signature (trait for type or just type)
fn extract_impl_signature(signature: &str) -> String {
    // Already formatted in signature field
    signature
        .trim_start_matches("impl")
        .trim()
        .trim_end_matches('{')
        .trim()
        .to_string()
}

/// Extract impl methods from body
fn extract_impl_methods(body: &str) -> String {
    extract_trait_methods(body) // Same logic
}

/// Extract type alias target type
fn extract_type_alias_target(signature: &str) -> String {
    if let Some(eq_pos) = signature.find('=') {
        let after_eq = &signature[eq_pos + 1..];
        let end = after_eq.find(';').unwrap_or(after_eq.len());
        after_eq[..end].trim().to_string()
    } else {
        "unknown".to_string()
    }
}

/// Extract const type and value
fn extract_const_info(signature: &str) -> (String, String) {
    // Format: const NAME: Type = value;
    let mut ty = "unknown".to_string();
    let mut value = "unknown".to_string();
    
    if let Some(colon_pos) = signature.find(':') {
        let after_colon = &signature[colon_pos + 1..];
        if let Some(eq_pos) = after_colon.find('=') {
            ty = after_colon[..eq_pos].trim().to_string();
            let after_eq = &after_colon[eq_pos + 1..];
            let end = after_eq.find(';').unwrap_or(after_eq.len());
            value = after_eq[..end].trim().to_string();
        }
    }
    
    (ty, value)
}

/// Extract static type and value
fn extract_static_info(signature: &str) -> (String, String) {
    extract_const_info(signature) // Same format
}

/// Extract macro rules (simplified)
fn extract_macro_rules(body: &str) -> String {
    // Just take first few patterns
    let mut patterns = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    
    for c in body.chars() {
        match c {
            '{' => {
                depth += 1;
                if depth > 1 {
                    current.push(c);
                }
            }
            '}' => {
                depth -= 1;
                if depth == 1 {
                    if !current.trim().is_empty() && patterns.len() < 3 {
                        patterns.push(current.trim().to_string());
                    }
                    current = String::new();
                } else if depth > 1 {
                    current.push(c);
                }
            }
            _ => {
                if depth > 1 {
                    current.push(c);
                }
            }
        }
    }
    
    if patterns.is_empty() {
        "simplified".to_string()
    } else {
        patterns.join(" | ")
    }
}

/// Extract use path from signature
fn extract_use_path(signature: &str) -> String {
    signature
        .trim_start_matches("use")
        .trim()
        .trim_end_matches(';')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_split_fqn() {
        let (crate_name, module_path) = split_fqn("my_crate::module::submodule::function");
        assert_eq!(crate_name, "my_crate");
        assert_eq!(module_path, "my_crate::module::submodule");
    }
    
    #[test]
    fn test_extract_doc_chunks() {
        let item = ParsedItem {
            fqn: "test::module::func".to_string(),
            item_type: ItemType::Function,
            name: "func".to_string(),
            visibility: Visibility::Public,
            signature: "pub fn func()".to_string(),
            generic_params: vec![],
            where_clauses: vec![],
            attributes: vec![],
            doc_comment: "This is paragraph one.\n\nThis is paragraph two.\n\nThis is paragraph three.".to_string(),
            start_line: 1,
            end_line: 5,
            body_source: "pub fn func() {}".to_string(),
            generated_by: None,
        };
        
        let chunks = extract_doc_chunks(&item, 50);
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|c| c.source_fqn == "test::module::func"));
    }
    
    #[test]
    fn test_generics_to_string() {
        let generics = vec![
            GenericParam {
                name: "T".to_string(),
                kind: "type".to_string(),
                bounds: vec!["Clone".to_string(), "Send".to_string()],
                default: None,
            },
            GenericParam {
                name: "a".to_string(),
                kind: "lifetime".to_string(),
                bounds: vec![],
                default: None,
            },
        ];
        
        let result = generics_to_string(&generics);
        assert!(result.contains("T: Clone + Send"));
        assert!(result.contains("'a"));
    }
}
