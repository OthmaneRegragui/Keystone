use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use chrono::{Duration, Utc};
use crate::error::{AppError, AppResult};
use crate::models::{Bot, BotPathRule};
use crate::db::rows::{CreateBotData, CreateApiKeyData};
use crate::db::repos::{
    AdminSettingRepository, ApiKeyRepository, BotRepository, BucketRepository, GroupRepository,
    UserRepository,
};
use crate::utils::names::validate_component_name;
use tracing::info;
use uuid::Uuid;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

const MAX_PATH_RULES: usize = 100;
const MAX_RULE_PATH_LEN: usize = 1024;

/// Build an `AdminBotDto` from a `Bot`, joining the owner's username and the
/// key's prefix/expiry/status.
async fn admin_bot_dto(state: &AppState, bot: Bot) -> AppResult<AdminBotDto> {
    let username = UserRepository::find_by_id(state.db.pool(), bot.user_id)
        .await?
        .map(|u| u.username)
        .unwrap_or_else(|| "unknown".into());

    let key = ApiKeyRepository::find_by_id(state.db.pool(), bot.key_id).await?;
    let (prefix, expires_at, is_active) = match key {
        Some(k) => (k.key_prefix, k.expires_at, k.is_active),
        None => (String::new(), None, false),
    };

    Ok(AdminBotDto {
        id: bot.id.to_string(),
        user_id: bot.user_id.to_string(),
        username,
        key_id: bot.key_id.to_string(),
        prefix,
        name: bot.name,
        can_upload: bot.can_upload,
        can_download: bot.can_download,
        can_copy: bot.can_copy,
        can_edit: bot.can_edit,
        can_delete: bot.can_delete,
        can_list: bot.can_list,
        path_rules: bot.path_rules,
        upload_limit_bytes: bot.upload_limit_bytes,
        uploaded_bytes: bot.uploaded_bytes,
        expires_at,
        is_active,
        created_at: bot.created_at,
        updated_at: bot.updated_at,
    })
}

/// Normalize a rule path for storage: trim trailing slashes so matching is
/// unambiguous (`/a/` and `/a` are the same rule). `""` (empty) stays empty.
fn normalize_rule_path(path: &str) -> String {
    path.trim_end_matches('/').to_string()
}

/// Validate path rules against the owning user so a bot never points at
/// buckets/paths the owner itself cannot reach, and the paths are well-formed.
async fn validate_path_rules(
    state: &AppState,
    owner_id: Uuid,
    rules: &Option<Vec<BotPathRule>>,
) -> AppResult<()> {
    let Some(rules) = rules else {
        return Ok(());
    };
    if rules.len() > MAX_PATH_RULES {
        return Err(AppError::BadRequest(format!(
            "too many path rules (maximum {MAX_PATH_RULES})"
        )));
    }

    let accessible = BucketRepository::list_accessible_to_user(state.db.pool(), &owner_id.to_string())
        .await?;

    for rule in rules {
        if rule.path.len() > MAX_RULE_PATH_LEN {
            return Err(AppError::BadRequest(format!(
                "path too long for bucket '{}'",
                rule.bucket
            )));
        }
        if !accessible.iter().any(|b| b.name == rule.bucket) {
            return Err(AppError::BadRequest(format!(
                "bucket '{}' is not accessible to the owner",
                rule.bucket
            )));
        }
        let path = normalize_rule_path(&rule.path);
        if path.is_empty() {
            continue;
        }
        if !path.starts_with('/') {
            return Err(AppError::BadRequest(format!(
                "path '{}' must be empty or start with '/'",
                rule.path
            )));
        }
        // Split after the leading '/' and reject empty ("//") and reserved
        // (".", "..") segments. `validate_component_name` additionally rejects
        // separators, control characters and OS-hostile names.
        let parts: Vec<&str> = path.split('/').collect();
        for part in parts.iter().skip(1) {
            if part.is_empty() {
                return Err(AppError::BadRequest(format!(
                    "path '{}' contains an empty segment",
                    rule.path
                )));
            }
            validate_component_name(part)
                .map_err(|e| AppError::BadRequest(format!("invalid path '{}': {e}", rule.path)))?;
        }
    }
    Ok(())
}

