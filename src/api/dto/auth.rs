use chrono::{DateTime, Utc};
use crate::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(length(min = 3, max = 50))]
    pub username: String,
    #[validate(email)]
    pub email: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: u64,
    pub user: UserDto,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    /// Optional for backward compatibility with API clients. Browser sessions
    /// send the token in the `keystone_refresh` httpOnly cookie instead.
    pub refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// A user creating an API key for their own account (the account page).
#[derive(Debug, Deserialize)]
pub struct CreateUserApiKeyRequest {
    pub name: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub expires_in_days: Option<u32>,
}

/// One of the caller's own API keys. Excludes the key hash and the owner id
/// (implicit); includes everything the account page needs to render it.
#[derive(Debug, Serialize)]
pub struct UserApiKeyDto {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

#[derive(Debug, Serialize)]
pub struct AccountPermissionsDto {
    pub allow_api_keys: bool,
    pub allow_password_change: bool,
    pub allow_bots: bool,
}

#[derive(Debug, Serialize)]
pub struct UserDto {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl From<User> for UserDto {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            email: user.email,
            role: user.role.to_string(),
            created_at: user.created_at,
        }
    }
}
