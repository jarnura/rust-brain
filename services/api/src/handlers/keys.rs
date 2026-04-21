//! API key management handlers per ADR-007.
//!
//! All endpoints require `admin` tier access:
//! - `POST /api/keys` — create a new key (returns plaintext key once)
//! - `GET /api/keys` — list key metadata (never returns key values)
//! - `DELETE /api/keys/{id}` — revoke (soft-delete) a key
//! - `PATCH /api/keys/{id}` — update tier, rate limit, or expiry

use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::errors::AppError;
use crate::middleware::auth::{ApiKeyContext, Tier};
use crate::state::AppState;

/// Generate a new API key in the `rb_live_<32-hex-chars>` format.
fn generate_api_key() -> String {
    let random_bytes = *Uuid::new_v4().as_bytes();
    let hex: String = random_bytes.iter().map(|b| format!("{:02x}", b)).collect();
    format!("rb_live_{}", hex)
}

// =============================================================================
// Request / Response types
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(default = "default_tier")]
    pub tier: Tier,
    pub workspace_id: Option<String>,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_minute: u32,
    pub expires_at: Option<DateTime<Utc>>,
}

fn default_tier() -> Tier {
    Tier::Standard
}

fn default_rate_limit() -> u32 {
    60
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub id: Uuid,
    pub name: String,
    pub tier: Tier,
    pub key: String,
    pub workspace_id: Option<String>,
    pub rate_limit_per_minute: u32,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct KeyMetadata {
    pub id: Uuid,
    pub name: String,
    pub tier: Tier,
    pub workspace_id: Option<String>,
    pub rate_limit_per_minute: u32,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateKeyRequest {
    pub tier: Option<Tier>,
    pub rate_limit_per_minute: Option<u32>,
    pub expires_at: Option<Option<DateTime<Utc>>>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ListKeysResponse {
    pub keys: Vec<KeyMetadata>,
}

// =============================================================================
// Handlers
// =============================================================================

pub async fn create_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreateKeyResponse>), AppError> {
    if !ctx.tier.can_write() {
        return Err(AppError::Unauthorized(
            "Only admin tier can create API keys".to_string(),
        ));
    }

    let raw_key = generate_api_key();
    let key_hash = crate::middleware::auth::hash_key(&raw_key);

    let effective_rate_limit = match req.tier {
        Tier::Admin => req.rate_limit_per_minute.max(120),
        Tier::Standard => req.rate_limit_per_minute,
        Tier::Readonly => req.rate_limit_per_minute.min(30),
    };

    let row = sqlx::query(
        "INSERT INTO api_keys (key_hash, name, tier, workspace_id, rate_limit_per_minute, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id",
    )
    .bind(&key_hash)
    .bind(&req.name)
    .bind(req.tier.to_string())
    .bind(&req.workspace_id)
    .bind(effective_rate_limit as i32)
    .bind(req.expires_at)
    .fetch_one(&state.pg_pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            AppError::Conflict("An API key with this name or value already exists".to_string())
        } else {
            AppError::Database(format!("Failed to create API key: {}", e))
        }
    })?;

    let id: Uuid = row.get("id");

    Ok((
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            id,
            name: req.name,
            tier: req.tier,
            key: raw_key,
            workspace_id: req.workspace_id,
            rate_limit_per_minute: effective_rate_limit,
            expires_at: req.expires_at,
        }),
    ))
}

pub async fn list_keys(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
) -> Result<Json<ListKeysResponse>, AppError> {
    if !ctx.tier.can_write() {
        return Err(AppError::Unauthorized(
            "Only admin tier can list API keys".to_string(),
        ));
    }

    let rows = sqlx::query(
        "SELECT id, name, tier, workspace_id, rate_limit_per_minute, is_active, \
         created_at, expires_at, last_used_at \
         FROM api_keys ORDER BY created_at DESC",
    )
    .fetch_all(&state.pg_pool)
    .await
    .map_err(|e| AppError::Database(format!("Failed to list API keys: {}", e)))?;

    let keys: Vec<KeyMetadata> = rows
        .iter()
        .map(|row| {
            let tier_str: String = row.get("tier");
            KeyMetadata {
                id: row.get("id"),
                name: row.get("name"),
                tier: match tier_str.as_str() {
                    "admin" => Tier::Admin,
                    "standard" => Tier::Standard,
                    _ => Tier::Readonly,
                },
                workspace_id: row.get("workspace_id"),
                rate_limit_per_minute: row.get::<i32, _>("rate_limit_per_minute") as u32,
                is_active: row.get("is_active"),
                created_at: row.get("created_at"),
                expires_at: row.get("expires_at"),
                last_used_at: row.get("last_used_at"),
            }
        })
        .collect();

    Ok(Json(ListKeysResponse { keys }))
}

