use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::rows::CreateApiKeyData;
use crate::db::repos::{ApiKeyRepository, UserRepository};
use tracing::info;
use uuid::Uuid;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::api::validators::validate_scopes;
use crate::AppState;

pub async fn list_all_api_keys(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<AdminApiKeyDto>>> {
    auth.require_admin()?;
    let users = UserRepository::list(state.db.pool(), 0, 200).await.unwrap_or_default();
    let user_map: std::collections::HashMap<String, String> = users.iter()
        .map(|u| (u.id.to_string(), u.username.clone()))
        .collect();
    let mut all_keys = Vec::new();

    for user in &users {
        let keys = ApiKeyRepository::list_by_user(state.db.pool(), user.id).await.unwrap_or_default();
        for k in keys {
            all_keys.push(AdminApiKeyDto {
                id: k.id.to_string(),
                user_id: k.user_id.map(|u| u.to_string()),
                username: k.user_id.and_then(|u| user_map.get(&u.to_string()).cloned()),
                name: k.name,
                prefix: k.key_prefix,
                scopes: k.scopes,
                last_used_at: k.last_used_at,
                expires_at: k.expires_at,
                created_at: k.created_at,
                is_active: k.is_active,
            });
        }
    }

    let bot_keys = ApiKeyRepository::list_bot_keys(state.db.pool()).await.unwrap_or_default();
    for k in bot_keys {
        all_keys.push(AdminApiKeyDto {
            id: k.id.to_string(),
            user_id: None,
            username: None,
            name: k.name,
            prefix: k.key_prefix,
            scopes: k.scopes,
            last_used_at: k.last_used_at,
            expires_at: k.expires_at,
            created_at: k.created_at,
            is_active: k.is_active,
        });
    }

    Ok(Json(all_keys))
}

pub async fn create_admin_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateAdminApiKeyRequest>,
) -> AppResult<Json<serde_json::Value>> {
    auth.require_admin()?;

    // Same bounds the user-facing endpoint enforces (that DTO validates them;
    // this one is a plain Deserialize so enforce here).
    if body.name.trim().is_empty() || body.name.chars().count() > 100 {
        return Err(AppError::BadRequest("key name must be 1..=100 characters".into()));
    }
    if body.scopes.len() > 10 || !validate_scopes(&body.scopes) {
        return Err(AppError::BadRequest(
            "one or more scopes are not allowed".into(),
        ));
    }
    // Cap the duration: chrono's Duration overflows (panic) for huge day
    // counts (see the user-facing create_api_key for the same guard).
    if body.expires_in_days.is_some_and(|d| d > 3650) {
        return Err(AppError::BadRequest(
            "expires_in_days must not exceed 3650".into(),
        ));
    }

    let target_user_id = body.user_id.as_ref().map(|uid| {
        Uuid::parse_str(uid).map_err(|_| AppError::BadRequest("invalid user id".into()))
    }).transpose()?;

    if let Some(uid) = target_user_id {
        let _ = UserRepository::find_by_id(state.db.pool(), uid).await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
    }

    let expires_at = body.expires_in_days
        .map(|days| chrono::Utc::now() + chrono::Duration::days(days as i64));

    let (full_key, prefix, key_hash) = if target_user_id.is_some() {
        crate::utils::auth::api_keys::generate_api_key()
    } else {
        let random_suffix = &Uuid::new_v4().to_string()[..8];
        let prefix = format!("bot_{random_suffix}");
        let (full, _, hash) = crate::utils::auth::api_keys::generate_api_key();
        (full, prefix, hash)
    };

    let data = CreateApiKeyData {
        user_id: target_user_id,
        name: body.name.clone(),
        key_prefix: prefix.clone(),
        key_hash,
        scopes: body.scopes.clone(),
        expires_at,
    };
    let api_key = ApiKeyRepository::create(state.db.pool(), data).await?;
    info!("admin {} created API key '{}' (bot={})", auth.username, body.name, target_user_id.is_none());
    Ok(Json(serde_json::json!({
        "id": api_key.id,
        "name": body.name,
        "full_key": full_key,
        "prefix": prefix,
        "scopes": body.scopes,
        "expires_at": expires_at,
    })))
}

pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<RevokeApiKeyRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    let uid = Uuid::parse_str(&body.id)
        .map_err(|_| AppError::BadRequest("invalid key id".into()))?;
    ApiKeyRepository::deactivate(state.db.pool(), uid).await?;
    info!("admin {} revoked API key {}", auth.username, body.id);
    Ok(Json(MessageResponse { message: "API key revoked".to_string() }))
}
