//! Syn-based deep parser for Rust source code
//!
//! Provides detailed parsing for:
//! - Full AST analysis
//! - Generic parameters with bounds
//! - Where clause predicates
//! - Attribute extraction
//! - Visibility resolution
//! - Signature extraction

use crate::parsers::{GenericParam, ItemType, ParsedItem, SkeletonItem, Visibility, WhereClause};
use anyhow::{anyhow, Context, Result};
use quote::ToTokens;
use syn::{
    Attribute, GenericParam as SynGenericParam, Item as SynItem, ItemConst, ItemEnum,
    ItemExternCrate, ItemFn, ItemForeignMod, ItemImpl, ItemMod, ItemStatic, ItemStruct, ItemTrait,
    ItemType as SynItemType, ItemUse, ReturnType, Type, Visibility as SynVisibility,
    WhereClause as SynWhereClause, WherePredicate,
};

/// Maximum body_source length to store (to prevent memory explosion with expanded code)
const MAX_BODY_SOURCE_LEN: usize = 200;

/// Truncate body_source aggressively to prevent OOM on large expanded codebases
fn truncate_body_source(source: &str) -> String {
    if source.len() <= MAX_BODY_SOURCE_LEN {
        source.to_string()
    } else {
        format!("[BODY: {} bytes]", source.len())
    }
}

/// Syn-based parser for deep analysis
pub struct SynParser;

impl SynParser {
    /// Create a new syn parser
    pub fn new() -> Self {
        Self
    }

    /// Parse a single item from source
    pub fn parse_item(
        &self,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        // Try to parse as a valid Rust item
        let item: SynItem = syn::parse_str(source).with_context(|| {
            format!(
                "Failed to parse item: {}",
                source.lines().next().unwrap_or("")
            )
        })?;

        self.item_to_parsed(item, source, module_path, skeleton)
    }

    /// Convert syn Item to ParsedItem
    fn item_to_parsed(
        &self,
        item: SynItem,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        match item {
            SynItem::Fn(fn_item) => self.parse_function(fn_item, source, module_path, skeleton),
            SynItem::Struct(struct_item) => {
                self.parse_struct(struct_item, source, module_path, skeleton)
            }
            SynItem::Enum(enum_item) => self.parse_enum(enum_item, source, module_path, skeleton),
            SynItem::Trait(trait_item) => {
                self.parse_trait(trait_item, source, module_path, skeleton)
            }
            SynItem::Impl(impl_item) => self.parse_impl(impl_item, source, module_path, skeleton),
            SynItem::Type(type_item) => {
                self.parse_type_alias(type_item, source, module_path, skeleton)
            }
            SynItem::Const(const_item) => {
                self.parse_const(const_item, source, module_path, skeleton)
            }
            SynItem::Static(static_item) => {
                self.parse_static(static_item, source, module_path, skeleton)
            }
            SynItem::Mod(mod_item) => self.parse_module(mod_item, source, module_path, skeleton),
            SynItem::Use(use_item) => self.parse_use(use_item, source, module_path, skeleton),
            SynItem::ForeignMod(foreign_mod) => {
                self.parse_foreign_mod(foreign_mod, source, module_path, skeleton)
            }
            SynItem::ExternCrate(extern_crate) => {
                self.parse_extern_crate(extern_crate, source, module_path, skeleton)
            }
            _ => Err(anyhow!("Unsupported item type: {:?}", item)),
        }
    }