pub async fn revoke_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    if !ctx.tier.can_write() {
        return Err(AppError::Unauthorized(
            "Only admin tier can revoke API keys".to_string(),
        ));
    }

    let result = sqlx::query("UPDATE api_keys SET is_active = false WHERE id = $1")
        .bind(id)
        .execute(&state.pg_pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to revoke API key: {}", e)))?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound(format!("API key {} not found", id)));
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn update_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<ApiKeyContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateKeyRequest>,
) -> Result<Json<KeyMetadata>, AppError> {
    if !ctx.tier.can_write() {
        return Err(AppError::Unauthorized(
            "Only admin tier can update API keys".to_string(),
        ));
    }

    // Build dynamic UPDATE query
    let mut set_clauses: Vec<String> = Vec::new();
    let mut param_idx = 2; // $1 is id
    let mut tier_val: Option<String> = None;
    let mut rate_limit_val: Option<i32> = None;
    let mut expires_at_val: Option<Option<DateTime<Utc>>> = None;
    let mut is_active_val: Option<bool> = None;

    if let Some(ref tier) = req.tier {
        set_clauses.push(format!("tier = ${}", param_idx));
        tier_val = Some(tier.to_string());
        param_idx += 1;
    }
    if let Some(rate_limit) = req.rate_limit_per_minute {
        set_clauses.push(format!("rate_limit_per_minute = ${}", param_idx));
        rate_limit_val = Some(rate_limit as i32);
        param_idx += 1;
    }
    if let Some(ref expires_at) = req.expires_at {
        set_clauses.push(format!("expires_at = ${}", param_idx));
        expires_at_val = Some(*expires_at);
        param_idx += 1;
    }
    if let Some(is_active) = req.is_active {
        set_clauses.push(format!("is_active = ${}", param_idx));
        is_active_val = Some(is_active);
    }

    if set_clauses.is_empty() {
        return Err(AppError::BadRequest("No fields to update".to_string()));
    }

    let query = format!(
        "UPDATE api_keys SET {} WHERE id = $1 \
         RETURNING id, name, tier, workspace_id, rate_limit_per_minute, is_active, \
         created_at, expires_at, last_used_at",
        set_clauses.join(", ")
    );

    let mut q = sqlx::query(&query).bind(id);

    if let Some(ref t) = tier_val {
        q = q.bind(t);
    }
    if let Some(r) = rate_limit_val {
        q = q.bind(r);
    }
    if let Some(ref e) = expires_at_val {
        q = q.bind(e);
    }
    if let Some(a) = is_active_val {
        q = q.bind(a);
    }

    let row = q
        .fetch_optional(&state.pg_pool)
        .await
        .map_err(|e| AppError::Database(format!("Failed to update API key: {}", e)))?;

    let row = row.ok_or_else(|| AppError::NotFound(format!("API key {} not found", id)))?;

    let tier_str: String = row.get("tier");
    Ok(Json(KeyMetadata {
        id: row.get("id"),
        name: row.get("name"),
        tier: match tier_str.as_str() {
            "admin" => Tier::Admin,
            "standard" => Tier::Standard,
            _ => Tier::Readonly,
        },
        workspace_id: row.get("workspace_id"),
        rate_limit_per_minute: row.get::<i32, _>("rate_limit_per_minute") as u32,
        is_active: row.get("is_active"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        last_used_at: row.get("last_used_at"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("rb_live_"));
        assert_eq!(key.len(), 8 + 32); // "rb_live_" + 32 hex chars
    }

    #[test]
    fn test_generate_api_key_uniqueness() {
        let a = generate_api_key();
        let b = generate_api_key();
        assert_ne!(a, b);
    }

    #[test]
    fn test_default_tier_is_standard() {
        assert_eq!(default_tier(), Tier::Standard);
    }

    #[test]
    fn test_default_rate_limit() {
        assert_eq!(default_rate_limit(), 60);
    }
}
