//! Type Resolution Module for rust-brain
//!
//! This module provides type resolution capabilities without attempting full monomorphization.
//! Instead, it:
//! - Stores generic functions as-is with constraints
//! - Indexes every concrete call site
//! - Maps trait implementations (impl Trait for Type)
//! - Enables queries like "show me parse for String"
//!
//! Strategy from ORCHESTRATOR_PROMPT:
//! - DO NOT attempt full monomorphization
//! - Store generic functions as-is with constraints
//! - Index every concrete call site
//! - On query "show me parse for String": lookup call_sites with matching type args

mod resolver;

pub use resolver::{CallSite, ResolutionQuality, TraitImplementation, TypeArg, TypeResolver};

use anyhow::Result;
use sqlx::{PgPool, Row};

/// Result of type resolution analysis
#[derive(Debug, Clone)]
pub struct TypeResolutionResult {
    /// All discovered trait implementations
    pub trait_impls: Vec<TraitImplementation>,

    /// All discovered call sites with concrete type info
    pub call_sites: Vec<CallSite>,

    /// Any errors encountered during resolution
    pub errors: Vec<String>,
}

/// Statistics about type resolution
#[derive(Debug, Default)]
pub struct ResolutionStats {
    pub trait_impls_found: usize,
    pub call_sites_found: usize,
    pub monomorphized_calls: usize,
    pub heuristic_resolutions: usize,
    pub analyzed_resolutions: usize,
}

/// Main entry point for type resolution
pub struct TypeResolutionService {
    pool: PgPool,
    resolver: TypeResolver,
}

