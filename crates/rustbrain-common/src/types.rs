//! Core type definitions shared across rust-brain services.
//!
//! These types represent code elements extracted during ingestion
//! and queried through the API. Key types include:
//!
//! - [`ItemType`] — classifies extracted code items
//! - [`Visibility`] — Rust visibility levels
//! - [`GenericParam`] / [`WhereClause`] — generic parameter metadata
//! - [`ResolutionQuality`] — confidence level of type analysis
//! - [`StoreReference`] — cross-store consistency tracking

use serde::{Deserialize, Serialize};
use tracing::{debug, trace, warn};

/// A generic parameter extracted from a Rust item signature.
///
/// Represents type parameters (`T`), lifetime parameters (`'a`), and
/// const generic parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenericParam {
    /// Parameter name (e.g., "T", "'a")
    pub name: String,
    /// Parameter type: "type", "lifetime", or "const"
    pub kind: String,
    /// Type bounds (e.g., ["Clone", "Send"])
    pub bounds: Vec<String>,
    /// Default value if any (e.g., "String" for <T = String>)
    pub default: Option<String>,
}

/// A single predicate from a `where` clause (e.g., `T: Clone + Send`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhereClause {
    /// The type being constrained (e.g., "T", "Self::Item")
    pub subject: String,
    /// The bounds applied (e.g., ["Clone", "Send"])
    pub bounds: Vec<String>,
}

/// Rust visibility level for a code item.
///
/// Maps to the `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`, and
/// private (no modifier) visibility specifiers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Visibility {
    /// pub
    Public,
    /// pub(crate)
    PubCrate,
    /// pub(super)
    PubSuper,
    /// pub(in path::to::module)
    PubIn(String),
    /// No visibility modifier (private)
    Private,
}

impl Visibility {
    /// Returns a short string key for this visibility level.
    ///
    /// Used for database storage and serialization. Returns `"pub"`,
    /// `"pub_crate"`, `"pub_super"`, the path for `PubIn`, or `"private"`.
    pub fn as_str(&self) -> &str {
        trace!("Visibility::as_str");
        match self {
            Visibility::Public => "pub",
            Visibility::PubCrate => "pub_crate",
            Visibility::PubSuper => "pub_super",
            Visibility::PubIn(path) => path.as_str(),
            Visibility::Private => "private",
        }
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Visibility::Public => write!(f, "pub"),
            Visibility::PubCrate => write!(f, "pub(crate)"),
            Visibility::PubSuper => write!(f, "pub(super)"),
            Visibility::PubIn(path) => write!(f, "pub(in {})", path),
            Visibility::Private => write!(f, ""),
        }
    }
}

/// Classification of a Rust code item.
///
/// Serialized as snake_case strings (e.g., `"function"`, `"type_alias"`).
/// The [`Unknown`](ItemType::Unknown) variant captures item kinds not yet
/// handled by the parser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    /// `fn` or method
    Function,
    /// `struct`
    Struct,
    /// `enum`
    Enum,
    /// `trait`
    Trait,
    /// `impl` block (inherent or trait)
    Impl,
    /// `type` alias
    TypeAlias,
    /// `const` item
    Const,
    /// `static` item
    Static,
    /// `macro_rules!` or proc-macro
    Macro,
    /// `mod` declaration
    Module,
    /// `use` import
    Use,
    /// `extern` block
    ExternBlock,
    /// Unrecognized item kind (stores the raw kind string)
    Unknown(String),
}

impl ItemType {
    /// Returns the snake_case string representation used in the database
    /// and JSON serialization.
    pub fn as_str(&self) -> &str {
        trace!("ItemType::as_str");
        match self {
            ItemType::Function => "function",
            ItemType::Struct => "struct",
            ItemType::Enum => "enum",
            ItemType::Trait => "trait",
            ItemType::Impl => "impl",
            ItemType::TypeAlias => "type_alias",
            ItemType::Const => "const",
            ItemType::Static => "static",
            ItemType::Macro => "macro",
            ItemType::Module => "module",
            ItemType::Use => "use",
            ItemType::ExternBlock => "extern_block",
            ItemType::Unknown(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for ItemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Confidence indicator for type resolution results.
///
/// Attached to call-site and trait-implementation records to signal
/// how the type information was obtained.
///
/// Implements [`FromStr`](std::str::FromStr) for parsing from database strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionQuality {
    /// Parsed with `syn` — high confidence
    Analyzed,
    /// Extracted with regex heuristics — lower confidence
    Heuristic,
    /// Quality could not be determined
    Unknown,
}

impl std::fmt::Display for ResolutionQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolutionQuality::Analyzed => write!(f, "analyzed"),
            ResolutionQuality::Heuristic => write!(f, "heuristic"),
            ResolutionQuality::Unknown => write!(f, "unknown"),
        }
    }
}

