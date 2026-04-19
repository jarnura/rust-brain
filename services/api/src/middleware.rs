//! Axum middleware for workspace-aware request metrics.
//!
//! Intercepts every HTTP request and records Prometheus metrics
//! (request count, duration, errors) with a `workspace` label
//! extracted from the `X-Workspace-Id` header.

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response},
    middleware::Next,
};
use tracing::debug;

use crate::state::AppState;

const WORKSPACE_NONE: &str = "none";

/// Axum middleware that records workspace-labeled request metrics.
///
/// For every request:
/// 1. Extracts `X-Workspace-Id` header (defaults to `"none"`)
/// 2. Derives an `endpoint` label from the request URI path
/// 3. Increments `rustbrain_api_requests_total{endpoint, method, workspace}`
/// 4. Observes `rustbrain_api_request_duration_seconds{endpoint, workspace}`
/// 5. On non-2xx responses, increments `rustbrain_api_errors_total{endpoint, error_code, workspace}`
pub async fn workspace_metrics(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let workspace = extract_workspace(&req);
    let endpoint = derive_endpoint(req.uri().path());
    let method = req.method().clone();

    debug!(
        endpoint = %endpoint,
        method = %method,
        workspace = %workspace,
        "Recording workspace metrics"
    );

    state
        .metrics
        .record_request(&endpoint, method.as_str(), &workspace);

    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed();

    state
        .metrics
        .record_duration(&endpoint, &workspace, duration);

    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let error_code = status.as_u16().to_string();
        state
            .metrics
            .record_error(&endpoint, &error_code, &workspace);
    }

    response
}

/// Extracts the workspace ID from the `X-Workspace-Id` header.
///
/// Returns `"none"` if the header is absent or cannot be decoded.
fn extract_workspace(req: &Request<Body>) -> String {
    req.headers()
        .get("X-Workspace-Id")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or(WORKSPACE_NONE)
        .to_string()
}

/// Derives a stable endpoint label from a URI path.
///
/// Collapses path segments that look like UUIDs or hex IDs into `:id`
/// to limit label cardinality. For example:
/// - `/workspaces/abc123def456/files` → `/workspaces/:id/files`
/// - `/tools/search_semantic` → `/tools/search_semantic`
/// - `/executions/550e8400-e29b-41d4-a716-446655440000` → `/executions/:id`
fn derive_endpoint(path: &str) -> String {
    let segments: Vec<&str> = path.split('/').collect();
    let mut result = Vec::with_capacity(segments.len());

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 && is_dynamic_id(segment) {
            result.push(":id");
        } else {
            result.push(segment);
        }
    }

    result.join("/")
}

/// Checks whether a path segment looks like a dynamic identifier
/// (UUID, hex string, or numeric ID) that should be collapsed.
fn is_dynamic_id(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }

    // Numeric IDs: "123", "42"
    if segment.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // UUIDs: "550e8400-e29b-41d4-a716-446655440000"
    if segment.contains('-') && segment.len() >= 12 {
        return true;
    }

    // Hex IDs (12+ lowercase hex chars, matching workspace short IDs)
    if segment.len() >= 12
        && segment
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_workspace_present() {
        let req = Request::builder()
            .header("X-Workspace-Id", "abc123def456")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_workspace(&req), "abc123def456");
    }

    #[test]
    fn test_extract_workspace_absent() {
        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(extract_workspace(&req), "none");
    }

    #[test]
    fn test_extract_workspace_empty() {
        let req = Request::builder()
            .header("X-Workspace-Id", "")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_workspace(&req), "none");
    }

    #[test]
    fn test_derive_endpoint_static() {
        assert_eq!(
            derive_endpoint("/tools/search_semantic"),
            "/tools/search_semantic"
        );
        assert_eq!(derive_endpoint("/health"), "/health");
        assert_eq!(derive_endpoint("/metrics"), "/metrics");
    }

    #[test]
    fn test_derive_endpoint_dynamic_uuid() {
        assert_eq!(
            derive_endpoint("/workspaces/550e8400-e29b-41d4-a716-446655440000/files"),
            "/workspaces/:id/files"
        );
    }

    #[test]
    fn test_derive_endpoint_dynamic_hex() {
        assert_eq!(
            derive_endpoint("/workspaces/abc123def456/diff"),
            "/workspaces/:id/diff"
        );
    }

    #[test]
    fn test_derive_endpoint_dynamic_numeric() {
        assert_eq!(derive_endpoint("/api/tasks/42"), "/api/tasks/:id");
    }

    #[test]
    fn test_derive_endpoint_executions() {
        assert_eq!(
            derive_endpoint("/executions/550e8400-e29b-41d4-a716-446655440000"),
            "/executions/:id"
        );
    }

    #[test]
    fn test_is_dynamic_id_numeric() {
        assert!(is_dynamic_id("123"));
        assert!(is_dynamic_id("42"));
        assert!(!is_dynamic_id("search"));
    }

    #[test]
    fn test_is_dynamic_id_uuid() {
        assert!(is_dynamic_id("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_dynamic_id("search_semantic"));
    }

    #[test]
    fn test_is_dynamic_id_hex() {
        assert!(is_dynamic_id("abc123def456"));
        assert!(is_dynamic_id("550e8400e29b"));
        assert!(!is_dynamic_id("health"));
        assert!(!is_dynamic_id("files"));
    }
}
