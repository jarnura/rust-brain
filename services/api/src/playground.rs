//! Playground HTML endpoint for interactive API exploration
//!
//! Provides a self-contained web UI for:
//! - Semantic code search
//! - Function lookup by FQN/pattern
//! - Call graph visualization
//! - Raw Cypher query execution

use axum::{
    extract::State,
    http::{header, Response},
    body::Body,
};
use crate::AppState;

/// Generate the playground HTML page
pub async fn playground_html(State(_state): State<AppState>) -> Response<Body> {
    let html = include_str!("../static/playground.html");
    
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(html))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playground_html_content_type() {
        // The playground should return HTML content
        let html = include_str!("../static/playground.html");
        assert!(!html.is_empty());
        assert!(html.contains("<!DOCTYPE html>"));
    }
}
