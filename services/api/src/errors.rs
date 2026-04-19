//! Error types for the rust-brain API server.
//!
//! [`AppError`] is the canonical error type returned by all handler functions.
//! It implements [`IntoResponse`] so Axum can convert it directly into an HTTP
//! response with the appropriate status code and a JSON body matching
//! [`ApiError`].
//!
//! # Status Code Mapping
//!
//! | Variant | HTTP Status | JSON `code` |
//! |---------|-------------|-------------|
//! | `Database` | 500 | `DATABASE_ERROR` |
//! | `Neo4j` | 500 | `NEO4J_ERROR` |
//! | `Qdrant` | 500 | `QDRANT_ERROR` |
//! | `Ollama` | 500 | `OLLAMA_ERROR` |
//! | `OpenCode` | 500 | `OPENCODE_ERROR` |
//! | `NotFound` | 404 | `NOT_FOUND` |
//! | `BadRequest` | 400 | `BAD_REQUEST` |
//! | `Internal` | 500 | `INTERNAL_ERROR` |

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::fmt;

/// JSON body returned for all error responses.
///
/// Every error response contains a human-readable `error` message and a
/// machine-readable `code` string (see module-level table).
#[derive(Debug, Serialize)]
pub struct ApiError {
    /// Human-readable error description
    pub error: String,
    /// Machine-readable error code (e.g., `"NOT_FOUND"`, `"DATABASE_ERROR"`)
    pub code: String,
}

/// Application error type used by all API handlers.
///
/// Each variant wraps a descriptive message string. The [`IntoResponse`]
/// implementation maps variants to HTTP status codes and serializes the
/// response as [`ApiError`] JSON.
#[derive(Debug)]
pub enum AppError {
    /// PostgreSQL operation failed
    Database(String),
    /// Neo4j graph database operation failed
    Neo4j(String),
    /// Qdrant vector store operation failed
    Qdrant(String),
    /// Ollama embedding/chat model operation failed
    Ollama(String),
    /// OpenCode session management operation failed
    OpenCode(String),
    /// Requested resource does not exist (HTTP 404)
    NotFound(String),
    /// Client sent an invalid request (HTTP 400)
    BadRequest(String),
    /// Unclassified server error (HTTP 500)
    Internal(String),
    /// Operation conflicts with existing resource state (HTTP 409)
    Conflict(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Database(msg) => write!(f, "Database error: {}", msg),
            AppError::Neo4j(msg) => write!(f, "Neo4j error: {}", msg),
            AppError::Qdrant(msg) => write!(f, "Qdrant error: {}", msg),
            AppError::Ollama(msg) => write!(f, "Ollama error: {}", msg),
            AppError::OpenCode(msg) => write!(f, "OpenCode error: {}", msg),
            AppError::NotFound(msg) => write!(f, "Not found: {}", msg),
            AppError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            AppError::Internal(msg) => write!(f, "Internal error: {}", msg),
            AppError::Conflict(msg) => write!(f, "Conflict: {}", msg),
        }
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error, code) = match self {
            AppError::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "DATABASE_ERROR"),
            AppError::Neo4j(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "NEO4J_ERROR"),
            AppError::Qdrant(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "QDRANT_ERROR"),
            AppError::Ollama(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "OLLAMA_ERROR"),
            AppError::OpenCode(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "OPENCODE_ERROR"),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg, "NOT_FOUND"),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg, "BAD_REQUEST"),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg, "INTERNAL_ERROR"),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg, "CONFLICT"),
        };

        let body = Json(ApiError {
            error: error.to_string(),
            code: code.to_string(),
        });

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_error_display() {
        assert_eq!(
            AppError::Database("conn refused".to_string()).to_string(),
            "Database error: conn refused"
        );
        assert_eq!(
            AppError::NotFound("item".to_string()).to_string(),
            "Not found: item"
        );
        assert_eq!(
            AppError::BadRequest("invalid".to_string()).to_string(),
            "Bad request: invalid"
        );
        assert_eq!(
            AppError::Neo4j("timeout".to_string()).to_string(),
            "Neo4j error: timeout"
        );
        assert_eq!(
            AppError::Qdrant("error".to_string()).to_string(),
            "Qdrant error: error"
        );
        assert_eq!(
            AppError::Ollama("error".to_string()).to_string(),
            "Ollama error: error"
        );
        assert_eq!(
            AppError::Internal("panic".to_string()).to_string(),
            "Internal error: panic"
        );
        assert_eq!(
            AppError::Conflict("duplicate".to_string()).to_string(),
            "Conflict: duplicate"
        );
    }

    #[test]
    fn test_app_error_into_response_status_codes() {
        use axum::http::StatusCode;

        let cases = vec![
            (
                AppError::Database("err".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::Neo4j("err".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::Qdrant("err".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::Ollama("err".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (
                AppError::Internal("err".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            (AppError::Conflict("err".into()), StatusCode::CONFLICT),
            (AppError::NotFound("err".into()), StatusCode::NOT_FOUND),
            (AppError::BadRequest("err".into()), StatusCode::BAD_REQUEST),
        ];

        for (error, expected_status) in cases {
            let response = error.into_response();
            assert_eq!(response.status(), expected_status);
        }
    }

    #[test]
    fn test_api_error_serialization() {
        let api_error = ApiError {
            error: "Something went wrong".to_string(),
            code: "INTERNAL_ERROR".to_string(),
        };
        let json = serde_json::to_value(&api_error).unwrap();
        assert_eq!(json["error"], "Something went wrong");
        assert_eq!(json["code"], "INTERNAL_ERROR");
    }
}
