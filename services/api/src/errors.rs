//! Error types for the rust-brain API server.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

#[derive(Debug)]
pub enum AppError {
    Database(String),
    Neo4j(String),
    Qdrant(String),
    Ollama(String),
    OpenCode(String),
    NotFound(String),
    BadRequest(String),
    Internal(String),
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
        }
    }
}

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
    }

    #[test]
    fn test_app_error_into_response_status_codes() {
        use axum::http::StatusCode;

        let cases = vec![
            (AppError::Database("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Neo4j("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Qdrant("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Ollama("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Internal("err".into()), StatusCode::INTERNAL_SERVER_ERROR),
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
