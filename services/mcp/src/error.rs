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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        // Test each error variant's code
        let api_err = McpError::Api("API failed".to_string());
        assert_eq!(api_err.to_code(), -2);

        let not_found = McpError::NotFound("Item not found".to_string());
        assert_eq!(not_found.to_code(), -3);

        let invalid_req = McpError::InvalidRequest("Bad request".to_string());
        assert_eq!(invalid_req.to_code(), -4);

        let internal = McpError::Internal("Internal error".to_string());
        assert_eq!(internal.to_code(), -5);

        let json_err = McpError::Json(serde_json::from_str::<i32>("not a number").unwrap_err());
        assert_eq!(json_err.to_code(), -6);

        let io_err = McpError::Io(std::io::Error::new(std::io::ErrorKind::Other, "IO error"));
        assert_eq!(io_err.to_code(), -7);

        let protocol_err = McpError::Protocol("Protocol error".to_string());
        assert_eq!(protocol_err.to_code(), -8);
    }

    #[test]
    fn test_is_retryable() {
        // Retryable errors
        let internal = McpError::Internal("Internal error".to_string());
        assert!(internal.is_retryable());

        // Non-retryable errors
        let api_err = McpError::Api("API failed".to_string());
        assert!(!api_err.is_retryable());

        let not_found = McpError::NotFound("Not found".to_string());
        assert!(!not_found.is_retryable());

        let invalid_req = McpError::InvalidRequest("Bad request".to_string());
        assert!(!invalid_req.is_retryable());

        let json_err = McpError::Json(serde_json::from_str::<i32>("not a number").unwrap_err());
        assert!(!json_err.is_retryable());

        let io_err = McpError::Io(std::io::Error::new(std::io::ErrorKind::Other, "IO error"));
        assert!(!io_err.is_retryable());

        let protocol_err = McpError::Protocol("Protocol error".to_string());
        assert!(!protocol_err.is_retryable());
    }

    #[test]
    fn test_error_display() {
        let api_err = McpError::Api("Something went wrong".to_string());
        assert!(api_err.to_string().contains("API error"));
        assert!(api_err.to_string().contains("Something went wrong"));

        let not_found = McpError::NotFound("my_function".to_string());
        assert!(not_found.to_string().contains("Not found"));
        assert!(not_found.to_string().contains("my_function"));

        let invalid_req = McpError::InvalidRequest("missing field".to_string());
        assert!(invalid_req.to_string().contains("Invalid request"));
    }

    #[test]
    fn test_json_error_from_serde() {
        let json_str = r#"{"invalid": json}"#;
        let result: std::result::Result<serde_json::Value, _> = serde_json::from_str(json_str);
        assert!(result.is_err());
        
        // The error should be convertible to McpError
        let mcp_err: McpError = result.unwrap_err().into();
        assert!(matches!(mcp_err, McpError::Json(_)));
    }

    #[test]
    fn test_error_debug() {
        let api_err = McpError::Api("test".to_string());
        let debug_str = format!("{:?}", api_err);
        assert!(debug_str.contains("Api"));
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let mcp_err: McpError = io_err.into();
        assert!(matches!(mcp_err, McpError::Io(_)));
    }
}
