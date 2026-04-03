//! Playground route handler.

use axum::{http::StatusCode, response::IntoResponse};

pub async fn playground_html() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("Content-Type", "text/html; charset=utf-8")],
        include_str!("../../static/index.html"),
    )
}