/// Normalize every rule in the list (see `normalize_rule_path`).
fn normalize_rules(rules: Option<Vec<BotPathRule>>) -> Option<Vec<BotPathRule>> {
    rules.map(|list| {
        list.into_iter()
            .map(|r| BotPathRule {
                bucket: r.bucket,
                path: normalize_rule_path(&r.path),
                status: r.status,
            })
            .collect()
    })
}

/// Whether the caller is allowed to manage bots. Admins always may; a regular
/// user may when they are in a group with `allow_bots`, or — with no groups —
/// when the platform setting `allow_user_bots` is enabled. Returns `true` for
/// admin (full access) and `false` for an eligible user (own bots only).
/// Bot management is restricted to browser UI sessions (JWT); API-key
/// authenticated requests can never create or modify bots.
async fn bot_manage_scope(state: &AppState, auth: &AuthUser) -> AppResult<bool> {
    auth.require_ui_session()?;
    if auth.is_admin() {
        return Ok(true);
    }
    let user_id = auth.user_id.to_string();
    let user_groups = GroupRepository::list_user_groups(state.db.pool(), &user_id).await?;
    let allowed = if user_groups.is_empty() {
        AdminSettingRepository::get_bool(state.db.pool(), "allow_user_bots").await?
    } else {
        GroupRepository::user_allows_bots(state.db.pool(), &user_id).await?
    };
    if !allowed {
        return Err(AppError::Forbidden(
            "bots are not enabled for your account".into(),
        ));
    }
    Ok(false)
}

