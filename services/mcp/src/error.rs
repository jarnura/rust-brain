//! Error types for the MCP server

use thiserror::Error;

/// Main error type for the MCP server
#[derive(Debug, Error)]
pub enum McpError {
    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// API returned an error
    #[error("API error: {0}")]
    Api(String),

    /// Resource not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// Invalid request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// JSON parsing error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// MCP protocol error
    #[error("MCP protocol error: {0}")]
    Protocol(String),
}

impl McpError {
    /// Convert to MCP error code
    pub fn to_code(&self) -> i32 {
        match self {
            McpError::Http(_) => -1,
            McpError::Api(_) => -2,
            McpError::NotFound(_) => -3,
            McpError::InvalidRequest(_) => -4,
            McpError::Internal(_) => -5,
            McpError::Json(_) => -6,
            McpError::Io(_) => -7,
            McpError::Protocol(_) => -8,
        }
    }

    /// Check if this error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            McpError::Http(_) | McpError::Internal(_)
        )
    }
}

/// Result type alias for MCP operations
pub type Result<T> = std::result::Result<T, McpError>;
