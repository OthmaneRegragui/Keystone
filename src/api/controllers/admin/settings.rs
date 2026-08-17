use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::AdminSettingRepository;
use tracing::info;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<PlatformSettingsDto>> {
    auth.require_admin()?;
    let settings = AdminSettingRepository::get_platform_settings(state.db.pool()).await?;
    Ok(Json(settings))
}

pub async fn update_setting(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateSettingRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    match body.key.as_str() {
        "block_registrations" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "block_registrations", val).await?;
            info!("admin {} set block_registrations={}", auth.username, val);
        }
        "allow_user_api_keys" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_api_keys", val).await?;
            info!("admin {} set allow_user_api_keys={}", auth.username, val);
        }
        "allow_user_password_change" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_password_change", val).await?;
            info!("admin {} set allow_user_password_change={}", auth.username, val);
        }
        "allow_user_bots" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_bots", val).await?;
            info!("admin {} set allow_user_bots={}", auth.username, val);
        }
        _ => {
            return Err(AppError::BadRequest(format!("unknown setting key: {}", body.key)));
        }
    }
    Ok(Json(MessageResponse { message: "setting updated".to_string() }))
}
