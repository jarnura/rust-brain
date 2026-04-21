//! API key authentication middleware per ADR-007.
//!
//! Extracts the `Authorization: Bearer <key>` header, SHA-256 hashes the key,
//! looks up the hash in the `api_keys` table, and attaches [`ApiKeyContext`]
//! to request extensions. Rejects expired, inactive, or missing keys with 401.
//!
//! Bypass routes (always public): `GET /health`, `GET /metrics`.
//!
//! When `RUSTBRAIN_AUTH_DISABLED=true`, all requests receive a synthetic
//! admin context and skip the database lookup.

use axum::{
    body::Body,
    extract::State,
    http::{Request, Response},
    middleware::Next,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tracing::{debug, warn};

use crate::errors::AppError;
use crate::state::AppState;

/// Key tier determining endpoint access and default rate limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Admin,
    Standard,
    Readonly,
}

impl Tier {
    /// Default rate limit (requests per minute) for each tier.
    pub fn default_rate_limit(&self) -> u32 {
        match self {
            Tier::Admin => 120,
            Tier::Standard => 60,
            Tier::Readonly => 30,
        }
    }

    /// Whether this tier can access write endpoints (key management, ingestion triggers).
    pub fn can_write(&self) -> bool {
        matches!(self, Tier::Admin)
    }

    /// Whether this tier can access chat endpoints.
    pub fn can_chat(&self) -> bool {
        matches!(self, Tier::Admin | Tier::Standard)
    }
}

impl std::fmt::Display for Tier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tier::Admin => write!(f, "admin"),
            Tier::Standard => write!(f, "standard"),
            Tier::Readonly => write!(f, "readonly"),
        }
    }
}

/// Context attached to authenticated requests via request extensions.
///
/// Available to handlers via `Extension<ApiKeyContext>` extractor.
#[derive(Debug, Clone)]
pub struct ApiKeyContext {
    /// UUID of the API key row in the database.
    pub key_id: String,
    /// Access tier of the key.
    pub tier: Tier,
    /// Workspace scope: `None` = all workspaces, `Some(id)` = scoped.
    pub workspace_id: Option<String>,
    /// Per-key rate limit in requests per minute.
    pub rate_limit_per_minute: u32,
}

