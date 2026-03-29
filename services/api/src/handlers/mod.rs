//! API request handlers for the rust-brain API server.

pub mod health;
pub mod search;
pub mod graph;
pub mod items;
pub mod chat;
pub mod ingestion;
pub mod playground;
pub mod typecheck;

use serde::{Deserialize, Serialize};

// =============================================================================
// Shared Types
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerInfo {
    pub fqn: String,
    pub name: String,
    pub file_path: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalleeInfo {
    pub fqn: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CallerNode {
    pub fqn: String,
    pub name: String,
    pub file_path: String,
    pub line: u32,
    pub depth: usize,
}

// =============================================================================
// Shared Defaults
// =============================================================================

pub fn default_true() -> bool { true }
pub fn default_limit() -> usize { 10 }
pub fn default_depth() -> usize { 1 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 10);
    }

    #[test]
    fn test_default_depth() {
        assert_eq!(default_depth(), 1);
    }
}
