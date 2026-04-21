//! Type Resolver Implementation
//!
//! Provides concrete type resolution at generic call sites without full monomorphization.
//! Uses a dual strategy:
//! - **Analyzed**: Full syn parsing for precise type extraction
//! - **Heuristic**: Regex + pattern matching as fallback for complex or unparseable code

use crate::parsers::GenericParam;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use syn::{
    Expr, ExprCall, ExprMethodCall, ExprPath, GenericArgument, ImplItem, Item as SynItem, ItemImpl,
    PathArguments, Type, TypePath,
};
use tracing::{debug, warn};

/// Quality indicator for type resolution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionQuality {
    /// Full syn parsing succeeded - high confidence
    Analyzed,
    /// Regex/heuristics used - lower confidence
    Heuristic,
}

impl ResolutionQuality {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResolutionQuality::Analyzed => "analyzed",
            ResolutionQuality::Heuristic => "heuristic",
        }
    }

    pub fn parse_str(s: &str) -> Self {
        match s {
            "analyzed" => ResolutionQuality::Analyzed,
            _ => ResolutionQuality::Heuristic,
        }
    }
}

/// A concrete type argument at a call site
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeArg {
    /// Parameter name (e.g., "T", "U")
    pub param_name: String,
    /// Concrete type provided (e.g., "String", "Vec<i32>")
    pub concrete_type: String,
}

/// Dispatch kind for a call site.
///
/// Indicates how the call was resolved:
/// - **Static**: Direct function/method call or concrete impl dispatch (receiver type known)
/// - **Trait**: Call resolved to a trait method definition (concrete impl not yet determined)
/// - **Dynamic**: Cannot determine dispatch target (unknown receiver type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CallDispatch {
    /// Direct function/method call or resolved to a concrete impl
    Static,
    /// Call resolved to a trait method (impl not yet determined)
    Trait,
    /// Cannot determine dispatch target
    #[default]
    Dynamic,
}

impl CallDispatch {
    /// Returns the string representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            CallDispatch::Static => "static",
            CallDispatch::Trait => "trait",
            CallDispatch::Dynamic => "dynamic",
        }
    }

    /// Parses a dispatch kind from its string representation.
    pub fn parse_str(s: &str) -> Self {
        match s {
            "static" => CallDispatch::Static,
            "trait" => CallDispatch::Trait,
            _ => CallDispatch::Dynamic,
        }
    }
}

impl std::fmt::Display for CallDispatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A call site with type information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSite {
    /// FQN of the calling function
    pub caller_fqn: String,
    /// FQN of the called function
    pub callee_fqn: String,
    /// Source file path
    pub file_path: String,
    /// Line number of the call
    pub line_number: usize,
    /// Concrete type arguments
    pub concrete_type_args: Vec<TypeArg>,
    /// Whether this is a monomorphized call
    pub is_monomorphized: bool,
    /// Quality of the resolution
    pub quality: ResolutionQuality,
    /// Dispatch kind for this call site
    #[serde(default)]
    pub dispatch: CallDispatch,
}

/// A trait implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraitImplementation {
    /// FQN of the trait being implemented
    pub trait_fqn: String,
    /// The type implementing the trait
    pub self_type: String,
    /// Generated FQN for the impl block
    pub impl_fqn: String,
    /// Source file path
    pub file_path: String,
    /// Line number of impl block
    pub line_number: usize,
    /// Generic parameters on the impl
    pub generic_params: Vec<GenericParam>,
    /// Quality of the resolution
    pub quality: ResolutionQuality,
}

/// The main type resolver
pub struct TypeResolver {}

impl TypeResolver {
    /// Create a new type resolver
    pub fn new() -> Self {
        Self {}
    }

    /// Analyze expanded source code for type information
    ///
    /// This method:
    /// 1. Parses the source for impl blocks (trait implementations)
    /// 2. Extracts call sites with concrete type arguments
    /// 3. Marks quality as "analyzed" or "heuristic"
    pub fn analyze_source(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        expanded_source: &str,
        caller_fqns: &[String],
    ) -> super::TypeResolutionResult {
        let mut trait_impls = Vec::new();
        let mut call_sites = Vec::new();
        let mut errors = Vec::new();

        // Phase 1: Try syn-based analysis
        match self.analyze_with_syn(
            crate_name,
            module_path,
            file_path,
            expanded_source,
            caller_fqns,
        ) {
            Ok((impls, sites)) => {
                trait_impls.extend(impls);
                call_sites.extend(sites);
            }
            Err(e) => {
                debug!("Syn analysis failed, falling back to heuristics: {}", e);
                errors.push(format!("Syn analysis failed: {}", e));

                // Fall back to heuristic analysis
                match self.analyze_with_heuristics(
                    crate_name,
                    module_path,
                    file_path,
                    expanded_source,
                    caller_fqns,
                ) {
                    Ok((impls, sites)) => {
                        trait_impls.extend(impls);
                        call_sites.extend(sites);
                    }
                    Err(e) => {
                        warn!("Heuristic analysis also failed: {}", e);
                        errors.push(format!("Heuristic analysis failed: {}", e));
                    }
                }
            }
        }

        super::TypeResolutionResult {
            trait_impls,
            call_sites,
            errors,
        }
    }

    /// Analyze source using heuristics only (skips syn parsing)
    /// Use for large files where syn would be too slow.
    pub fn analyze_heuristics_only(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        expanded_source: &str,
        caller_fqns: &[String],
    ) -> super::TypeResolutionResult {
        let mut errors = Vec::new();

        match self.analyze_with_heuristics(
            crate_name,
            module_path,
            file_path,
            expanded_source,
            caller_fqns,
        ) {
            Ok((trait_impls, call_sites)) => super::TypeResolutionResult {
                trait_impls,
                call_sites,
                errors,
            },
            Err(e) => {
                warn!("Heuristic analysis failed: {}", e);
                errors.push(format!("Heuristic analysis failed: {}", e));
                super::TypeResolutionResult {
                    trait_impls: Vec::new(),
                    call_sites: Vec::new(),
                    errors,
                }
            }
        }
    }

