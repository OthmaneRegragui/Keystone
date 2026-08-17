use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::Json;
use crate::utils::auth::password::{hash_password, verify_password};
use crate::utils::auth::session::REFRESH_COOKIE_NAME;
use crate::error::{AppError, AppResult};
use crate::models::User;
use crate::db::repos::{AdminSettingRepository, GroupRepository, UserRepository};
use tracing::info;
use validator::Validate;

use crate::dto::*;
use crate::AppState;

/// Build the `Set-Cookie` header that stores the refresh token in an httpOnly
/// cookie, out of reach of page JavaScript. `SameSite=Lax` prevents cross-site
/// requests from carrying it (and the refresh endpoint is POST-only, so Lax
/// never forwards it on top-level GET navigations). `Secure` is set only in
/// production, where the deployment is expected to be behind TLS.
fn refresh_cookie(token: &str, max_age_secs: u64) -> String {
    let secure = if crate::error::is_production() {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{REFRESH_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_secs}{secure}"
    )
}

/// Expire the refresh cookie (used on logout).
fn clear_refresh_cookie() -> String {
    format!("{REFRESH_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

/// Extract a cookie value by name from a `Cookie` header.
fn parse_cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let pair = pair.trim();
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name).then(|| v.trim())
    })
}

/// Resolve the refresh token presented by a request: the httpOnly cookie for
/// browser sessions, falling back to the JSON body for API clients that still
/// manage the token themselves.
fn resolve_refresh_token(headers: &HeaderMap, body: Option<&RefreshRequest>) -> Option<String> {
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| parse_cookie(c, REFRESH_COOKIE_NAME))
    {
        return Some(cookie.to_string());
    }
    body.and_then(|b| b.refresh_token.clone())
}