/// Reference tracking entry for cross-store consistency.
///
/// Maps an item's FQN to its identifiers in each storage backend,
/// enabling cascade deletion and garbage collection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoreReference {
    /// Fully qualified name (canonical identifier)
    pub fqn: String,
    /// Postgres extracted_items.id (if stored)
    pub postgres_id: Option<String>,
    /// Neo4j node element ID (if stored)
    pub neo4j_node_id: Option<String>,
    /// Qdrant point ID (if stored)
    pub qdrant_point_id: Option<String>,
    /// Crate name for scoped operations
    pub crate_name: String,
    /// Timestamp of last successful sync
    pub last_synced: Option<String>,
}

impl StoreReference {
    /// Create a new store reference with just the FQN and crate
    pub fn new(fqn: String, crate_name: String) -> Self {
        debug!(fqn = %fqn, crate_name = %crate_name, "StoreReference::new entry");
        Self {
            fqn,
            postgres_id: None,
            neo4j_node_id: None,
            qdrant_point_id: None,
            crate_name,
            last_synced: None,
        }
    }

    /// Check if the item exists in all three stores
    pub fn is_fully_synced(&self) -> bool {
        let result = self.postgres_id.is_some()
            && self.neo4j_node_id.is_some()
            && self.qdrant_point_id.is_some();
        debug!(result = result, "StoreReference::is_fully_synced exit");
        result
    }

    /// Check if the item is orphaned (exists in no store)
    pub fn is_orphaned(&self) -> bool {
        let result = self.postgres_id.is_none()
            && self.neo4j_node_id.is_none()
            && self.qdrant_point_id.is_none();
        if result {
            warn!(fqn = %self.fqn, "StoreReference is orphaned");
        }
        result
    }

    /// Get stores where this item is missing
    pub fn missing_stores(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if self.postgres_id.is_none() {
            missing.push("postgres");
        }
        if self.neo4j_node_id.is_none() {
            missing.push("neo4j");
        }
        if self.qdrant_point_id.is_none() {
            missing.push("qdrant");
        }
        if !missing.is_empty() {
            warn!(fqn = %self.fqn, missing = ?missing, "StoreReference has missing stores");
        }
        missing
    }
}

impl std::str::FromStr for ResolutionQuality {
    type Err = String;

    /// Parses a resolution quality from a lowercase string.
    ///
    /// Accepts `"analyzed"`, `"heuristic"`, or `"unknown"`.
    ///
    /// # Errors
    ///
    /// Returns an error string for any unrecognized value.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        trace!("ResolutionQuality::from_str entry");
        let result = match s {
            "analyzed" => Ok(ResolutionQuality::Analyzed),
            "heuristic" => Ok(ResolutionQuality::Heuristic),
            "unknown" => Ok(ResolutionQuality::Unknown),
            _ => Err(format!("Unknown resolution quality: {}", s)),
        };
        match &result {
            Ok(q) => debug!(quality = ?q, "ResolutionQuality::from_str success"),
            Err(e) => warn!(input = s, error = %e, "ResolutionQuality::from_str failed"),
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_item_type_as_str() {
        assert_eq!(ItemType::Function.as_str(), "function");
        assert_eq!(ItemType::ExternBlock.as_str(), "extern_block");
        assert_eq!(ItemType::Unknown("custom".to_string()).as_str(), "custom");
    }

    #[test]
    fn test_visibility_display() {
        assert_eq!(Visibility::Public.to_string(), "pub");
        assert_eq!(Visibility::PubCrate.to_string(), "pub(crate)");
        assert_eq!(Visibility::Private.to_string(), "");
    }

    #[test]
    fn test_resolution_quality_roundtrip() {
        assert_eq!(
            "analyzed".parse::<ResolutionQuality>().unwrap(),
            ResolutionQuality::Analyzed
        );
        assert_eq!(
            "heuristic".parse::<ResolutionQuality>().unwrap(),
            ResolutionQuality::Heuristic
        );
    }

    #[test]
    fn test_item_type_serialization() {
        let json = serde_json::to_string(&ItemType::Function).unwrap();
        assert_eq!(json, "\"function\"");
        let deserialized: ItemType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ItemType::Function);
    }

    #[test]
    fn test_generic_param_serialization() {
        let param = GenericParam {
            name: "T".to_string(),
            kind: "type".to_string(),
            bounds: vec!["Clone".to_string()],
            default: None,
        };
        let json = serde_json::to_value(&param).unwrap();
        assert_eq!(json["name"], "T");
        assert_eq!(json["bounds"], serde_json::json!(["Clone"]));
    }
}