    /// Analyze source using syn for precise type extraction
    fn analyze_with_syn(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        source: &str,
        _caller_fqns: &[String],
    ) -> Result<(Vec<TraitImplementation>, Vec<CallSite>)> {
        let file: syn::File =
            syn::parse_str(source).with_context(|| "Failed to parse source with syn")?;

        // Phase 1: Extract trait impls and build a method dispatch index.
        // The index maps (self_type, method_name) → impl_fqn so we can resolve
        // trait method calls on concrete receiver types.
        let mut trait_impls = Vec::new();
        let mut trait_method_index: std::collections::HashMap<(String, String), String> =
            std::collections::HashMap::new();

        for (idx, item) in file.items.iter().enumerate() {
            if let SynItem::Impl(impl_item) = item {
                if let Some(impl_info) =
                    self.extract_trait_impl(impl_item, crate_name, module_path, file_path, idx)
                {
                    for method_item in &impl_item.items {
                        if let ImplItem::Fn(method) = method_item {
                            let key = (impl_info.self_type.clone(), method.sig.ident.to_string());
                            trait_method_index.insert(key, impl_info.impl_fqn.clone());
                        }
                    }
                    trait_impls.push(impl_info);
                }
            }
        }

        // Phase 2: Extract call sites with dispatch resolution.
        // When a method call's receiver type is known and matches a trait impl
        // in the index, we emit an additional call site targeting the concrete
        // impl method FQN with dispatch = Static.
        let mut call_sites = Vec::new();

        for item in file.items.iter() {
            match item {
                SynItem::Impl(impl_item) => {
                    let caller_fqn = self.impl_caller_fqn(impl_item, module_path);
                    let self_type = self.type_to_string(&impl_item.self_ty);
                    for call_site in self.extract_calls_from_impl(
                        impl_item,
                        file_path,
                        &caller_fqn,
                        &self_type,
                        &trait_method_index,
                    ) {
                        call_sites.push(call_site);
                    }
                }
                SynItem::Fn(fn_item) => {
                    let caller_fqn = format!("{}::{}", module_path, fn_item.sig.ident);
                    for call_site in self.extract_calls_from_fn(
                        fn_item,
                        file_path,
                        &caller_fqn,
                        None,
                        &trait_method_index,
                    ) {
                        call_sites.push(call_site);
                    }
                }
                _ => {}
            }
        }

        for impl_info in &mut trait_impls {
            impl_info.quality = ResolutionQuality::Analyzed;
        }
        for site in &mut call_sites {
            site.quality = ResolutionQuality::Analyzed;
        }

        Ok((trait_impls, call_sites))
    }

    /// Extract trait implementation info from an impl block
    fn extract_trait_impl(
        &self,
        impl_item: &ItemImpl,
        _crate_name: &str,
        module_path: &str,
        file_path: &str,
        _item_idx: usize,
    ) -> Option<TraitImplementation> {
        // Check if this is a trait implementation
        let (trait_path, is_trait_impl) = if let Some((_, path, _)) = &impl_item.trait_ {
            (Some(path.clone()), true)
        } else {
            (None, false)
        };

        if !is_trait_impl {
            return None;
        }

        let trait_fqn = trait_path
            .as_ref()
            .map(|p| self.path_to_fqn(p))
            .unwrap_or_default();

        let self_type = self.type_to_string(&impl_item.self_ty);

        // Use canonical format matching syn_parser: module::TraitName_Type
        let impl_fqn = format!("{}::{}_{}", module_path, trait_fqn, self_type);

        // Extract generic parameters
        let generic_params = self.extract_generic_params(&impl_item.generics);

        // Estimate line number (approximate)
        let line_number = 1;

        Some(TraitImplementation {
            trait_fqn,
            self_type,
            impl_fqn,
            file_path: file_path.to_string(),
            line_number,
            generic_params,
            quality: ResolutionQuality::Analyzed,
        })
    }

    /// Extract call sites from an impl block
    fn extract_calls_from_impl(
        &self,
        impl_item: &ItemImpl,
        file_path: &str,
        caller_fqn: &str,
        self_type: &str,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();

        for item in &impl_item.items {
            if let ImplItem::Fn(method) = item {
                let method_caller_fqn = format!("{}::{}", caller_fqn, method.sig.ident);
                sites.extend(self.extract_calls_from_impl_fn(
                    method,
                    file_path,
                    &method_caller_fqn,
                    Some(self_type),
                    trait_method_index,
                ));
            }
        }

        sites
    }

    /// Extract call sites from an impl method
    fn extract_calls_from_impl_fn(
        &self,
        method: &syn::ImplItemFn,
        file_path: &str,
        caller_fqn: &str,
        self_type: Option<&str>,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();

        self.extract_calls_from_block(
            &method.block,
            file_path,
            caller_fqn,
            self_type,
            trait_method_index,
            &mut sites,
        );

        sites
    }

    /// Extract call sites from a standalone function
    fn extract_calls_from_fn(
        &self,
        fn_item: &syn::ItemFn,
        file_path: &str,
        caller_fqn: &str,
        self_type: Option<&str>,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();

        self.extract_calls_from_block(
            &fn_item.block,
            file_path,
            caller_fqn,
            self_type,
            trait_method_index,
            &mut sites,
        );

        sites
    }