impl TypeResolutionService {
    /// Create a new type resolution service
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            resolver: TypeResolver::new(),
        }
    }

    /// Analyze expanded source code for type information
    ///
    /// This method:
    /// 1. Parses the expanded source for impl blocks
    /// 2. Extracts trait implementations (impl Trait for Type)
    /// 3. Finds call sites with concrete type arguments
    /// 4. Stores results in the database
    pub async fn analyze_expanded_source(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        expanded_source: &str,
        caller_fqns: &[String],
    ) -> Result<TypeResolutionResult> {
        // Use the resolver to analyze the source
        let result = self.resolver.analyze_source(
            crate_name,
            module_path,
            file_path,
            expanded_source,
            caller_fqns,
        );

        // Store results in database
        self.store_trait_implementations(&result.trait_impls)
            .await?;
        self.store_call_sites(&result.call_sites).await?;

        Ok(result)
    }

    /// Analyze expanded source using heuristics only (for large files)
    ///
    /// This skips syn-based parsing and uses regex heuristics directly.
    /// Use for files > 10MB where syn would be too slow.
    pub async fn analyze_with_heuristics(
        &self,
        crate_name: &str,
        module_path: &str,
        file_path: &str,
        expanded_source: &str,
        caller_fqns: &[String],
    ) -> Result<TypeResolutionResult> {
        // Use the resolver's heuristic analysis directly
        let result = self.resolver.analyze_heuristics_only(
            crate_name,
            module_path,
            file_path,
            expanded_source,
            caller_fqns,
        );

        // Store results in database
        self.store_trait_implementations(&result.trait_impls)
            .await?;
        self.store_call_sites(&result.call_sites).await?;

        Ok(result)
    }

    /// Store trait implementations in the database
    async fn store_trait_implementations(&self, impls: &[TraitImplementation]) -> Result<()> {
        for impl_info in impls {
            let generic_params_json = serde_json::to_value(&impl_info.generic_params)?;

            sqlx::query(
                r#"
                INSERT INTO trait_implementations 
                    (trait_fqn, self_type, impl_fqn, file_path, line_number, generic_params, quality)
                VALUES 
                    ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (impl_fqn) DO UPDATE SET
                    trait_fqn = EXCLUDED.trait_fqn,
                    self_type = EXCLUDED.self_type,
                    file_path = EXCLUDED.file_path,
                    line_number = EXCLUDED.line_number,
                    generic_params = EXCLUDED.generic_params,
                    quality = EXCLUDED.quality
                "#
            )
            .bind(&impl_info.trait_fqn)
            .bind(&impl_info.self_type)
            .bind(&impl_info.impl_fqn)
            .bind(&impl_info.file_path)
            .bind(impl_info.line_number as i32)
            .bind(&generic_params_json)
            .bind(impl_info.quality.as_str())
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Store call sites in the database
    async fn store_call_sites(&self, sites: &[CallSite]) -> Result<()> {
        for site in sites {
            let type_args_json = serde_json::to_value(&site.concrete_type_args)?;

            sqlx::query(
                r#"
                INSERT INTO call_sites 
                    (caller_fqn, callee_fqn, file_path, line_number, concrete_type_args, is_monomorphized, quality)
                VALUES 
                    ($1, $2, $3, $4, $5, $6, $7)
                "#
            )
            .bind(&site.caller_fqn)
            .bind(&site.callee_fqn)
            .bind(&site.file_path)
            .bind(site.line_number as i32)
            .bind(&type_args_json)
            .bind(site.is_monomorphized)
            .bind(site.quality.as_str())
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    /// Query for functions with specific type arguments
    ///
    /// Example: "show me parse for String"
    /// This queries call_sites where concrete_type_args contains "String"
    pub async fn find_calls_with_type_arg(
        &self,
        callee_name: &str,
        type_arg: &str,
    ) -> Result<Vec<CallSite>> {
        let pattern = format!("%{}%", type_arg);
        let callee_pattern = format!("%::{}", callee_name);

        let rows = sqlx::query(
            r#"
            SELECT 
                caller_fqn,
                callee_fqn,
                file_path,
                line_number,
                concrete_type_args,
                is_monomorphized,
                quality
            FROM call_sites
            WHERE callee_fqn LIKE $1
              AND concrete_type_args::text LIKE $2
            ORDER BY line_number
            "#,
        )
        .bind(&callee_pattern)
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await?;

        let mut sites = Vec::new();
        for row in rows {
            let caller_fqn: String = row.get("caller_fqn");
            let callee_fqn: String = row.get("callee_fqn");
            let file_path: String = row.get("file_path");
            let line_number: i32 = row.get("line_number");
            let concrete_type_args_json: Option<serde_json::Value> = row.get("concrete_type_args");
            let is_monomorphized: bool = row.get("is_monomorphized");
            let quality_str: Option<String> = row.get("quality");

            let type_args: Vec<TypeArg> = concrete_type_args_json
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default();

            sites.push(CallSite {
                caller_fqn,
                callee_fqn,
                file_path,
                line_number: line_number as usize,
                concrete_type_args: type_args,
                is_monomorphized,
                quality: ResolutionQuality::parse_str(quality_str.as_deref().unwrap_or_default()),
            });
        }

        Ok(sites)
    }

    /// Get all trait implementations for a type
    pub async fn find_impls_for_type(&self, type_name: &str) -> Result<Vec<TraitImplementation>> {
        let pattern = format!("%{}%", type_name);

        let rows = sqlx::query(
            r#"
            SELECT 
                trait_fqn,
                self_type,
                impl_fqn,
                file_path,
                line_number,
                generic_params,
                quality
            FROM trait_implementations
            WHERE self_type LIKE $1
            ORDER BY trait_fqn
            "#,
        )
        .bind(&pattern)
        .fetch_all(&self.pool)
        .await?;

        let mut impls = Vec::new();
        for row in rows {
            let trait_fqn: String = row.get("trait_fqn");
            let self_type: String = row.get("self_type");
            let impl_fqn: String = row.get("impl_fqn");
            let file_path: String = row.get("file_path");
            let line_number: i32 = row.get("line_number");
            let generic_params_json: Option<serde_json::Value> = row.get("generic_params");
            let quality_str: Option<String> = row.get("quality");

            let generic_params: Vec<crate::parsers::GenericParam> = generic_params_json
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default();

            impls.push(TraitImplementation {
                trait_fqn,
                self_type,
                impl_fqn,
                file_path,
                line_number: line_number as usize,
                generic_params,
                quality: ResolutionQuality::parse_str(quality_str.as_deref().unwrap_or_default()),
            });
        }

        Ok(impls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolution_quality_from_str() {
        assert!(matches!(
            ResolutionQuality::parse_str("analyzed"),
            ResolutionQuality::Analyzed
        ));
        assert!(matches!(
            ResolutionQuality::parse_str("heuristic"),
            ResolutionQuality::Heuristic
        ));
        assert!(matches!(
            ResolutionQuality::parse_str("unknown"),
            ResolutionQuality::Heuristic
        ));
    }
}