fn build_auth_response(
    user: User,
    access_token: String,
    refresh_token: String,
    expires_in: u64,
) -> AuthResponse {
    AuthResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in,
        user: UserDto::from(user),
    }
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterRequest>,
) -> AppResult<(HeaderMap, Json<AuthResponse>)> {
    // Normalize before validation so " Bob@X.com " and "bob@x.com" are the
    // same account (Postgres UNIQUE is case-sensitive; case-variant accounts
    // would otherwise enable squatting/confusion).
    let mut body = body;
    body.email = body.email.trim().to_lowercase();

    // Validate input BEFORE the registration-policy check so invalid input
    // always gets 422 (regardless of whether the instance has users yet) and
    // the "registrations disabled" state is not leaked via 403 to garbage
    // input.
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    crate::utils::names::validate_username(&body.username)
        .map_err(AppError::Validation)?;

    let block = AdminSettingRepository::get_bool(state.db.pool(), "block_registrations")
        .await
        .unwrap_or(true);
    if block {
        // Propagate the error instead of `unwrap_or(0)`: a DB failure must not
        // be treated as "no users yet" (that would hand out the admin role).
        let user_count = UserRepository::count(state.db.pool()).await?;
        if user_count > 0 {
            return Err(AppError::Forbidden(
                "new registrations are currently disabled".into(),
            ));
        }
    }

    let password_hash = hash_password(&body.password)?;

    // The "first registered user becomes admin" decision happens inside a
    // transaction guarded by a Postgres advisory lock (see
    // UserRepository::create_with_bootstrap_role), so two concurrent
    // registrations on a fresh install cannot both become admin.
    let user = UserRepository::create_with_bootstrap_role(
        state.db.pool(),
        crate::db::rows::CreateUserData {
            username: body.username,
            email: body.email,
            password_hash,
            role: crate::models::UserRole::User,
            storage_quota: 1_073_741_824,
        },
    )
    .await?;

    let access_token = state.jwt_service.create_token(user.id, &user.role.to_string())?;
    let (refresh_token, _) = state
        .session_service
        .create_refresh_token(user.id)
        .await?;

    info!("user registered: {}", user.username);

    let headers = HeaderMap::from_iter([(
        header::SET_COOKIE,
        refresh_cookie(&refresh_token, state.session_service.expiry_seconds())
            .parse()
            .expect("refresh cookie header is valid"),
    )]);

    Ok((headers, Json(build_auth_response(
        user,
        access_token,
        refresh_token,
        state.config.auth.jwt_expiration_secs as u64,
    ))))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> AppResult<(HeaderMap, Json<AuthResponse>)> {
    // Reject absurdly large inputs up front (uniform 401 so the check itself
    // does not become an oracle). Argon2 cost scales with the input size, and
    // emails are bound straight into SQL.
    if body.email.len() > 320 || body.password.len() > 1024 {
        return Err(AppError::Unauthorized(
            "invalid email or password".into(),
        ));
    }

    let email = body.email.trim().to_lowercase();

    let user = match UserRepository::find_by_email(state.db.pool(), &email).await? {
        Some(user) => user,
        None => {
            // Timing-equalization: run a full Argon2 computation so "unknown
            // email" takes as long as "known email, wrong password" and the
            // login endpoint does not leak which emails exist.
            let _ = hash_password(&body.password)?;
            return Err(AppError::Unauthorized(
                "invalid email or password".into(),
            ));
        }
    };

    let valid = verify_password(&body.password, &user.password_hash)?;
    if !valid {
        return Err(AppError::Unauthorized(
            "invalid email or password".into(),
        ));
    }

    UserRepository::update_last_login(state.db.pool(), user.id).await?;

    let access_token = state.jwt_service.create_token(user.id, &user.role.to_string())?;
    let (refresh_token, _) = state
        .session_service
        .create_refresh_token(user.id)
        .await?;

    info!("user logged in: {}", user.username);

    let headers = HeaderMap::from_iter([(
        header::SET_COOKIE,
        refresh_cookie(&refresh_token, state.session_service.expiry_seconds())
            .parse()
            .expect("refresh cookie header is valid"),
    )]);

    Ok((headers, Json(build_auth_response(
        user,
        access_token,
        refresh_token,
        state.config.auth.jwt_expiration_secs as u64,
    ))))
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<RefreshRequest>>,
) -> AppResult<(HeaderMap, Json<AuthResponse>)> {
    let presented = resolve_refresh_token(&headers, body.as_deref())
        .ok_or_else(|| AppError::Unauthorized("missing refresh token".into()))?;

    let refresh = state
        .session_service
        .validate_refresh_token(&presented)
        .await?;

    let user = UserRepository::find_by_id(state.db.pool(), refresh.user_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    let access_token = state.jwt_service.create_token(user.id, &user.role.to_string())?;
    let (new_refresh, _) = state
        .session_service
        .rotate_token(&presented)
        .await?;

    let headers = HeaderMap::from_iter([(
        header::SET_COOKIE,
        refresh_cookie(&new_refresh, state.session_service.expiry_seconds())
            .parse()
            .expect("refresh cookie header is valid"),
    )]);

    Ok((headers, Json(build_auth_response(
        user,
        access_token,
        new_refresh,
        state.config.auth.jwt_expiration_secs as u64,
    ))))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<RefreshRequest>>,
) -> AppResult<(HeaderMap, Json<MessageResponse>)> {
    if let Some(presented) = resolve_refresh_token(&headers, body.as_deref()) {
        // A presented refresh token must be valid — refusing a bogus one (401)
        // prevents the endpoint from being used as an oracle or silently
        // "succeeding" a logout that never happened.
        let refresh = state
            .session_service
            .validate_refresh_token(&presented)
            .await?;
        state.session_service.revoke_token(refresh.id).await?;
    }

    let headers = HeaderMap::from_iter([(
        header::SET_COOKIE,
        clear_refresh_cookie().parse().expect("clear cookie header is valid"),
    )]);

    Ok((headers, Json(MessageResponse {
        message: "logged out successfully".to_string(),
    })))
}

pub async fn forgot_password(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForgotPasswordRequest>,
) -> AppResult<Json<MessageResponse>> {
    // Always return the same response, whether user exists or not
    if body.email.len() <= 320 {
        if let Ok(Some(user)) = UserRepository::find_by_email(state.db.pool(), &body.email).await {
            // TODO: Generate and store a proper reset token, send email
            info!("password reset requested for user_id={}", user.id);
        } else {
            info!("password reset requested for unknown email");
        }
    } else {
        info!("password reset requested for unknown email");
    }

    Ok(Json(MessageResponse {
        message: "if the email exists, a reset link has been sent".to_string(),
    }))
}

pub async fn reset_password(
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<ResetPasswordRequest>,
) -> AppResult<Json<MessageResponse>> {
    // Password reset via token is not yet implemented
    Err(AppError::BadRequest(
        "password reset via token is not yet implemented".into(),
    ))
}

pub async fn change_password(
    State(state): State<Arc<AppState>>,
    auth: crate::api::extractors::AuthUser,
    Json(body): Json<ChangePasswordRequest>,
) -> AppResult<Json<MessageResponse>> {
    // Credential management requires a browser UI session; an API key must not
    // be able to rotate account credentials.
    auth.require_ui_session()?;
    // Admins always allowed. Users with at least one group are governed by
    // their groups' `allow_password_change` flag (ANY group allows); users
    // with no group fall back to the global `allow_user_password_change` setting.
    if !auth.is_admin() {
        let user_id = auth.user_id.to_string();
        let user_groups = GroupRepository::list_user_groups(state.db.pool(), &user_id).await?;
        let allowed = if user_groups.is_empty() {
            AdminSettingRepository::get_bool(state.db.pool(), "allow_user_password_change").await?
        } else {
            GroupRepository::user_allows_password_change(state.db.pool(), &user_id).await?
        };
        if !allowed {
            return Err(AppError::Forbidden(
                "Password change is not enabled for your account. Contact an administrator.".into(),
            ));
        }
    }

    // Consistent with register validation (8..=128 chars). Argon2 cost scales
    // with input size, so also cap the current password.
    let new_len = body.new_password.chars().count();
    if new_len < 8 {
        return Err(AppError::Validation("new password must be at least 8 characters".into()));
    }
    if new_len > 128 {
        return Err(AppError::Validation("new password must be at most 128 characters".into()));
    }
    if body.current_password.len() > 1024 {
        return Err(AppError::Validation("current password is too long".into()));
    }
    if body.new_password == body.current_password {
        return Err(AppError::Validation(
            "new password must be different from the current password".into(),
        ));
    }

    let user = UserRepository::find_by_id(state.db.pool(), auth.user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    let valid = verify_password(&body.current_password, &user.password_hash)?;
    if !valid {
        return Err(AppError::Unauthorized("current password is incorrect".into()));
    }

    let new_hash = hash_password(&body.new_password)?;
    UserRepository::update_password_hash(state.db.pool(), auth.user_id, &new_hash).await?;

    // Invalidate all of the user's refresh tokens so a stolen session does not
    // survive the password change.
    state.session_service.revoke_all_for_user(auth.user_id).await;

    info!("user {} changed their password", user.username);
    Ok(Json(MessageResponse { message: "password updated successfully".to_string() }))
}

/// Effective capability flags for the authenticated user, so the account UI can
/// show/hide the API key and password change sections correctly.
/// Admins always have all three. Non-admins follow the group policy when they
/// belong to at least one group (ANY group allows), otherwise the global
/// setting.
pub async fn account_permissions(
    State(state): State<Arc<AppState>>,
    auth: crate::api::extractors::AuthUser,
) -> AppResult<Json<AccountPermissionsDto>> {
    let (allow_api_keys, allow_password_change, allow_bots) = if auth.is_admin() {
        (true, true, true)
    } else {
        let user_id = auth.user_id.to_string();
        let user_groups = GroupRepository::list_user_groups(state.db.pool(), &user_id).await?;
        if user_groups.is_empty() {
            (
                AdminSettingRepository::get_bool(state.db.pool(), "allow_user_api_keys").await?,
                AdminSettingRepository::get_bool(state.db.pool(), "allow_user_password_change").await?,
                AdminSettingRepository::get_bool(state.db.pool(), "allow_user_bots").await?,
            )
        } else {
            (
                GroupRepository::user_allows_api_keys(state.db.pool(), &user_id).await?,
                GroupRepository::user_allows_password_change(state.db.pool(), &user_id).await?,
                GroupRepository::user_allows_bots(state.db.pool(), &user_id).await?,
            )
        }
    };
    Ok(Json(AccountPermissionsDto {
        allow_api_keys,
        allow_password_change,
        allow_bots,
    }))
}