    /// Parse a function item
    fn parse_function(
        &self,
        item: ItemFn,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.sig.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_function_signature(&item);
        let generic_params = self.extract_generic_params(&item.sig.generics);
        let where_clauses = self.extract_where_clauses(&item.sig.generics.where_clause);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Function,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a struct item
    fn parse_struct(
        &self,
        item: ItemStruct,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_struct_signature(&item);
        let generic_params = self.extract_generic_params(&item.generics);
        let where_clauses = self.extract_where_clauses(&item.generics.where_clause);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Struct,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse an enum item
    fn parse_enum(
        &self,
        item: ItemEnum,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_enum_signature(&item);
        let generic_params = self.extract_generic_params(&item.generics);
        let where_clauses = self.extract_where_clauses(&item.generics.where_clause);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Enum,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a trait item
    fn parse_trait(
        &self,
        item: ItemTrait,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_trait_signature(&item);
        let generic_params = self.extract_generic_params(&item.generics);
        let where_clauses = self.extract_where_clauses(&item.generics.where_clause);
        let mut attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        // Detect async trait: check for #[async_trait] attribute or async fn methods
        let has_async_trait_attr = item.attrs.iter().any(|a| a.path().is_ident("async_trait"));
        let has_async_methods = item.items.iter().any(|ti| {
            if let syn::TraitItem::Fn(method) = ti {
                method.sig.asyncness.is_some()
            } else {
                false
            }
        });

        if has_async_trait_attr || has_async_methods {
            attributes.push("async_trait=true".to_string());
        }

        // Collect async method names for downstream consumers
        let async_methods: Vec<String> = item
            .items
            .iter()
            .filter_map(|ti| {
                if let syn::TraitItem::Fn(method) = ti {
                    if method.sig.asyncness.is_some() {
                        return Some(method.sig.ident.to_string());
                    }
                }
                None
            })
            .collect();

        if !async_methods.is_empty() {
            attributes.push(format!("async_methods={}", async_methods.join(",")));
        }

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Trait,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse an impl block
    fn parse_impl(
        &self,
        item: ItemImpl,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        // Extract the self type name
        let self_type = self.type_to_string(&item.self_ty);

        // Determine if this is a trait impl
        let (name, trait_fqn) = if let Some((_, path, _)) = &item.trait_ {
            let trait_name = path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            (format!("{}_{}", trait_name, self_type), Some(trait_name))
        } else {
            (self_type.clone(), None)
        };

        let visibility = Visibility::Public; // Impl blocks don't have visibility
        let signature = self.extract_impl_signature(&item);
        let generic_params = self.extract_generic_params(&item.generics);
        let where_clauses = self.extract_where_clauses(&item.generics.where_clause);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        // For impl blocks, we might want to include trait info in attributes
        let mut final_attributes = attributes;
        if let Some(trait_name) = trait_fqn {
            final_attributes.push(format!("impl_for={}", trait_name));
        }

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Impl,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes: final_attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a type alias
    fn parse_type_alias(
        &self,
        item: SynItemType,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_type_alias_signature(&item);
        let generic_params = self.extract_generic_params(&item.generics);
        let where_clauses = self.extract_where_clauses(&item.generics.where_clause);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::TypeAlias,
            name,
            visibility,
            signature,
            generic_params,
            where_clauses,
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a const item
    fn parse_const(
        &self,
        item: ItemConst,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_const_signature(&item);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Const,
            name,
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a static item
    fn parse_static(
        &self,
        item: ItemStatic,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = self.extract_static_signature(&item);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Static,
            name,
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a module item
    fn parse_module(
        &self,
        item: ItemMod,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = format!("mod {}", name);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::Module,
            name,
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a use declaration
    fn parse_use(
        &self,
        item: ItemUse,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = self.use_tree_to_string(&item.tree);
        let visibility = self.convert_visibility(&item.vis);
        let signature = format!("use {}", name);
        let attributes = self.extract_attributes(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name.replace("::", "_")),
            item_type: ItemType::Use,
            name,
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment: String::new(),
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse a foreign mod (extern block)
    fn parse_foreign_mod(
        &self,
        item: ItemForeignMod,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let abi_name = item
            .abi
            .name
            .as_ref()
            .map(|n| n.value())
            .unwrap_or_else(|| "C".to_string());
        let name = format!("extern_{}", abi_name);
        let signature = format!("extern \"{}\"", abi_name);
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::ExternBlock,
            name,
            visibility: Visibility::Public,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    /// Parse an extern crate declaration
    fn parse_extern_crate(
        &self,
        item: ItemExternCrate,
        source: &str,
        module_path: &str,
        skeleton: &SkeletonItem,
    ) -> Result<ParsedItem> {
        let name = item.ident.to_string();
        let visibility = self.convert_visibility(&item.vis);
        let signature = if let Some((_, rename)) = &item.rename {
            format!("extern crate {} as {}", name, rename)
        } else {
            format!("extern crate {}", name)
        };
        let attributes = self.extract_attributes(&item.attrs);
        let doc_comment = self.extract_doc_from_attrs(&item.attrs);

        Ok(ParsedItem {
            fqn: format!("{}::{}", module_path, name),
            item_type: ItemType::ExternBlock,
            name,
            visibility,
            signature,
            generic_params: Vec::new(),
            where_clauses: Vec::new(),
            attributes,
            doc_comment,
            start_line: skeleton.start_line,
            end_line: skeleton.end_line,
            body_source: truncate_body_source(source),
            generated_by: None,
        })
    }

    // ========================================================================
    // Helper Methods
    // ========================================================================

    /// Convert syn visibility to our Visibility enum
    fn convert_visibility(&self, vis: &SynVisibility) -> Visibility {
        match vis {
            SynVisibility::Public(_) => Visibility::Public,
            SynVisibility::Restricted(restricted) => {
                // Check if it's pub(crate), pub(super), or pub(in path)
                let path_str = restricted.path.to_token_stream().to_string();

                // The path in syn includes the leading `in` for pub(in path)
                if path_str == "crate" {
                    Visibility::PubCrate
                } else if path_str == "super" {
                    Visibility::PubSuper
                } else {
                    Visibility::PubIn(path_str)
                }
            }
            SynVisibility::Inherited => Visibility::Private,
        }
    }

    /// Extract generic parameters from syn Generics
    fn extract_generic_params(&self, generics: &syn::Generics) -> Vec<GenericParam> {
        generics
            .params
            .iter()
            .map(|param| match param {
                SynGenericParam::Type(type_param) => {
                    let bounds = type_param
                        .bounds
                        .iter()
                        .map(|b| self.type_bound_to_string(b))
                        .collect();

                    GenericParam {
                        name: type_param.ident.to_string(),
                        kind: "type".to_string(),
                        bounds,
                        default: type_param
                            .default
                            .as_ref()
                            .map(|d| d.to_token_stream().to_string().replace(" ", "")),
                    }
                }
                SynGenericParam::Lifetime(lifetime_param) => {
                    let bounds = lifetime_param
                        .bounds
                        .iter()
                        .map(|l| format!("'{}", l.ident))
                        .collect();

                    GenericParam {
                        name: format!("'{}", lifetime_param.lifetime.ident),
                        kind: "lifetime".to_string(),
                        bounds,
                        default: None,
                    }
                }
                SynGenericParam::Const(const_param) => GenericParam {
                    name: const_param.ident.to_string(),
                    kind: "const".to_string(),
                    bounds: vec![const_param.ty.to_token_stream().to_string()],
                    default: const_param
                        .default
                        .as_ref()
                        .map(|d| d.to_token_stream().to_string()),
                },
            })
            .collect()
    }

    /// Extract where clauses from syn WhereClause
    fn extract_where_clauses(&self, where_clause: &Option<SynWhereClause>) -> Vec<WhereClause> {
        where_clause
            .as_ref()
            .map(|wc| {
                wc.predicates
                    .iter()
                    .filter_map(|pred| match pred {
                        WherePredicate::Type(pred_type) => Some(WhereClause {
                            subject: self.type_to_string(&pred_type.bounded_ty),
                            bounds: pred_type
                                .bounds
                                .iter()
                                .map(|b| self.type_bound_to_string(b))
                                .collect(),
                        }),
                        WherePredicate::Lifetime(pred_lifetime) => Some(WhereClause {
                            subject: format!("'{}", pred_lifetime.lifetime.ident),
                            bounds: pred_lifetime
                                .bounds
                                .iter()
                                .map(|l| format!("'{}", l.ident))
                                .collect(),
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Extract attributes as strings
    fn extract_attributes(&self, attrs: &[Attribute]) -> Vec<String> {
        let mut result: Vec<String> = attrs
            .iter()
            .filter(|attr| {
                // Skip doc attributes as they're handled separately
                !attr.path().is_ident("doc")
            })
            .map(|attr| attr.to_token_stream().to_string())
            .collect();

        // Extract cfg conditions as structured metadata for downstream consumers
        let cfg_conditions = self.extract_cfg_conditions(attrs);
        if !cfg_conditions.is_empty() {
            result.push(format!("cfg_conditions={}", cfg_conditions.join(";")));
        }

        result
    }

    /// Extract #[cfg(...)] conditions from attributes
    fn extract_cfg_conditions(&self, attrs: &[Attribute]) -> Vec<String> {
        attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg"))
            .map(|attr| {
                // Get the token stream inside the cfg(...)
                attr.meta.to_token_stream().to_string()
            })
            .collect()
    }

    /// Extract doc comments from attributes
    fn extract_doc_from_attrs(&self, attrs: &[Attribute]) -> String {
        attrs
            .iter()
            .filter(|attr| attr.path().is_ident("doc"))
            .filter_map(|attr| {
                // #[doc = "..."] is a NameValue meta (from /// comments or explicit #[doc = "..."])
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(expr_lit) = &nv.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            return Some(lit_str.value());
                        }
                    }
                }
                // Fallback: try parse_args for #[doc("...")] style
                if let Ok(doc_lit) = attr.parse_args::<syn::LitStr>() {
                    Some(doc_lit.value())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Extract function signature
    fn extract_function_signature(&self, item: &ItemFn) -> String {
        let sig = &item.sig;

        let mut parts = Vec::new();

        // Visibility
        parts.push(item.vis.to_token_stream().to_string());

        // Const/async/unsafe
        if sig.constness.is_some() {
            parts.push("const".to_string());
        }
        if sig.asyncness.is_some() {
            parts.push("async".to_string());
        }
        if sig.unsafety.is_some() {
            parts.push("unsafe".to_string());
        }

        // fn name<generics>(params) -> return_type
        parts.push("fn".to_string());
        parts.push(sig.ident.to_string());

        // Generic params
        if !sig.generics.params.is_empty() {
            parts.push(
                sig.generics
                    .split_for_impl()
                    .0
                    .to_token_stream()
                    .to_string(),
            );
        }

        // Parameters
        parts.push(sig.inputs.to_token_stream().to_string());

        // Return type
        if let ReturnType::Type(_, ty) = &sig.output {
            parts.push(format!("-> {}", ty.to_token_stream()));
        }

        parts.join(" ")
    }

    /// Extract struct signature
    fn extract_struct_signature(&self, item: &ItemStruct) -> String {
        let mut parts = vec![
            item.vis.to_token_stream().to_string(),
            "struct".to_string(),
            item.ident.to_string(),
        ];

        if !item.generics.params.is_empty() {
            parts.push(
                item.generics
                    .split_for_impl()
                    .0
                    .to_token_stream()
                    .to_string(),
            );
        }

        parts.join(" ")
    }

    /// Extract enum signature
    fn extract_enum_signature(&self, item: &ItemEnum) -> String {
        let mut parts = vec![
            item.vis.to_token_stream().to_string(),
            "enum".to_string(),
            item.ident.to_string(),
        ];

        if !item.generics.params.is_empty() {
            parts.push(
                item.generics
                    .split_for_impl()
                    .0
                    .to_token_stream()
                    .to_string(),
            );
        }

        parts.join(" ")
    }

    /// Extract trait signature
    fn extract_trait_signature(&self, item: &ItemTrait) -> String {
        let mut parts = vec![item.vis.to_token_stream().to_string()];

        if item.unsafety.is_some() {
            parts.push("unsafe".to_string());
        }

        parts.push("trait".to_string());
        parts.push(item.ident.to_string());

        if !item.generics.params.is_empty() {
            parts.push(
                item.generics
                    .split_for_impl()
                    .0
                    .to_token_stream()
                    .to_string(),
            );
        }

        // Add supertraits
        if !item.supertraits.is_empty() {
            parts.push(":".to_string());
            parts.push(
                item.supertraits
                    .iter()
                    .map(|t| t.to_token_stream().to_string())
                    .collect::<Vec<_>>()
                    .join(" + "),
            );
        }

        parts.join(" ")
    }

    /// Extract impl signature
    fn extract_impl_signature(&self, item: &ItemImpl) -> String {
        let mut parts = vec!["impl".to_string()];

        if item.unsafety.is_some() {
            parts.insert(0, "unsafe".to_string());
        }

        if !item.generics.params.is_empty() {
            parts.push(
                item.generics
                    .split_for_impl()
                    .0
                    .to_token_stream()
                    .to_string(),
            );
        }

        if let Some((neg, path, _)) = &item.trait_ {
            if neg.is_some() {
                parts.push("!".to_string());
            }
            parts.push(path.to_token_stream().to_string());
            parts.push("for".to_string());
        }

        parts.push(item.self_ty.to_token_stream().to_string());

        parts.join(" ")
    }

    /// Extract type alias signature
    fn extract_type_alias_signature(&self, item: &SynItemType) -> String {
        format!(
            "{} type {} = {}",
            item.vis.to_token_stream(),
            item.ident,
            item.ty.to_token_stream()
        )
    }

    /// Extract const signature
    fn extract_const_signature(&self, item: &ItemConst) -> String {
        let mut sig = format!(
            "{} const {}: {}",
            item.vis.to_token_stream(),
            item.ident,
            item.ty.to_token_stream()
        );

        sig.push_str(&format!(" = {}", item.expr.to_token_stream()));

        sig
    }

    /// Extract static signature
    fn extract_static_signature(&self, item: &ItemStatic) -> String {
        let mut sig = format!(
            "{} static {}: {}",
            item.vis.to_token_stream(),
            item.ident,
            item.ty.to_token_stream()
        );

        sig.push_str(&format!(" = {}", item.expr.to_token_stream()));

        sig
    }

    /// Convert a type to string
    fn type_to_string(&self, ty: &Type) -> String {
        ty.to_token_stream().to_string()
    }

    /// Convert a type bound to string
    fn type_bound_to_string(&self, bound: &syn::TypeParamBound) -> String {
        bound.to_token_stream().to_string()
    }

    /// Convert use tree to string
    fn use_tree_to_string(&self, tree: &syn::UseTree) -> String {
        tree.to_token_stream().to_string()
    }
}

impl Default for SynParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skeleton(start: usize, end: usize) -> SkeletonItem {
        SkeletonItem {
            item_type: ItemType::Function,
            name: Some("test".to_string()),
            start_byte: start,
            end_byte: end,
            start_line: 1,
            end_line: 1,
        }
    }

    #[test]
    fn test_parse_function() {
        let parser = SynParser::new();
        let source = r#"pub fn hello<T: Clone + Send>(x: T) -> T where T: 'static {
    x.clone()
}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert_eq!(result.name, "hello");
        assert!(matches!(result.visibility, Visibility::Public));
        assert_eq!(result.generic_params.len(), 1);
        assert_eq!(result.generic_params[0].name, "T");
        assert!(result.generic_params[0]
            .bounds
            .contains(&"Clone".to_string()));
        assert!(result.generic_params[0]
            .bounds
            .contains(&"Send".to_string()));
    }

    #[test]
    fn test_parse_struct_with_generics() {
        let parser = SynParser::new();
        let source = r#"#[derive(Clone)]
pub struct Container<T, U: Sync> {
    inner: T,
    other: U,
}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert_eq!(result.name, "Container");
        assert_eq!(result.generic_params.len(), 2);
        assert_eq!(result.generic_params[0].name, "T");
        assert_eq!(result.generic_params[1].name, "U");
    }

    #[test]
    fn test_parse_where_clause() {
        let parser = SynParser::new();
        let source = r#"pub fn process<T, U>(x: T, y: U) -> bool
where
    T: Clone + Send,
    U: Sync + 'static,
{
    true
}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(result.where_clauses.len() >= 2);

        let t_clause = result
            .where_clauses
            .iter()
            .find(|c| c.subject == "T")
            .expect("Should have T clause");
        assert!(t_clause.bounds.contains(&"Clone".to_string()));

        let u_clause = result
            .where_clauses
            .iter()
            .find(|c| c.subject == "U")
            .expect("Should have U clause");
        assert!(u_clause.bounds.contains(&"Sync".to_string()));
    }

    #[test]
    fn test_parse_impl() {
        let parser = SynParser::new();
        let source = r#"impl<T: Clone> MyTrait for Container<T> {
    fn do_thing(&self) -> T {
        self.inner.clone()
    }
}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::Impl));
        assert!(result.name.contains("MyTrait"));
        assert!(result
            .attributes
            .iter()
            .any(|a| a.contains("impl_for=MyTrait")));
    }

    #[test]
    fn test_visibility_parsing() {
        let parser = SynParser::new();

        // Test pub
        let source = "pub fn test() {}";
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();
        assert!(matches!(result.visibility, Visibility::Public));

        // Test pub(crate)
        let source = "pub(crate) fn test() {}";
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();
        assert!(matches!(result.visibility, Visibility::PubCrate));

        // Test private
        let source = "fn test() {}";
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();
        assert!(matches!(result.visibility, Visibility::Private));
    }

    #[test]
    fn test_attribute_extraction() {
        let parser = SynParser::new();
        let source = r#"#[derive(Clone, Debug)]
#[cfg(feature = "test")]
#[inline]
pub struct Point {
    x: i32,
}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(result.attributes.iter().any(|a| a.contains("derive")));
        assert!(result.attributes.iter().any(|a| a.contains("cfg")));
        assert!(result.attributes.iter().any(|a| a.contains("inline")));
    }

    #[test]
    fn test_doc_comment_extraction() {
        let parser = SynParser::new();
        let source = r#"/// This is a test function.
/// It does something useful.
#[doc = "More docs"]
pub fn test() {}"#;

        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(result.doc_comment.contains("This is a test function"));
        assert!(result.doc_comment.contains("It does something useful"));
        assert!(result.doc_comment.contains("More docs"));
    }

    #[test]
    fn test_parse_extern_block() {
        let parser = SynParser::new();
        let source = r#"extern "C" {
    fn printf(format: *const i8, ...) -> i32;
    fn malloc(size: usize) -> *mut u8;
}"#;
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::ExternBlock));
        assert_eq!(result.name, "extern_C");
        assert!(result.signature.contains("extern \"C\""));
    }

    #[test]
    fn test_parse_extern_crate() {
        let parser = SynParser::new();
        let source = "extern crate serde;";
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::ExternBlock));
        assert_eq!(result.name, "serde");
        assert!(result.signature.contains("extern crate serde"));
    }

    #[test]
    fn test_parse_extern_crate_renamed() {
        let parser = SynParser::new();
        let source = "extern crate serde as serde_lib;";
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::ExternBlock));
        assert_eq!(result.name, "serde");
        assert!(result.signature.contains("as serde_lib"));
    }

    #[test]
    fn test_parse_async_trait() {
        let parser = SynParser::new();
        let source = r#"pub trait AsyncService {
    async fn connect(&self) -> Result<(), Error>;
    async fn disconnect(&self);
    fn name(&self) -> &str;
}"#;
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::Trait));
        assert_eq!(result.name, "AsyncService");
        assert!(result.attributes.iter().any(|a| a == "async_trait=true"));
        assert!(result
            .attributes
            .iter()
            .any(|a| a.starts_with("async_methods=")));
        // Verify the async method names are tracked
        let async_methods_attr = result
            .attributes
            .iter()
            .find(|a| a.starts_with("async_methods="))
            .unwrap();
        assert!(async_methods_attr.contains("connect"));
        assert!(async_methods_attr.contains("disconnect"));
        assert!(!async_methods_attr.contains("name"));
    }

    #[test]
    fn test_parse_sync_trait_no_async_marker() {
        let parser = SynParser::new();
        let source = r#"pub trait SyncService {
    fn process(&self) -> bool;
}"#;
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(matches!(result.item_type, ItemType::Trait));
        assert!(!result.attributes.iter().any(|a| a == "async_trait=true"));
        assert!(!result
            .attributes
            .iter()
            .any(|a| a.starts_with("async_methods=")));
    }

    #[test]
    fn test_cfg_condition_extraction() {
        let parser = SynParser::new();
        let source = r#"#[cfg(feature = "async")]
#[cfg(target_os = "linux")]
pub fn platform_specific() {}"#;
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        // Should have the raw cfg attributes
        assert!(result.attributes.iter().any(|a| a.contains("cfg")));
        // Should have the structured cfg_conditions metadata
        let cfg_attr = result
            .attributes
            .iter()
            .find(|a| a.starts_with("cfg_conditions="))
            .expect("Should have cfg_conditions attribute");
        assert!(cfg_attr.contains("feature"));
        assert!(cfg_attr.contains("target_os"));
    }

    #[test]
    fn test_no_cfg_conditions_when_absent() {
        let parser = SynParser::new();
        let source = r#"#[derive(Clone)]
pub fn simple() {}"#;
        let skeleton = make_skeleton(0, source.len());
        let result = parser.parse_item(source, "test", &skeleton).unwrap();

        assert!(!result
            .attributes
            .iter()
            .any(|a| a.starts_with("cfg_conditions=")));
    }
}