/// GET /api/admin/bots — list every bot with owner info.
pub async fn list_bots(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<AdminBotDto>>> {
    let is_admin = bot_manage_scope(&state, &auth).await?;
    let bots = if is_admin {
        BotRepository::list_all(state.db.pool()).await?
    } else {
        BotRepository::list_by_user(state.db.pool(), auth.user_id).await?
    };
    let mut dtos = Vec::with_capacity(bots.len());
    for bot in bots {
        dtos.push(admin_bot_dto(&state, bot).await?);
    }
    Ok(Json(dtos))
}

/// POST /api/admin/bots — create a bot. Admins may create for any user; an
/// eligible user may only create bots for themself.
pub async fn create_bot(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateAdminBotRequest>,
) -> AppResult<Json<serde_json::Value>> {
    let is_admin = bot_manage_scope(&state, &auth).await?;
    if body.name.trim().is_empty() || body.name.chars().count() > 100 {
        return Err(AppError::BadRequest("bot name must be 1..=100 characters".into()));
    }
    if body.upload_limit_bytes < 0 {
        return Err(AppError::BadRequest("upload_limit_bytes must be >= 0".into()));
    }
    if body.expires_in_days.is_some_and(|d| d > 3650) {
        return Err(AppError::BadRequest("expires_in_days must not exceed 3650".into()));
    }

    let owner_id = if is_admin {
        let owner_id = Uuid::parse_str(&body.user_id)
            .map_err(|_| AppError::BadRequest("invalid user id".into()))?;
        let _ = UserRepository::find_by_id(state.db.pool(), owner_id)
            .await?
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
        owner_id
    } else {
        auth.user_id
    };

    validate_path_rules(&state, owner_id, &body.path_rules).await?;
    let path_rules = normalize_rules(body.path_rules);

    let expires_at = body
        .expires_in_days
        .map(|days| Utc::now() + Duration::days(days as i64));

    let (full_key, _, key_hash) = crate::utils::auth::api_keys::generate_api_key();
    let prefix = format!("bot_{}", &Uuid::new_v4().to_string()[..8]);

    let scopes = {
        let b = Bot {
            id: Uuid::new_v4(),
            user_id: owner_id,
            key_id: Uuid::new_v4(),
            name: body.name.clone(),
            can_upload: body.can_upload,
            can_download: body.can_download,
            can_copy: body.can_copy,
            can_edit: body.can_edit,
            can_delete: body.can_delete,
            can_list: body.can_list,
            path_rules: path_rules.clone(),
            upload_limit_bytes: body.upload_limit_bytes,
            uploaded_bytes: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        b.scopes()
    };

    let api_key = ApiKeyRepository::create(
        state.db.pool(),
        CreateApiKeyData {
            user_id: Some(owner_id),
            name: body.name.clone(),
            key_prefix: prefix.clone(),
            key_hash,
            scopes,
            expires_at,
        },
    )
    .await?;

    let bot = BotRepository::create(
        state.db.pool(),
        CreateBotData {
            user_id: owner_id,
            key_id: api_key.id,
            name: body.name.clone(),
            can_upload: body.can_upload,
            can_download: body.can_download,
            can_copy: body.can_copy,
            can_edit: body.can_edit,
            can_delete: body.can_delete,
            can_list: body.can_list,
            path_rules,
            upload_limit_bytes: body.upload_limit_bytes,
        },
    )
    .await?;

    info!("user {} created bot '{}' for user {}", auth.username, body.name, owner_id);

    let dto = admin_bot_dto(&state, bot).await?;
    Ok(Json(serde_json::json!({
        "full_key": full_key,
        "prefix": prefix,
        "expires_at": expires_at,
        "bot": dto,
    })))
}

/// PUT /api/admin/bots/:id — update any bot's configuration (admin) or one of
/// the caller's own bots (eligible user).
pub async fn update_bot(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<AdminUpdateBotRequest>,
) -> AppResult<Json<AdminBotDto>> {
    let is_admin = bot_manage_scope(&state, &auth).await?;
    if body.upload_limit_bytes.is_some_and(|v| v < 0) {
        return Err(AppError::BadRequest("upload_limit_bytes must be >= 0".into()));
    }
    if let Some(name) = &body.name {
        if name.trim().is_empty() || name.chars().count() > 100 {
            return Err(AppError::BadRequest("bot name must be 1..=100 characters".into()));
        }
    }

    let existing = if is_admin {
        BotRepository::find_by_id(state.db.pool(), id).await?
    } else {
        BotRepository::find_by_user_and_id(state.db.pool(), auth.user_id, id).await?
    }
    .ok_or_else(|| AppError::NotFound("bot not found".into()))?;

    let path_rules = body.path_rules.clone().flatten();
    if path_rules.is_some() {
        validate_path_rules(&state, existing.user_id, &path_rules).await?;
    }

    let bot = BotRepository::update(
        state.db.pool(),
        existing.user_id,
        id,
        crate::db::repos::bots::UpdateBotData {
            name: body.name,
            can_upload: body.can_upload,
            can_download: body.can_download,
            can_copy: body.can_copy,
            can_edit: body.can_edit,
            can_delete: body.can_delete,
            can_list: body.can_list,
            path_rules: body.path_rules.map(normalize_rules),
            upload_limit_bytes: body.upload_limit_bytes,
        },
    )
    .await?
    .ok_or_else(|| AppError::NotFound("bot not found".into()))?;

    ApiKeyRepository::update_scopes(state.db.pool(), bot.key_id, &bot.scopes()).await?;

    info!("user {} updated bot '{}'", auth.username, bot.name);
    Ok(Json(admin_bot_dto(&state, bot).await?))
}

/// DELETE /api/admin/bots/:id — delete a bot and its API key (admin may delete
/// any bot; an eligible user only their own).
pub async fn delete_bot(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    let is_admin = bot_manage_scope(&state, &auth).await?;
    let bot = if is_admin {
        BotRepository::find_by_id(state.db.pool(), id).await?
    } else {
        BotRepository::find_by_user_and_id(state.db.pool(), auth.user_id, id).await?
    }
    .ok_or_else(|| AppError::NotFound("bot not found".into()))?;

    let _ = ApiKeyRepository::delete(state.db.pool(), bot.key_id).await;
    BotRepository::delete(state.db.pool(), bot.id).await?;

    info!("user {} deleted bot '{}'", auth.username, bot.name);
    Ok(Json(MessageResponse {
        message: format!("bot '{}' deleted", bot.name),
    }))
}
