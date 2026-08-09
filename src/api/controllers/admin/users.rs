use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::utils::auth::password::hash_password;
use crate::utils::names::validate_username;
use crate::error::{AppError, AppResult};
use crate::models::UserRole;
use crate::db::repos::{GroupRepository, UserRepository};
use tracing::info;
use uuid::Uuid;
use validator::ValidateEmail;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct UpdateUserBody {
    pub id: String,
    pub email: Option<String>,
    pub role: Option<String>,
    pub password: Option<String>,
    pub group_ids: Option<Vec<String>>,
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<AdminUserDto>>> {
    auth.require_admin()?;
    let users = UserRepository::list(state.db.pool(), 0, 200).await?;
    let mut dtos = Vec::new();
    for u in users {
        let group_ids = GroupRepository::list_user_groups(state.db.pool(), &u.id.to_string()).await.unwrap_or_default();
        dtos.push(AdminUserDto {
            id: u.id.to_string(),
            username: u.username,
            email: u.email,
            role: u.role.to_string(),
            storage_quota: u.storage_quota,
            storage_used: u.storage_used,
            created_at: u.created_at,
            group_ids,
        });
    }
    Ok(Json(dtos))
}

pub async fn get_user(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> AppResult<Json<AdminUserDto>> {
    auth.require_admin()?;
    let id = params.get("id").ok_or_else(|| AppError::BadRequest("missing id param".into()))?;
    let uid = Uuid::parse_str(id).map_err(|_| AppError::BadRequest("invalid user id".into()))?;
    let u = UserRepository::find_by_id(state.db.pool(), uid)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;
    let group_ids = GroupRepository::list_user_groups(state.db.pool(), id).await.unwrap_or_default();
    Ok(Json(AdminUserDto {
        id: u.id.to_string(),
        username: u.username,
        email: u.email,
        role: u.role.to_string(),
        storage_quota: u.storage_quota,
        storage_used: u.storage_used,
        created_at: u.created_at,
        group_ids,
    }))
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateAdminUserRequest>,
) -> AppResult<Json<AdminUserDto>> {
    auth.require_admin()?;
    // Mirror the public registration policy so the admin endpoint cannot
    // bypass it: username 3..=50 chars, valid email, password 8..=128.
    if body.username.len() < 3 || body.username.len() > 50 {
        return Err(AppError::Validation(
            "username must be between 3 and 50 characters".into(),
        ));
    }
    validate_username(&body.username).map_err(AppError::Validation)?;
    if !body.email.validate_email() {
        return Err(AppError::Validation("invalid email address".into()));
    }
    if body.password.len() < 8 || body.password.len() > 128 {
        return Err(AppError::Validation(
            "password must be between 8 and 128 characters".into(),
        ));
    }
    let password_hash = hash_password(&body.password)?;
    let role: UserRole = body.role.parse().map_err(|_| AppError::BadRequest("invalid role".into()))?;
    let user = UserRepository::create(
        state.db.pool(),
        crate::db::rows::CreateUserData {
            username: body.username,
            email: body.email,
            password_hash,
            role,
            storage_quota: 0,
        },
    )
    .await?;
    if !body.group_ids.is_empty() {
        for gid in &body.group_ids {
            Uuid::parse_str(gid)
                .map_err(|_| AppError::BadRequest(format!("invalid group id '{gid}'")))?;
        }
        let gid_strs: Vec<String> = body.group_ids.iter().map(|g| g.clone()).collect();
        GroupRepository::set_user_groups(state.db.pool(), &user.id.to_string(), &gid_strs).await?;
    }
    let group_ids = GroupRepository::list_user_groups(state.db.pool(), &user.id.to_string()).await.unwrap_or_default();
    info!("admin {} created user {}", auth.username, user.username);
    Ok(Json(AdminUserDto {
        id: user.id.to_string(),
        username: user.username,
        email: user.email,
        role: user.role.to_string(),
        storage_quota: user.storage_quota,
        storage_used: user.storage_used,
        created_at: user.created_at,
        group_ids,
    }))
}

pub async fn update_user(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateUserBody>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    let uid = Uuid::parse_str(&body.id).map_err(|_| AppError::BadRequest("invalid user id".into()))?;

    if let Some(email) = body.email.as_deref() {
        if !email.validate_email() {
            return Err(AppError::Validation("invalid email address".into()));
        }
    }

    let password_hash = match &body.password {
        Some(p) => {
            if p.len() < 8 || p.len() > 128 {
                return Err(AppError::Validation(
                    "password must be between 8 and 128 characters".into(),
                ));
            }
            Some(hash_password(p)?)
        }
        None => None,
    };
    let role_str = match body.role.as_deref() {
        Some("admin") | Some("user") | Some("service") => body.role.as_deref(),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "invalid role '{other}' (expected 'admin', 'user' or 'service')"
            )));
        }
        None => None,
    };

    UserRepository::update_user(
        state.db.pool(),
        uid,
        body.email.as_deref(),
        role_str,
        password_hash.as_deref(),
    )
    .await?;

    if let Some(ref groups) = body.group_ids {
        for gid in groups {
            Uuid::parse_str(gid)
                .map_err(|_| AppError::BadRequest(format!("invalid group id '{gid}'")))?;
        }
        let gid_strs: Vec<String> = groups.iter().cloned().collect();
        GroupRepository::set_user_groups(state.db.pool(), &body.id, &gid_strs).await?;
    }

    info!("admin {} updated user {}", auth.username, body.id);
    Ok(Json(MessageResponse { message: "user updated".to_string() }))
}

pub async fn update_user_quota(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateUserQuotaRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    let uid = Uuid::parse_str(&body.user_id)
        .map_err(|_| AppError::BadRequest("invalid user id".into()))?;
    if body.storage_quota < 0 {
        return Err(AppError::BadRequest(
            "storage quota cannot be negative".into(),
        ));
    }
    UserRepository::update_storage_quota(state.db.pool(), uid, body.storage_quota).await?;
    info!("admin {} set quota for user {} to {} bytes", auth.username, body.user_id, body.storage_quota);
    Ok(Json(MessageResponse { message: "storage quota updated".to_string() }))
}