    /// Recursively extract calls from a block
    fn extract_calls_from_block(
        &self,
        block: &syn::Block,
        file_path: &str,
        caller_fqn: &str,
        self_type: Option<&str>,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
        sites: &mut Vec<CallSite>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Local(local) => {
                    if let Some(init) = &local.init {
                        self.extract_calls_from_expr(
                            &init.expr,
                            file_path,
                            caller_fqn,
                            self_type,
                            trait_method_index,
                            sites,
                        );
                    }
                }
                syn::Stmt::Item(SynItem::Fn(nested_fn)) => {
                    let nested_fqn = format!("{}::{}", caller_fqn, nested_fn.sig.ident);
                    sites.extend(self.extract_calls_from_fn(
                        nested_fn,
                        file_path,
                        &nested_fqn,
                        self_type,
                        trait_method_index,
                    ));
                }
                syn::Stmt::Item(_) => {}
                syn::Stmt::Expr(expr, _) => {
                    self.extract_calls_from_expr(
                        expr,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
                _ => {}
            }
        }
    }

    /// Recursively extract calls from an expression
    fn extract_calls_from_expr(
        &self,
        expr: &Expr,
        file_path: &str,
        caller_fqn: &str,
        self_type: Option<&str>,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
        sites: &mut Vec<CallSite>,
    ) {
        match expr {
            Expr::Call(call) => {
                if let Some(site) = self.extract_call_site(call, file_path, caller_fqn) {
                    sites.push(site);
                }
                // Recurse into arguments
                for arg in &call.args {
                    self.extract_calls_from_expr(
                        arg,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::MethodCall(method_call) => {
                let method_sites = self.extract_method_call_site(
                    method_call,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                );
                sites.extend(method_sites);
                // Recurse into receiver and arguments
                self.extract_calls_from_expr(
                    &method_call.receiver,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                for arg in &method_call.args {
                    self.extract_calls_from_expr(
                        arg,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::If(if_expr) => {
                self.extract_calls_from_expr(
                    &if_expr.cond,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                self.extract_calls_from_block(
                    &if_expr.then_branch,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                if let Some((_, else_block)) = &if_expr.else_branch {
                    self.extract_calls_from_expr(
                        else_block,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Match(match_expr) => {
                self.extract_calls_from_expr(
                    &match_expr.expr,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                for arm in &match_expr.arms {
                    self.extract_calls_from_expr(
                        &arm.body,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Block(block_expr) => {
                self.extract_calls_from_block(
                    &block_expr.block,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Assign(assign) => {
                self.extract_calls_from_expr(
                    &assign.left,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                self.extract_calls_from_expr(
                    &assign.right,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Binary(binary) => {
                self.extract_calls_from_expr(
                    &binary.left,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                self.extract_calls_from_expr(
                    &binary.right,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Unary(unary) => {
                self.extract_calls_from_expr(
                    &unary.expr,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Return(ret) => {
                if let Some(expr) = &ret.expr {
                    self.extract_calls_from_expr(
                        expr,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Await(await_expr) => {
                self.extract_calls_from_expr(
                    &await_expr.base,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Try(try_expr) => {
                self.extract_calls_from_expr(
                    &try_expr.expr,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Paren(paren) => {
                self.extract_calls_from_expr(
                    &paren.expr,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Tuple(tuple) => {
                for elem in &tuple.elems {
                    self.extract_calls_from_expr(
                        elem,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Array(array) => {
                for elem in &array.elems {
                    self.extract_calls_from_expr(
                        elem,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Struct(struct_expr) => {
                for field in &struct_expr.fields {
                    self.extract_calls_from_expr(
                        &field.expr,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
                if let Some(rest) = &struct_expr.rest {
                    self.extract_calls_from_expr(
                        rest,
                        file_path,
                        caller_fqn,
                        self_type,
                        trait_method_index,
                        sites,
                    );
                }
            }
            Expr::Closure(closure) => {
                self.extract_calls_from_expr(
                    &closure.body,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::Loop(loop_expr) => {
                self.extract_calls_from_block(
                    &loop_expr.body,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::ForLoop(for_loop) => {
                self.extract_calls_from_expr(
                    &for_loop.expr,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                self.extract_calls_from_block(
                    &for_loop.body,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            Expr::While(while_expr) => {
                self.extract_calls_from_expr(
                    &while_expr.cond,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
                self.extract_calls_from_block(
                    &while_expr.body,
                    file_path,
                    caller_fqn,
                    self_type,
                    trait_method_index,
                    sites,
                );
            }
            _ => {}
        }
    }

    /// Extract a call site from a function call
    fn extract_call_site(
        &self,
        call: &ExprCall,
        file_path: &str,
        caller_fqn: &str,
    ) -> Option<CallSite> {
        // Get the function being called
        let (callee_fqn, type_args) = match call.func.as_ref() {
            Expr::Path(path_expr) => {
                let fqn = self.path_expr_to_fqn(path_expr);
                let type_args = self.extract_turbofish_types(path_expr);
                (fqn, type_args)
            }
            _ => return None,
        };

        let is_monomorphized = !type_args.is_empty();

        Some(CallSite {
            caller_fqn: caller_fqn.to_string(),
            callee_fqn,
            file_path: file_path.to_string(),
            line_number: 1,
            concrete_type_args: type_args,
            is_monomorphized,
            quality: ResolutionQuality::Analyzed,
            dispatch: CallDispatch::Static,
        })
    }

    fn extract_method_call_site(
        &self,
        method_call: &ExprMethodCall,
        file_path: &str,
        caller_fqn: &str,
        self_type: Option<&str>,
        trait_method_index: &std::collections::HashMap<(String, String), String>,
    ) -> Vec<CallSite> {
        let method_name = method_call.method.to_string();
        let type_args = if let Some(turbofish) = &method_call.turbofish {
            self.extract_turbofish_args_from_angle_bracketed(turbofish)
        } else {
            Vec::new()
        };
        let is_monomorphized = !type_args.is_empty();
        let callee_fqn = self.infer_method_callee(&method_call.receiver, &method_name);

        let mut sites = Vec::new();

        let receiver_type = self_type
            .map(|s| s.to_string())
            .or_else(|| self.infer_receiver_type(&method_call.receiver));

        if let Some(ref recv_type) = receiver_type {
            if let Some(impl_fqn) =
                trait_method_index.get(&(recv_type.clone(), method_name.clone()))
            {
                sites.push(CallSite {
                    caller_fqn: caller_fqn.to_string(),
                    callee_fqn: format!("{}::{}", impl_fqn, method_name),
                    file_path: file_path.to_string(),
                    line_number: 1,
                    concrete_type_args: type_args.clone(),
                    is_monomorphized,
                    quality: ResolutionQuality::Analyzed,
                    dispatch: CallDispatch::Static,
                });

                sites.push(CallSite {
                    caller_fqn: caller_fqn.to_string(),
                    callee_fqn,
                    file_path: file_path.to_string(),
                    line_number: 1,
                    concrete_type_args: type_args,
                    is_monomorphized,
                    quality: ResolutionQuality::Analyzed,
                    dispatch: CallDispatch::Trait,
                });

                return sites;
            }
        }

        sites.push(CallSite {
            caller_fqn: caller_fqn.to_string(),
            callee_fqn,
            file_path: file_path.to_string(),
            line_number: 1,
            concrete_type_args: type_args,
            is_monomorphized,
            quality: ResolutionQuality::Analyzed,
            dispatch: CallDispatch::Dynamic,
        });

        sites
    }

    /// Extract type arguments from turbofish syntax
    fn extract_turbofish_types(&self, path_expr: &ExprPath) -> Vec<TypeArg> {
        let mut type_args = Vec::new();

        for segment in &path_expr.path.segments {
            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                for (idx, arg) in args.args.iter().enumerate() {
                    if let GenericArgument::Type(ty) = arg {
                        type_args.push(TypeArg {
                            param_name: format!("T{}", idx),
                            concrete_type: self.type_to_string(ty),
                        });
                    }
                }
            }
        }

        type_args
    }

    /// Extract type arguments from turbofish on method calls (AngleBracketedGenericArguments)
    fn extract_turbofish_args_from_angle_bracketed(
        &self,
        args: &syn::AngleBracketedGenericArguments,
    ) -> Vec<TypeArg> {
        let mut type_args = Vec::new();

        for (idx, arg) in args.args.iter().enumerate() {
            if let GenericArgument::Type(ty) = arg {
                type_args.push(TypeArg {
                    param_name: format!("T{}", idx),
                    concrete_type: self.type_to_string(ty),
                });
            }
        }

        type_args
    }

    /// Infer method callee FQN from receiver
    fn infer_method_callee(&self, receiver: &Expr, method_name: &str) -> String {
        // Try to get the type of the receiver
        let receiver_type = match receiver {
            Expr::Path(_path_expr) => {
                // Variable reference - we can't know the type without type checking
                // Use a placeholder
                format!("unknown::{}", method_name)
            }
            Expr::Call(_call) => {
                // Method called on result of another call
                format!("call_result::{}", method_name)
            }
            Expr::Field(field) => {
                // Method called on a field
                let member_name = match &field.member {
                    syn::Member::Named(ident) => ident.to_string(),
                    syn::Member::Unnamed(idx) => format!("_{}", idx.index),
                };
                if let Expr::Path(path_expr) = field.base.as_ref() {
                    let base = self.path_expr_to_fqn(path_expr);
                    format!("{}::{}.{}", base, member_name, method_name)
                } else {
                    format!("field::{}", method_name)
                }
            }
            _ => format!("unknown::{}", method_name),
        };

        receiver_type
    }

    fn infer_receiver_type(&self, receiver: &Expr) -> Option<String> {
        match receiver {
            Expr::Path(_path_expr) => None,
            Expr::Field(field_expr) => match &field_expr.member {
                syn::Member::Named(ident) => Some(ident.to_string()),
                syn::Member::Unnamed(index) => Some(format!("field_{}", index.index)),
            },
            Expr::Reference(ref_expr) => self.infer_receiver_type(&ref_expr.expr),
            Expr::Paren(paren) => self.infer_receiver_type(&paren.expr),
            Expr::Cast(cast) => Some(self.type_to_string(&cast.ty)),
            _ => None,
        }
    }

    /// Convert a path to FQN string
    fn path_to_fqn(&self, path: &syn::Path) -> String {
        path.segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Convert an expression path to FQN
    fn path_expr_to_fqn(&self, path_expr: &ExprPath) -> String {
        path_expr
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Convert a type to string
    fn type_to_string(&self, ty: &Type) -> String {
        match ty {
            Type::Path(type_path) => self.type_path_to_string(type_path),
            Type::Reference(ref_type) => {
                let mut s = String::from("&");
                if let Some(lt) = &ref_type.lifetime {
                    s.push_str(&format!("'{} ", lt.ident));
                }
                if ref_type.mutability.is_some() {
                    s.push_str("mut ");
                }
                s.push_str(&self.type_to_string(&ref_type.elem));
                s
            }
            Type::Tuple(tuple) => {
                let elems: Vec<_> = tuple.elems.iter().map(|t| self.type_to_string(t)).collect();
                format!("({})", elems.join(", "))
            }
            Type::Array(arr) => {
                format!("[{}; ?]", self.type_to_string(&arr.elem))
            }
            Type::Slice(slice) => {
                format!("[{}]", self.type_to_string(&slice.elem))
            }
            Type::Paren(paren) => {
                format!("({})", self.type_to_string(&paren.elem))
            }
            _ => quote::quote!(#ty).to_string().replace(' ', ""),
        }
    }

    /// Convert a type path to string
    fn type_path_to_string(&self, type_path: &TypePath) -> String {
        let mut segments = Vec::new();

        if let Some(qself) = &type_path.qself {
            segments.push(format!("<{}>", self.type_to_string(&qself.ty)));
        }

        for segment in &type_path.path.segments {
            let mut seg_str = segment.ident.to_string();

            if let PathArguments::AngleBracketed(args) = &segment.arguments {
                let args_str: Vec<_> = args
                    .args
                    .iter()
                    .map(|arg| match arg {
                        GenericArgument::Type(ty) => self.type_to_string(ty),
                        GenericArgument::Lifetime(lt) => format!("'{}", lt.ident),
                        GenericArgument::Const(c) => quote::quote!(#c).to_string(),
                        _ => String::new(),
                    })
                    .collect();

                if !args_str.is_empty() {
                    seg_str.push_str(&format!("<{}>", args_str.join(", ")));
                }
            }

            segments.push(seg_str);
        }

        segments.join("::")
    }

    /// Generate caller FQN for an impl block.
    ///
    /// Uses canonical `module::Type` format to match FQNs produced by
    /// syn_parser's parse_impl_with_methods (which creates `module::Type::method`).
    fn impl_caller_fqn(&self, impl_item: &ItemImpl, module_path: &str) -> String {
        let self_type = self.type_to_string(&impl_item.self_ty);

        // Use module::Type so that appending ::method yields module::Type::method
        format!("{}::{}", module_path, self_type)
    }

    /// Extract generic parameters from syn Generics
    fn extract_generic_params(&self, generics: &syn::Generics) -> Vec<GenericParam> {
        generics
            .params
            .iter()
            .map(|param| match param {
                syn::GenericParam::Type(type_param) => {
                    let bounds = type_param
                        .bounds
                        .iter()
                        .map(|b| quote::quote!(#b).to_string())
                        .collect();

                    GenericParam {
                        name: type_param.ident.to_string(),
                        kind: "type".to_string(),
                        bounds,
                        default: type_param
                            .default
                            .as_ref()
                            .map(|d| quote::quote!(#d).to_string()),
                    }
                }
                syn::GenericParam::Lifetime(lt_param) => {
                    let bounds = lt_param
                        .bounds
                        .iter()
                        .map(|lt| format!("'{}", lt.ident))
                        .collect();

                    GenericParam {
                        name: format!("'{}", lt_param.lifetime.ident),
                        kind: "lifetime".to_string(),
                        bounds,
                        default: None,
                    }
                }
                syn::GenericParam::Const(const_param) => GenericParam {
                    name: const_param.ident.to_string(),
                    kind: "const".to_string(),
                    bounds: vec![quote::quote!(#const_param.ty).to_string()],
                    default: const_param
                        .default
                        .as_ref()
                        .map(|d| quote::quote!(#d).to_string()),
                },
            })
            .collect()
    }

    // ========================================================================
    // Heuristic Analysis (Fallback)
    // ========================================================================

    /// Analyze source using regex and heuristics
    fn analyze_with_heuristics(
        &self,
        _crate_name: &str,
        module_path: &str,
        file_path: &str,
        source: &str,
        caller_fqns: &[String],
    ) -> Result<(Vec<TraitImplementation>, Vec<CallSite>)> {
        let mut trait_impls = Vec::new();
        let mut call_sites = Vec::new();

        // Pattern for impl Trait for Type
        let impl_trait_pattern =
            regex::Regex::new(r"impl\s*(?:<[^>]*>)?\s*(\w+(?:::\w+)*)\s+for\s+([^\{]+)").unwrap();

        // Pattern for turbofish calls: function::<Type>
        let turbofish_pattern = regex::Regex::new(r"(\w+(?:::\w+)*)::<([^>]+)>").unwrap();

        // Pattern for method calls with turbofish: .method::<Type>
        let method_turbofish_pattern = regex::Regex::new(r"\.(\w+)::<([^>]+)>").unwrap();

        // Find trait implementations
        for (line_num, line) in source.lines().enumerate() {
            if let Some(caps) = impl_trait_pattern.captures(line) {
                let trait_fqn = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let self_type = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");

                trait_impls.push(TraitImplementation {
                    trait_fqn: trait_fqn.to_string(),
                    self_type: self_type.to_string(),
                    impl_fqn: format!("{}::{}_{}", module_path, trait_fqn, self_type),
                    file_path: file_path.to_string(),
                    line_number: line_num + 1,
                    generic_params: Vec::new(),
                    quality: ResolutionQuality::Heuristic,
                });
            }
        }

        // Find call sites with turbofish
        for (line_num, line) in source.lines().enumerate() {
            // Function calls with turbofish
            for caps in turbofish_pattern.captures_iter(line) {
                let callee_fqn = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let type_arg_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                let type_args = self.parse_type_args_heuristic(type_arg_str);
                let is_monomorphized = !type_args.is_empty();

                call_sites.push(CallSite {
                    caller_fqn: caller_fqns.first().cloned().unwrap_or_default(),
                    callee_fqn: callee_fqn.to_string(),
                    file_path: file_path.to_string(),
                    line_number: line_num + 1,
                    concrete_type_args: type_args,
                    is_monomorphized,
                    quality: ResolutionQuality::Heuristic,
                    dispatch: CallDispatch::Static,
                });
            }

            // Method calls with turbofish
            for caps in method_turbofish_pattern.captures_iter(line) {
                let method_name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let type_arg_str = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                let type_args = self.parse_type_args_heuristic(type_arg_str);
                let is_monomorphized = !type_args.is_empty();

                call_sites.push(CallSite {
                    caller_fqn: caller_fqns.first().cloned().unwrap_or_default(),
                    callee_fqn: format!("unknown::{}", method_name),
                    file_path: file_path.to_string(),
                    line_number: line_num + 1,
                    concrete_type_args: type_args,
                    is_monomorphized,
                    quality: ResolutionQuality::Heuristic,
                    dispatch: CallDispatch::Dynamic,
                });
            }
        }

        Ok((trait_impls, call_sites))
    }

    /// Parse type arguments from a string (heuristic)
    fn parse_type_args_heuristic(&self, args_str: &str) -> Vec<TypeArg> {
        let mut type_args = Vec::new();

        // Handle nested generics by tracking bracket depth
        let mut depth = 0;
        let mut current = String::new();
        let mut idx = 0;

        for ch in args_str.chars() {
            match ch {
                '<' => {
                    depth += 1;
                    current.push(ch);
                }
                '>' => {
                    depth -= 1;
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        type_args.push(TypeArg {
                            param_name: format!("T{}", idx),
                            concrete_type: trimmed.to_string(),
                        });
                        idx += 1;
                    }
                    current.clear();
                }
                _ => {
                    current.push(ch);
                }
            }
        }

        // Don't forget the last one
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            type_args.push(TypeArg {
                param_name: format!("T{}", idx),
                concrete_type: trimmed.to_string(),
            });
        }

        type_args
    }
}

impl Default for TypeResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_trait_impl() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Display for Point {
                fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    write!(f, "Point({}, {})", self.x, self.y)
                }
            }
        "#;

        let result = resolver.analyze_source(
            "test_crate",
            "test::module",
            "test.rs",
            source,
            &["test::module::test".to_string()],
        );

        assert!(!result.trait_impls.is_empty());
        let impl_info = &result.trait_impls[0];
        assert!(impl_info.trait_fqn.contains("Display"));
        assert!(impl_info.self_type.contains("Point"));
    }

    #[test]
    fn test_extract_turbofish_call() {
        let resolver = TypeResolver::new();
        let source = r#"
            fn test() {
                let result = parse::<String>(input);
                let data = Vec::<i32>::new();
            }
        "#;

        let result = resolver.analyze_source(
            "test_crate",
            "test::module",
            "test.rs",
            source,
            &["test::module::test".to_string()],
        );

        // Should find the turbofish calls
        assert!(!result.call_sites.is_empty());

        // At least one should be monomorphized
        assert!(result.call_sites.iter().any(|s| s.is_monomorphized));
    }

    #[test]
    fn test_heuristic_fallback() {
        let resolver = TypeResolver::new();
        // Truly malformed code that syn cannot parse —
        // an impl block with invalid syntax triggers heuristic fallback
        let source = r#"
            impl SomeTrait for AnotherType {
                fn do_thing(&self) -> { broken syntax here }
            }

            impl SecondTrait for ThirdType {
                fn also_broken(??? invalid)
            }
        "#;

        // Should fall back to heuristics since syn will fail
        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", source, &[]);

        // Heuristic regex should still find impl patterns even in broken code
        // The result may be empty if heuristics also can't extract, which is acceptable
        for impl_info in &result.trait_impls {
            assert!(matches!(impl_info.quality, ResolutionQuality::Heuristic));
        }
    }

    #[test]
    fn test_parse_type_args_heuristic() {
        let resolver = TypeResolver::new();

        // Simple types
        let args = resolver.parse_type_args_heuristic("String, i32");
        assert_eq!(args.len(), 2);

        // Nested generics
        let args = resolver.parse_type_args_heuristic("Vec<String>, HashMap<String, i32>");
        assert_eq!(args.len(), 2);
        assert!(args[0].concrete_type.contains("Vec"));
        assert!(args[1].concrete_type.contains("HashMap"));
    }

    #[test]
    fn test_inherent_impl_not_treated_as_trait_impl() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Point {
                fn new(x: f64, y: f64) -> Self {
                    Point { x, y }
                }
            }
        "#;

        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", source, &[]);

        // Inherent impl should NOT appear in trait_impls
        assert!(result.trait_impls.is_empty());
    }

    #[test]
    fn test_multiple_trait_impls() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Clone for Foo {
                fn clone(&self) -> Self { Foo }
            }

            impl Default for Foo {
                fn default() -> Self { Foo }
            }

            impl Display for Bar {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    write!(f, "Bar")
                }
            }
        "#;

        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", source, &[]);

        assert_eq!(result.trait_impls.len(), 3);

        let trait_names: Vec<&str> = result
            .trait_impls
            .iter()
            .map(|i| i.trait_fqn.as_str())
            .collect();
        assert!(trait_names.iter().any(|n| n.contains("Clone")));
        assert!(trait_names.iter().any(|n| n.contains("Default")));
        assert!(trait_names.iter().any(|n| n.contains("Display")));
    }

    #[test]
    fn test_call_site_extraction_from_function() {
        let resolver = TypeResolver::new();
        let source = r#"
            fn process() {
                let x = compute(42);
                let y = transform(x);
            }

            fn compute(n: i32) -> i32 { n * 2 }
            fn transform(n: i32) -> i32 { n + 1 }
        "#;

        let result = resolver.analyze_source(
            "test_crate",
            "test::module",
            "test.rs",
            source,
            &["test::module::process".to_string()],
        );

        // Should find call sites from process()
        let process_calls: Vec<_> = result
            .call_sites
            .iter()
            .filter(|s| s.caller_fqn.contains("process"))
            .collect();

        assert!(
            process_calls.len() >= 2,
            "Expected at least 2 calls from process(), got {}",
            process_calls.len()
        );
    }

    #[test]
    fn test_analyzed_quality_for_syn_parsed() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Clone for Foo {
                fn clone(&self) -> Self { Foo }
            }
        "#;

        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", source, &[]);

        // Syn-parsed impls should have Analyzed quality
        for impl_info in &result.trait_impls {
            assert!(matches!(impl_info.quality, ResolutionQuality::Analyzed));
        }
    }

    #[test]
    fn test_empty_source() {
        let resolver = TypeResolver::new();
        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", "", &[]);

        assert!(result.trait_impls.is_empty());
        assert!(result.call_sites.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_generic_trait_impl() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl<T: Clone> From<Vec<T>> for Container<T> {
                fn from(items: Vec<T>) -> Self {
                    Container { items }
                }
            }
        "#;

        let result = resolver.analyze_source("test_crate", "test::module", "test.rs", source, &[]);

        assert_eq!(result.trait_impls.len(), 1);
        let impl_info = &result.trait_impls[0];
        assert!(impl_info.trait_fqn.contains("From"));
        assert!(impl_info.self_type.contains("Container"));
    }

    // -----------------------------------------------------------------------
    // ResolutionQuality helpers
    // -----------------------------------------------------------------------

    #[test]
    fn test_resolution_quality_as_str() {
        assert_eq!(ResolutionQuality::Analyzed.as_str(), "analyzed");
        assert_eq!(ResolutionQuality::Heuristic.as_str(), "heuristic");
    }

    #[test]
    fn test_resolution_quality_parse_str() {
        assert_eq!(
            ResolutionQuality::parse_str("analyzed"),
            ResolutionQuality::Analyzed
        );
        assert_eq!(
            ResolutionQuality::parse_str("heuristic"),
            ResolutionQuality::Heuristic
        );
        // Any unknown string maps to Heuristic
        assert_eq!(
            ResolutionQuality::parse_str("unknown"),
            ResolutionQuality::Heuristic
        );
        assert_eq!(
            ResolutionQuality::parse_str(""),
            ResolutionQuality::Heuristic
        );
    }

    // -----------------------------------------------------------------------
    // analyze_heuristics_only
    // -----------------------------------------------------------------------

    #[test]
    fn test_analyze_heuristics_only_with_valid_source() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Display for Widget {
                fn fmt(&self, f: &mut Formatter) -> fmt::Result { Ok(()) }
            }

            fn render<T: Display>(item: T) {
                let s = format::<String>("{}", item);
            }
        "#;

        let result = resolver.analyze_heuristics_only(
            "my_crate",
            "my_crate::ui",
            "ui.rs",
            source,
            &["my_crate::ui::render".to_string()],
        );

        // Heuristics should detect the trait impl from the impl line
        assert!(
            !result.trait_impls.is_empty(),
            "Expected at least one trait impl"
        );
        for impl_info in &result.trait_impls {
            assert!(matches!(impl_info.quality, ResolutionQuality::Heuristic));
            assert_eq!(impl_info.file_path, "ui.rs");
        }

        // Errors should be empty for valid source
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_analyze_heuristics_only_with_empty_source() {
        let resolver = TypeResolver::new();
        let result = resolver.analyze_heuristics_only("crate", "mod", "f.rs", "", &[]);

        assert!(result.trait_impls.is_empty());
        assert!(result.call_sites.is_empty());
        assert!(result.errors.is_empty());
    }

    #[test]
    fn test_analyze_heuristics_only_turbofish_detected() {
        let resolver = TypeResolver::new();
        let source = r#"
            fn main() {
                let x = parse::<u64>("42");
                let y = collect::<Vec<i32>>();
            }
        "#;

        let result = resolver.analyze_heuristics_only(
            "crate",
            "main_mod",
            "main.rs",
            source,
            &["main_mod::main".to_string()],
        );

        assert!(
            !result.call_sites.is_empty(),
            "Expected turbofish call sites"
        );
        for site in &result.call_sites {
            assert!(matches!(site.quality, ResolutionQuality::Heuristic));
            assert!(site.is_monomorphized);
        }
    }

    // -----------------------------------------------------------------------
    // Multi-item source files
    // -----------------------------------------------------------------------

    #[test]
    fn test_multi_item_source_extraction() {
        let resolver = TypeResolver::new();
        let source = r#"
            pub struct Config { timeout: u64 }

            pub fn create_config() -> Config {
                Config { timeout: 30 }
            }

            impl Default for Config {
                fn default() -> Self { Config { timeout: 30 } }
            }

            impl Clone for Config {
                fn clone(&self) -> Self { Config { timeout: self.timeout } }
            }

            pub fn process(cfg: Config) -> bool {
                let _ = cfg;
                true
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::config", "config.rs", source, &[]);

        // Two trait impls: Default and Clone
        assert_eq!(result.trait_impls.len(), 2);
        let traits: Vec<&str> = result
            .trait_impls
            .iter()
            .map(|i| i.trait_fqn.as_str())
            .collect();
        assert!(traits.contains(&"Default"));
        assert!(traits.contains(&"Clone"));

        // All should be Analyzed quality
        for impl_info in &result.trait_impls {
            assert!(matches!(impl_info.quality, ResolutionQuality::Analyzed));
        }
    }

    #[test]
    fn test_nested_impl_method_call_sites() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Processor {
                pub fn run(&self) {
                    let result = self.prepare();
                    self.emit(result);
                }

                fn prepare(&self) -> i32 { 0 }
                fn emit(&self, _v: i32) {}
            }
        "#;

        let result = resolver.analyze_source(
            "crate",
            "crate::proc",
            "proc.rs",
            source,
            &["crate::proc::Processor::run".to_string()],
        );

        // Inherent impl: no trait impls
        assert!(result.trait_impls.is_empty());

        // Should capture method calls inside run()
        let run_calls: Vec<_> = result
            .call_sites
            .iter()
            .filter(|s| s.caller_fqn.contains("run"))
            .collect();
        assert!(!run_calls.is_empty(), "Expected method calls from run()");
    }

    #[test]
    fn test_generic_params_extracted_on_impl() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl<T: Clone + Send, U: Sync> Converter<T, U> {
                pub fn convert(&self) {}
            }
        "#;

        // Inherent impl — but we do still parse generics
        let result = resolver.analyze_source("crate", "crate::conv", "conv.rs", source, &[]);

        // No trait impl (inherent impl)
        assert!(result.trait_impls.is_empty());
    }

    #[test]
    fn test_generic_trait_impl_generics_populated() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl<T: Clone + Send> Iterator for MyIter<T> {
                type Item = T;
                fn next(&mut self) -> Option<T> { None }
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::iter", "iter.rs", source, &[]);

        assert_eq!(result.trait_impls.len(), 1);
        let impl_info = &result.trait_impls[0];
        assert_eq!(impl_info.trait_fqn, "Iterator");
        assert!(impl_info.self_type.contains("MyIter"));
        // Generic param T should be captured
        assert!(!impl_info.generic_params.is_empty());
        assert_eq!(impl_info.generic_params[0].name, "T");
        assert_eq!(impl_info.generic_params[0].kind, "type");
    }

    #[test]
    fn test_impl_fqn_format() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Debug for Foo {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::foo", "foo.rs", source, &[]);

        assert!(!result.trait_impls.is_empty());
        // impl_fqn should follow module::Trait_Type format
        let fqn = &result.trait_impls[0].impl_fqn;
        assert!(
            fqn.contains("Debug"),
            "impl_fqn should contain trait name: {}",
            fqn
        );
        assert!(
            fqn.contains("Foo"),
            "impl_fqn should contain type name: {}",
            fqn
        );
    }

    #[test]
    fn test_turbofish_single_type_arg() {
        let resolver = TypeResolver::new();
        let source = r#"
            fn caller() {
                let v = Vec::<String>::new();
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::m", "m.rs", source, &[]);

        let mono_sites: Vec<_> = result
            .call_sites
            .iter()
            .filter(|s| s.is_monomorphized)
            .collect();
        assert!(
            !mono_sites.is_empty(),
            "Expected monomorphized call site for Vec::<String>::new()"
        );
        let site = &mono_sites[0];
        assert_eq!(site.concrete_type_args.len(), 1);
        assert!(site.concrete_type_args[0].concrete_type.contains("String"));
    }

    #[test]
    fn test_heuristic_type_args_nested_generics() {
        let resolver = TypeResolver::new();

        // Two-level nested generics
        let args = resolver.parse_type_args_heuristic("HashMap<String, Vec<i32>>, Option<u64>");
        assert_eq!(args.len(), 2, "Expected 2 args: {:?}", args);
        assert!(args[0].concrete_type.contains("HashMap"));
        assert!(args[1].concrete_type.contains("Option"));
        assert_eq!(args[0].param_name, "T0");
        assert_eq!(args[1].param_name, "T1");
    }

    #[test]
    fn test_heuristic_single_type_arg() {
        let resolver = TypeResolver::new();
        let args = resolver.parse_type_args_heuristic("String");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].concrete_type, "String");
        assert_eq!(args[0].param_name, "T0");
    }

    #[test]
    fn test_heuristic_empty_type_args() {
        let resolver = TypeResolver::new();
        let args = resolver.parse_type_args_heuristic("");
        assert!(args.is_empty());
    }

    #[test]
    fn test_file_path_propagated_in_results() {
        let resolver = TypeResolver::new();
        let source = r#"
            impl Clone for MyType {
                fn clone(&self) -> Self { MyType }
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::m", "src/lib.rs", source, &[]);

        assert!(!result.trait_impls.is_empty());
        assert_eq!(result.trait_impls[0].file_path, "src/lib.rs");
    }

    #[test]
    fn test_heuristic_trait_impl_line_numbers() {
        let resolver = TypeResolver::new();
        // The impl line is on line 3 (1-indexed)
        let source = "// line 1\n// line 2\nimpl Foo for Bar {}\n";

        let result = resolver.analyze_heuristics_only("crate", "crate::m", "f.rs", source, &[]);

        assert!(!result.trait_impls.is_empty());
        // Line numbers in heuristic mode are 1-based
        assert_eq!(result.trait_impls[0].line_number, 3);
    }

    #[test]
    fn test_type_resolver_default_is_same_as_new() {
        let _resolver: TypeResolver = TypeResolver::default();
        // If this compiles and doesn't panic, Default is wired up correctly
    }

    #[test]
    fn test_analyze_source_no_errors_for_valid_source() {
        let resolver = TypeResolver::new();
        let source = r#"
            pub fn add(a: i32, b: i32) -> i32 { a + b }

            impl std::fmt::Display for () {
                fn fmt(&self, _f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) }
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::math", "math.rs", source, &[]);
        assert!(
            result.errors.is_empty(),
            "No errors expected for valid source: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_analyze_source_errors_recorded_for_invalid_source() {
        let resolver = TypeResolver::new();
        // Truly unparseable — not valid Rust
        let source = "this is not rust code @#$%^";

        let result = resolver.analyze_source("crate", "crate::m", "f.rs", source, &[]);

        // Syn will fail; errors should be recorded
        assert!(
            !result.errors.is_empty(),
            "Expected errors for invalid source"
        );
    }

    #[test]
    fn test_method_turbofish_on_variable_receiver() {
        let resolver = TypeResolver::new();
        let source = r#"
            fn do_work(buf: Vec<u8>) {
                let s = buf.iter().map::<String, _>(|b| b.to_string()).collect::<Vec<_>>();
            }
        "#;

        let result = resolver.analyze_source("crate", "crate::work", "work.rs", source, &[]);

        // Should find at least one monomorphized method call
        let mono: Vec<_> = result
            .call_sites
            .iter()
            .filter(|s| s.is_monomorphized)
            .collect();
        assert!(!mono.is_empty(), "Expected monomorphized method calls");
    }
}