/// SHA-256 hash a raw API key string, returning the hex-encoded digest.
pub fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Rejects requests from tiers without chat access (readonly).
pub fn require_chat_access(ctx: &ApiKeyContext) -> Result<(), AppError> {
    if !ctx.tier.can_chat() {
        Err(AppError::Unauthorized(
            "Chat access requires standard or admin tier".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Rejects requests from tiers without write access (non-admin).
pub fn require_write_access(ctx: &ApiKeyContext) -> Result<(), AppError> {
    if !ctx.tier.can_write() {
        Err(AppError::Unauthorized(
            "Write operations require admin tier".to_string(),
        ))
    } else {
        Ok(())
    }
}

/// Routes that bypass authentication entirely (always public).
fn is_public_route(path: &str, method: &str) -> bool {
    matches!(path, "/health" | "/metrics" | "/health/consistency") && method == "GET"
}

/// Axum middleware that enforces API key authentication.
///
/// Flow:
/// 1. If `RUSTBRAIN_AUTH_DISABLED=true`, inject a synthetic admin context.
/// 2. If the route is public (`/health`, `/metrics`), skip auth.
/// 3. Extract `Authorization: Bearer <key>` header.
/// 4. SHA-256 hash the key, query `api_keys` table.
/// 5. Reject expired, inactive, or non-existent keys with 401.
/// 6. Attach `ApiKeyContext` to request extensions.
/// 7. Per-key rate limiting with headers (ADR-007).
/// 8. Update `last_used_at` asynchronously (fire-and-forget).
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response<Body>, AppError> {
    // 1. Auth disabled mode — synthetic admin context
    if state.config.auth_disabled {
        debug!("Auth disabled: injecting synthetic admin context");
        req.extensions_mut().insert(ApiKeyContext {
            key_id: "auth-disabled".to_string(),
            tier: Tier::Admin,
            workspace_id: None,
            rate_limit_per_minute: Tier::Admin.default_rate_limit(),
        });
        return Ok(next.run(req).await);
    }

    // 2. Public routes bypass auth
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    if is_public_route(&path, method.as_str()) {
        debug!(path = %path, "Public route: skipping auth");
        return Ok(next.run(req).await);
    }

    // 3. Extract Bearer token
    let key = extract_bearer_token(&req)?;

    // 4. Hash and lookup
    let key_hash = hash_key(&key);
    debug!(key_hash_prefix = &key_hash[..16], "Looking up API key");

    let row = sqlx::query(
        "SELECT id, tier, workspace_id, rate_limit_per_minute, expires_at, is_active \
         FROM api_keys WHERE key_hash = $1",
    )
    .bind(&key_hash)
    .fetch_optional(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Key lookup failed: {}", e)))?;

    let row = row.ok_or_else(|| {
        warn!(key_hash_prefix = &key_hash[..16], "API key not found");
        AppError::Unauthorized("Invalid API key".to_string())
    })?;

    // 5. Validate key state
    let is_active: bool = row.get("is_active");
    if !is_active {
        warn!(key_hash_prefix = &key_hash[..16], "API key is inactive");
        return Err(AppError::Unauthorized(
            "API key has been revoked".to_string(),
        ));
    }

    let expires_at: Option<chrono::DateTime<Utc>> = row.get("expires_at");
    if let Some(exp) = expires_at {
        if exp < Utc::now() {
            warn!(key_hash_prefix = &key_hash[..16], "API key expired");
            return Err(AppError::Unauthorized("API key has expired".to_string()));
        }
    }

    let tier_str: String = row.get("tier");
    let tier = match tier_str.as_str() {
        "admin" => Tier::Admin,
        "standard" => Tier::Standard,
        "readonly" => Tier::Readonly,
        other => {
            warn!(tier = other, "Unknown tier in database");
            return Err(AppError::Internal(format!("Unknown tier: {}", other)));
        }
    };

    let key_id: uuid::Uuid = row.get("id");
    let workspace_id: Option<String> = row.get("workspace_id");
    let rate_limit_per_minute: i32 = row.get("rate_limit_per_minute");

    let context = ApiKeyContext {
        key_id: key_id.to_string(),
        tier,
        workspace_id,
        rate_limit_per_minute: rate_limit_per_minute as u32,
    };

    // 6. Attach context to extensions
    req.extensions_mut().insert(context.clone());

    // 7. Per-key rate limiting
    let key_id_str = context.key_id.clone();
    let result = state
        .rate_limiter
        .check(&key_id_str, context.rate_limit_per_minute);

    if result.allowed {
        let mut response = next.run(req).await;
        let headers = response.headers_mut();
        headers.insert("x-ratelimit-limit", result.limit.into());
        headers.insert("x-ratelimit-remaining", result.remaining.into());
        headers.insert("x-ratelimit-reset", result.reset_at_secs.into());

        // 8. Update last_used_at asynchronously
        let pool = state.pg_pool.clone();
        let kid = key_id_str.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1")
                .bind(&kid)
                .execute(&pool)
                .await
            {
                debug!(key_id = %kid, error = %e, "Failed to update last_used_at");
            }
        });

        Ok(response)
    } else {
        let retry_after = 60 - (std::time::Instant::now().elapsed().as_secs() % 60).max(1);
        warn!(
            key_id = %key_id_str,
            limit = result.limit,
            "Rate limit exceeded"
        );
        Err(AppError::RateLimited {
            retry_after_secs: retry_after,
        })
    }
}

/// Extracts the Bearer token from the Authorization header.
fn extract_bearer_token(req: &Request<Body>) -> Result<String, AppError> {
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("Missing Authorization header".to_string()))?;

    let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
        AppError::Unauthorized("Invalid Authorization header: expected Bearer token".to_string())
    })?;

    if token.is_empty() {
        return Err(AppError::Unauthorized("Empty Bearer token".to_string()));
    }

    Ok(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_key_deterministic() {
        let key = "rb_live_abcdef1234567890abcdef1234567890";
        assert_eq!(hash_key(key), hash_key(key));
    }

    #[test]
    fn test_hash_key_different_inputs() {
        let a = hash_key("key_a");
        let b = hash_key("key_b");
        assert_ne!(a, b);
    }

    #[test]
    fn test_tier_default_rate_limits() {
        assert_eq!(Tier::Admin.default_rate_limit(), 120);
        assert_eq!(Tier::Standard.default_rate_limit(), 60);
        assert_eq!(Tier::Readonly.default_rate_limit(), 30);
    }

    #[test]
    fn test_tier_access_control() {
        assert!(Tier::Admin.can_write());
        assert!(Tier::Admin.can_chat());
        assert!(!Tier::Standard.can_write());
        assert!(Tier::Standard.can_chat());
        assert!(!Tier::Readonly.can_write());
        assert!(!Tier::Readonly.can_chat());
    }

    #[test]
    fn test_tier_display() {
        assert_eq!(Tier::Admin.to_string(), "admin");
        assert_eq!(Tier::Standard.to_string(), "standard");
        assert_eq!(Tier::Readonly.to_string(), "readonly");
    }

    #[test]
    fn test_tier_serde_roundtrip() {
        for tier in [Tier::Admin, Tier::Standard, Tier::Readonly] {
            let json = serde_json::to_string(&tier).unwrap();
            let back: Tier = serde_json::from_str(&json).unwrap();
            assert_eq!(tier, back);
        }
    }

    #[test]
    fn test_is_public_route() {
        assert!(is_public_route("/health", "GET"));
        assert!(is_public_route("/metrics", "GET"));
        assert!(is_public_route("/health/consistency", "GET"));
        assert!(!is_public_route("/health", "POST"));
        assert!(!is_public_route("/api/keys", "GET"));
        assert!(!is_public_route("/tools/search_semantic", "POST"));
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        let req = axum::http::Request::builder()
            .header("Authorization", "Bearer rb_live_test123")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_bearer_token(&req).unwrap(), "rb_live_test123");
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        let req = axum::http::Request::builder().body(Body::empty()).unwrap();
        assert!(extract_bearer_token(&req).is_err());
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let req = axum::http::Request::builder()
            .header("Authorization", "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();
        let err = extract_bearer_token(&req).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn test_extract_bearer_token_empty() {
        let req = axum::http::Request::builder()
            .header("Authorization", "Bearer ")
            .body(Body::empty())
            .unwrap();
        let err = extract_bearer_token(&req).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized(_)));
    }

    #[test]
    fn test_hash_key_length() {
        let hash = hash_key("test_key");
        // SHA-256 produces 64 hex chars
        assert_eq!(hash.len(), 64);
    }
}
