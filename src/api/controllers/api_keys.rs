use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use chrono::{Duration, Utc};
use crate::error::{AppError, AppResult};
use crate::db::rows::CreateApiKeyData;
use crate::db::repos::{AdminSettingRepository, ApiKeyRepository, GroupRepository};
use tracing::info;
use uuid::Uuid;
use validator::Validate;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::api::validators::validate_scopes;
use crate::AppState;

/// Check if non-admin users are allowed to manage API keys.
/// Returns Ok(()) if allowed, or Err if blocked.
///
/// Admins are always allowed. Users that belong to at least one group are
/// governed by their groups' `allow_api_keys` flag (ANY group allows); users
/// with no group fall back to the global `allow_user_api_keys` setting.
async fn check_api_key_allowed(state: &AppState, auth: &AuthUser) -> AppResult<()> {
    // Admins always allowed
    if auth.is_admin() {
        return Ok(());
    }
    let user_id = auth.user_id.to_string();
    let user_groups = GroupRepository::list_user_groups(state.db.pool(), &user_id).await?;
    let allowed = if user_groups.is_empty() {
        AdminSettingRepository::get_bool(state.db.pool(), "allow_user_api_keys").await?
    } else {
        GroupRepository::user_allows_api_keys(state.db.pool(), &user_id).await?
    };
    if !allowed {
        return Err(AppError::Forbidden(
            "API key management is not enabled for your account. Contact an administrator.".into(),
        ));
    }
    Ok(())
}

pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<CreateApiKeyRequest>,
) -> AppResult<Json<ApiKeyCreatedResponse>> {
    check_api_key_allowed(&state, &auth_user).await?;
    // Prevent scope escalation: an API key authenticating with a narrow scope
    // must not be able to mint another key with broader scopes.
    auth_user.require_ui_session()?;
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    if !validate_scopes(&body.scopes) {
        return Err(AppError::Validation(
            "one or more scopes are not allowed".into(),
        ));
    }

    // Cap the duration: chrono's Duration overflows (panic) for huge day
    // counts, and an effectively-never-expiring key should be an explicit
    // decision (`expires_in_days: null`), not a silent u32 edge case.
    if body.expires_in_days.is_some_and(|d| d > 3650) {
        return Err(AppError::Validation(
            "expires_in_days must not exceed 3650".into(),
        ));
    }

    // Enforce max 1 active key per user
    let existing = ApiKeyRepository::list_by_user(state.db.pool(), auth_user.user_id).await?;
    let active_count = existing.iter().filter(|k| k.is_active).count();
    if active_count >= 1 {
        return Err(AppError::Validation(
            "you can only have one active API key at a time. Revoke your existing key first.".into(),
        ));
    }

    let expires_at = body
        .expires_in_days
        .map(|days| Utc::now() + Duration::days(days as i64));

    let (full_key, prefix, key_hash) = crate::utils::auth::api_keys::generate_api_key();

    let data = CreateApiKeyData {
        user_id: Some(auth_user.user_id),
        name: body.name.clone(),
        key_prefix: prefix.clone(),
        key_hash,
        scopes: body.scopes.clone(),
        expires_at,
    };

    let api_key = ApiKeyRepository::create(state.db.pool(), data).await?;

    info!(
        "API key created: {} for user {}",
        body.name, auth_user.user_id
    );

    Ok(Json(ApiKeyCreatedResponse {
        id: api_key.id,
        name: body.name,
        full_key,
        prefix,
        scopes: body.scopes,
        expires_at,
    }))
}

pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> AppResult<Json<Vec<ApiKeyDto>>> {
    let keys = ApiKeyRepository::list_by_user(state.db.pool(), auth_user.user_id).await?;

    Ok(Json(keys.into_iter().map(ApiKeyDto::from).collect()))
}

/// POST /api/api-keys/revoke  —  revoke own key by id in body
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<RevokeOwnApiKeyRequest>,
) -> AppResult<Json<MessageResponse>> {
    let id = Uuid::parse_str(&body.id)
        .map_err(|_| AppError::BadRequest("invalid key id".into()))?;

    let key = ApiKeyRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound("API key not found".into()))?;

    if key.user_id != Some(auth_user.user_id) {
        return Err(AppError::Forbidden("you do not own this API key".into()));
    }

    ApiKeyRepository::deactivate(state.db.pool(), id).await?;
    info!("API key revoked: {} by user {}", key.name, auth_user.user_id);

    Ok(Json(MessageResponse {
        message: format!("API key '{}' has been revoked", key.name),
    }))
}

/// POST /api/api-keys/regenerate  —  revoke current active key, create a new one
pub async fn regenerate_api_key(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> AppResult<Json<ApiKeyCreatedResponse>> {
    check_api_key_allowed(&state, &auth_user).await?;
    // Prevent scope escalation: regenerate always issues files:read + files:write,
    // so it must only be callable from a UI session, never from an API key that
    // holds a narrower scope.
    auth_user.require_ui_session()?;

    // Find and deactivate any existing active key
    let existing = ApiKeyRepository::list_by_user(state.db.pool(), auth_user.user_id).await?;
    if let Some(old_key) = existing.iter().find(|k| k.is_active) {
        let old_id = old_key.id;
        ApiKeyRepository::deactivate(state.db.pool(), old_id).await?;
        info!(
            "Old API key '{}' deactivated during regenerate for user {}",
            old_key.name, auth_user.user_id
        );
    }

    let (full_key, prefix, key_hash) = crate::utils::auth::api_keys::generate_api_key();

    let data = CreateApiKeyData {
        user_id: Some(auth_user.user_id),
        name: format!("Personal Key {}", Utc::now().format("%Y-%m-%d")),
        key_prefix: prefix.clone(),
        key_hash,
        scopes: vec!["files:read".into(), "files:write".into()],
        expires_at: None,
    };

    let api_key = ApiKeyRepository::create(state.db.pool(), data).await?;

    info!(
        "API key regenerated for user {}",
        auth_user.user_id
    );

    Ok(Json(ApiKeyCreatedResponse {
        id: api_key.id,
        name: api_key.name.clone(),
        full_key,
        prefix,
        scopes: vec!["files:read".into(), "files:write".into()],
        expires_at: None,
    }))
}
