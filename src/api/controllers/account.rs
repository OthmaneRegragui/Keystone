use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::rows::CreateApiKeyData;
use crate::db::repos::{
    AdminSettingRepository, ApiKeyRepository, GroupRepository,
};
use crate::api::extractors::AuthUser;
use crate::api::validators::validate_scopes;
use crate::dto::*;
use tracing::info;
use uuid::Uuid;

use crate::AppState;

/// Whether the authenticated user is allowed to manage their own API keys.
/// Admins always may; everyone else follows the group policy (ANY group with
/// `allow_api_keys`), or the global `allow_user_api_keys` setting when they
/// are in no group.
async fn require_api_keys_enabled(state: &AppState, auth: &AuthUser) -> AppResult<()> {
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
            "API keys are not enabled for your account".into(),
        ));
    }
    Ok(())
}

/// GET /api/api-keys — the caller's own API keys. Reading one's own key list
/// is harmless with an API key; creating/revoking keys always requires a UI
/// session.
pub async fn list_my_api_keys(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<UserApiKeyDto>>> {
    require_api_keys_enabled(&state, &auth).await?;
    let keys = ApiKeyRepository::list_by_user(state.db.pool(), auth.user_id).await?;
    let dtos = keys
        .into_iter()
        .map(|k| UserApiKeyDto {
            id: k.id.to_string(),
            name: k.name,
            prefix: k.key_prefix,
            scopes: k.scopes,
            last_used_at: k.last_used_at,
            expires_at: k.expires_at,
            created_at: k.created_at,
            is_active: k.is_active,
        })
        .collect();
    Ok(Json(dtos))
}

/// POST /api/api-keys — create an API key for the caller's own account.
pub async fn create_user_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateUserApiKeyRequest>,
) -> AppResult<Json<serde_json::Value>> {
    // Key minting must be a browser UI action: a leaked automation key must
    // never be able to mint sibling keys (which would escalate a read-only
    // leak into full access).
    auth.require_ui_session()?;
    require_api_keys_enabled(&state, &auth).await?;

    if body.name.trim().is_empty() || body.name.chars().count() > 100 {
        return Err(AppError::BadRequest("key name must be 1..=100 characters".into()));
    }
    if body.scopes.len() > 10 || !validate_scopes(&body.scopes) {
        return Err(AppError::BadRequest(
            "one or more scopes are not allowed".into(),
        ));
    }
    // Cap the duration: chrono's Duration overflows (panics) for huge day
    // counts (same guard as the admin endpoint).
    if body.expires_in_days.is_some_and(|d| d > 3650) {
        return Err(AppError::BadRequest(
            "expires_in_days must not exceed 3650".into(),
        ));
    }

    let expires_at = body
        .expires_in_days
        .map(|days| chrono::Utc::now() + chrono::Duration::days(days as i64));

    let (full_key, prefix, key_hash) = crate::utils::auth::api_keys::generate_api_key();

    let api_key = ApiKeyRepository::create(
        state.db.pool(),
        CreateApiKeyData {
            user_id: Some(auth.user_id),
            name: body.name.clone(),
            key_prefix: prefix.clone(),
            key_hash,
            scopes: body.scopes.clone(),
            expires_at,
        },
    )
    .await?;

    info!("user {} created API key '{}'", auth.username, body.name);
    Ok(Json(serde_json::json!({
        "id": api_key.id.to_string(),
        "name": body.name,
        "full_key": full_key,
        "prefix": prefix,
        "scopes": body.scopes,
        "expires_at": expires_at,
    })))
}

/// DELETE /api/api-keys/:id — permanently delete one of the caller's own keys.
pub async fn delete_user_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_ui_session()?;
    require_api_keys_enabled(&state, &auth).await?;

    let key = ApiKeyRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound("API key not found".into()))?;
    if key.user_id != Some(auth.user_id) {
        return Err(AppError::NotFound("API key not found".into()));
    }

    ApiKeyRepository::delete(state.db.pool(), id).await?;
    info!("user {} deleted API key '{}'", auth.username, key.name);
    Ok(Json(MessageResponse {
        message: "API key deleted".to_string(),
    }))
}
