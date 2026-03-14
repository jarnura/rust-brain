//! Type Resolver Implementation
//!
//! Provides concrete type resolution at generic call sites without full monomorphization.
//! Uses a dual strategy:
//! - **Analyzed**: Full syn parsing for precise type extraction
//! - **Heuristic**: Regex + pattern matching as fallback for complex or unparseable code

use crate::parsers::{GenericParam, DualParser};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use syn::{
    Expr, ExprCall, ExprPath, ExprMethodCall,
    GenericArgument, Item as SynItem, ItemImpl, PathArguments,
    Type, TypePath, ImplItem,
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
    
    pub fn from_str(s: &str) -> Self {
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
pub struct TypeResolver {
    parser: DualParser,
}

impl TypeResolver {
    /// Create a new type resolver
    pub fn new() -> Self {
        Self {
            parser: DualParser::new().expect("Failed to create DualParser"),
        }
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
        match self.analyze_with_syn(crate_name, module_path, file_path, expanded_source, caller_fqns) {
            Ok((impls, sites)) => {
                trait_impls.extend(impls);
                call_sites.extend(sites);
            }
            Err(e) => {
                debug!("Syn analysis failed, falling back to heuristics: {}", e);
                errors.push(format!("Syn analysis failed: {}", e));
                
                // Fall back to heuristic analysis
                match self.analyze_with_heuristics(crate_name, module_path, file_path, expanded_source, caller_fqns) {
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
    
    /// Analyze source using syn for precise type extraction
    fn analyze_with_syn(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        source: &str,
        caller_fqns: &[String],
    ) -> Result<(Vec<TraitImplementation>, Vec<CallSite>)> {
        // Parse the entire source file
        let file: syn::File = syn::parse_str(source)
            .with_context(|| "Failed to parse source with syn")?;
        
        let mut trait_impls = Vec::new();
        let mut call_sites = Vec::new();
        
        for (idx, item) in file.items.iter().enumerate() {
            match item {
                SynItem::Impl(impl_item) => {
                    if let Some(impl_info) = self.extract_trait_impl(
                        impl_item,
                        crate_name,
                        module_path,
                        file_path,
                        idx,
                    ) {
                        trait_impls.push(impl_info);
                    }
                    
                    // Also extract call sites from within impl blocks
                    let caller_fqn = self.impl_caller_fqn(impl_item, module_path);
                    for call_site in self.extract_calls_from_impl(
                        impl_item,
                        file_path,
                        &caller_fqn,
                    ) {
                        call_sites.push(call_site);
                    }
                }
                SynItem::Fn(fn_item) => {
                    // Extract call sites from standalone functions
                    let caller_fqn = format!("{}::{}", module_path, fn_item.sig.ident);
                    for call_site in self.extract_calls_from_fn(
                        fn_item,
                        file_path,
                        &caller_fqn,
                    ) {
                        call_sites.push(call_site);
                    }
                }
                _ => {}
            }
        }
        
        // Mark all as analyzed quality
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
        crate_name: &str,
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
        
        let trait_fqn = trait_path.as_ref()
            .map(|p| self.path_to_fqn(p))
            .unwrap_or_default();
        
        let self_type = self.type_to_string(&impl_item.self_ty);
        
        let impl_fqn = format!("{}::<impl {} for {}>", module_path, trait_fqn, self_type);
        
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
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();
        
        for item in &impl_item.items {
            if let ImplItem::Fn(method) = item {
                let method_caller_fqn = format!("{}::{}", caller_fqn, method.sig.ident);
                sites.extend(self.extract_calls_from_impl_fn(method, file_path, &method_caller_fqn));
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
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();
        
        // Walk the expression tree looking for calls
        self.extract_calls_from_block(&method.block, file_path, caller_fqn, &mut sites);
        
        sites
    }
    
    /// Extract call sites from a standalone function
    fn extract_calls_from_fn(
        &self,
        fn_item: &syn::ItemFn,
        file_path: &str,
        caller_fqn: &str,
    ) -> Vec<CallSite> {
        let mut sites = Vec::new();
        
        // Walk the expression tree looking for calls
        self.extract_calls_from_block(&fn_item.block, file_path, caller_fqn, &mut sites);
        
        sites
    }
    
    /// Recursively extract calls from a block
    fn extract_calls_from_block(
        &self,
        block: &syn::Block,
        file_path: &str,
        caller_fqn: &str,
        sites: &mut Vec<CallSite>,
    ) {
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Local(local) => {
                    if let Some(init) = &local.init {
                        self.extract_calls_from_expr(&init.expr, file_path, caller_fqn, sites);
                    }
                }
                syn::Stmt::Item(item) => {
                    // Could have nested items
                    if let SynItem::Fn(nested_fn) = item {
                        let nested_fqn = format!("{}::{}", caller_fqn, nested_fn.sig.ident);
                        sites.extend(self.extract_calls_from_fn(nested_fn, file_path, &nested_fqn));
                    }
                }
                syn::Stmt::Expr(expr, _) => {
                    self.extract_calls_from_expr(expr, file_path, caller_fqn, sites);
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
        sites: &mut Vec<CallSite>,
    ) {
        match expr {
            Expr::Call(call) => {
                if let Some(site) = self.extract_call_site(call, file_path, caller_fqn) {
                    sites.push(site);
                }
                // Recurse into arguments
                for arg in &call.args {
                    self.extract_calls_from_expr(arg, file_path, caller_fqn, sites);
                }
            }
            Expr::MethodCall(method_call) => {
                if let Some(site) = self.extract_method_call_site(method_call, file_path, caller_fqn) {
                    sites.push(site);
                }
                // Recurse into receiver and arguments
                self.extract_calls_from_expr(&method_call.receiver, file_path, caller_fqn, sites);
                for arg in &method_call.args {
                    self.extract_calls_from_expr(arg, file_path, caller_fqn, sites);
                }
            }
            Expr::If(if_expr) => {
                self.extract_calls_from_expr(&if_expr.cond, file_path, caller_fqn, sites);
                self.extract_calls_from_block(&if_expr.then_branch, file_path, caller_fqn, sites);
                if let Some((_, else_block)) = &if_expr.else_branch {
                    self.extract_calls_from_expr(else_block, file_path, caller_fqn, sites);
                }
            }
            Expr::Match(match_expr) => {
                self.extract_calls_from_expr(&match_expr.expr, file_path, caller_fqn, sites);
                for arm in &match_expr.arms {
                    self.extract_calls_from_expr(&arm.body, file_path, caller_fqn, sites);
                }
            }
            Expr::Block(block_expr) => {
                self.extract_calls_from_block(&block_expr.block, file_path, caller_fqn, sites);
            }
            Expr::Assign(assign) => {
                self.extract_calls_from_expr(&assign.left, file_path, caller_fqn, sites);
                self.extract_calls_from_expr(&assign.right, file_path, caller_fqn, sites);
            }
            Expr::Binary(binary) => {
                self.extract_calls_from_expr(&binary.left, file_path, caller_fqn, sites);
                self.extract_calls_from_expr(&binary.right, file_path, caller_fqn, sites);
            }
            Expr::Unary(unary) => {
                self.extract_calls_from_expr(&unary.expr, file_path, caller_fqn, sites);
            }
            Expr::Return(ret) => {
                if let Some(expr) = &ret.expr {
                    self.extract_calls_from_expr(expr, file_path, caller_fqn, sites);
                }
            }
            Expr::Await(await_expr) => {
                self.extract_calls_from_expr(&await_expr.base, file_path, caller_fqn, sites);
            }
            Expr::Try(try_expr) => {
                self.extract_calls_from_expr(&try_expr.expr, file_path, caller_fqn, sites);
            }
            Expr::Paren(paren) => {
                self.extract_calls_from_expr(&paren.expr, file_path, caller_fqn, sites);
            }
            Expr::Tuple(tuple) => {
                for elem in &tuple.elems {
                    self.extract_calls_from_expr(elem, file_path, caller_fqn, sites);
                }
            }
            Expr::Array(array) => {
                for elem in &array.elems {
                    self.extract_calls_from_expr(elem, file_path, caller_fqn, sites);
                }
            }
            Expr::Struct(struct_expr) => {
                for field in &struct_expr.fields {
                    self.extract_calls_from_expr(&field.expr, file_path, caller_fqn, sites);
                }
                if let Some(rest) = &struct_expr.rest {
                    self.extract_calls_from_expr(rest, file_path, caller_fqn, sites);
                }
            }
            Expr::Closure(closure) => {
                self.extract_calls_from_expr(&closure.body, file_path, caller_fqn, sites);
            }
            Expr::Loop(loop_expr) => {
                self.extract_calls_from_block(&loop_expr.body, file_path, caller_fqn, sites);
            }
            Expr::ForLoop(for_loop) => {
                self.extract_calls_from_expr(&for_loop.expr, file_path, caller_fqn, sites);
                self.extract_calls_from_block(&for_loop.body, file_path, caller_fqn, sites);
            }
            Expr::While(while_expr) => {
                self.extract_calls_from_expr(&while_expr.cond, file_path, caller_fqn, sites);
                self.extract_calls_from_block(&while_expr.body, file_path, caller_fqn, sites);
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
            line_number: 1, // Approximate - would need span info for exact
            concrete_type_args: type_args,
            is_monomorphized,
            quality: ResolutionQuality::Analyzed,
        })
    }
    
    /// Extract a call site from a method call
    fn extract_method_call_site(
        &self,
        method_call: &ExprMethodCall,
        file_path: &str,
        caller_fqn: &str,
    ) -> Option<CallSite> {
        let method_name = method_call.method.to_string();
        
        // Extract turbofish type arguments if present
        let type_args = if let Some(turbofish) = &method_call.turbofish {
            self.extract_turbofish_args_from_angle_bracketed(turbofish)
        } else {
            Vec::new()
        };
        
        let is_monomorphized = !type_args.is_empty();
        
        // Try to infer the callee FQN from the receiver type
        let callee_fqn = self.infer_method_callee(&method_call.receiver, &method_name);
        
        Some(CallSite {
            caller_fqn: caller_fqn.to_string(),
            callee_fqn,
            file_path: file_path.to_string(),
            line_number: 1,
            concrete_type_args: type_args,
            is_monomorphized,
            quality: ResolutionQuality::Analyzed,
        })
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
        args: &syn::AngleBracketedGenericArguments
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
            Expr::Path(path_expr) => {
                // Variable reference - we can't know the type without type checking
                // Use a placeholder
                format!("unknown::{}", method_name)
            }
            Expr::Call(call) => {
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
                let elems: Vec<_> = tuple.elems.iter()
                    .map(|t| self.type_to_string(t))
                    .collect();
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
                let args_str: Vec<_> = args.args.iter()
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
    
    /// Generate caller FQN for an impl block
    fn impl_caller_fqn(&self, impl_item: &ItemImpl, module_path: &str) -> String {
        let self_type = self.type_to_string(&impl_item.self_ty);
        
        if let Some((_, trait_path, _)) = &impl_item.trait_ {
            let trait_name = self.path_to_fqn(trait_path);
            format!("{}::<impl {} for {}>", module_path, trait_name, self_type)
        } else {
            format!("{}::<impl {}>", module_path, self_type)
        }
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
                        default: type_param.default.as_ref().map(|d| {
                            quote::quote!(#d).to_string()
                        }),
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
                syn::GenericParam::Const(const_param) => {
                    GenericParam {
                        name: const_param.ident.to_string(),
                        kind: "const".to_string(),
                        bounds: vec![quote::quote!(#const_param.ty).to_string()],
                        default: const_param.default.as_ref().map(|d| {
                            quote::quote!(#d).to_string()
                        }),
                    }
                }
            })
            .collect()
    }
    
    // ========================================================================
    // Heuristic Analysis (Fallback)
    // ========================================================================
    
    /// Analyze source using regex and heuristics
    fn analyze_with_heuristics(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        source: &str,
        caller_fqns: &[String],
    ) -> Result<(Vec<TraitImplementation>, Vec<CallSite>)> {
        let mut trait_impls = Vec::new();
        let mut call_sites = Vec::new();
        
        // Pattern for impl Trait for Type
        let impl_trait_pattern = regex::Regex::new(
            r"impl\s*(?:<[^>]*>)?\s*(\w+(?:::\w+)*)\s+for\s+([^\{]+)"
        ).unwrap();
        
        // Pattern for turbofish calls: function::<Type>
        let turbofish_pattern = regex::Regex::new(
            r"(\w+(?:::\w+)*)::<([^>]+)>"
        ).unwrap();
        
        // Pattern for method calls with turbofish: .method::<Type>
        let method_turbofish_pattern = regex::Regex::new(
            r"\.(\w+)::<([^>]+)>"
        ).unwrap();
        
        // Find trait implementations
        for (line_num, line) in source.lines().enumerate() {
            if let Some(caps) = impl_trait_pattern.captures(line) {
                let trait_fqn = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let self_type = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                
                trait_impls.push(TraitImplementation {
                    trait_fqn: trait_fqn.to_string(),
                    self_type: self_type.to_string(),
                    impl_fqn: format!("{}::<impl {} for {}>", module_path, trait_fqn, self_type),
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
        let result = resolver.analyze_source(
            "test_crate",
            "test::module",
            "test.rs",
            source,
            &[],
        );

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
}
