use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use crate::error::{AppError, AppResult};
use crate::models::{Bot, UserRole};
use crate::db::repos::{ApiKeyRepository, BotRepository, UserRepository};
use crate::utils::auth::api_keys::hash_api_key;
use uuid::Uuid;

use crate::AppState;

/// Capability dimensions a bot may be granted. Used by
/// [`AuthUser::require_bot_capability`] so every file operation can be gated
/// on both the underlying API-key scope and the bot's finer-grained flags.
#[derive(Debug, Clone, Copy)]
pub enum BotCapability {
    Upload,
    Download,
    Copy,
    Edit,
    Delete,
    List,
}

impl BotCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            BotCapability::Upload => "upload",
            BotCapability::Download => "download",
            BotCapability::Copy => "copy",
            BotCapability::Edit => "edit",
            BotCapability::Delete => "delete",
            BotCapability::List => "list",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub role: UserRole,
    pub username: String,
    pub email: String,
    /// Scopes granted to this request. `None` when authenticated with a JWT
    /// (normal UI user with unrestricted access); `Some(scopes)` when
    /// authenticated with an API key.
    pub scopes: Option<Vec<String>>,
    /// When the request was authenticated with a bot's API key, the bot's
    /// restriction record. `None` for normal JWT users and ordinary API keys.
    pub bot: Option<Bot>,
}

impl AuthUser {
    pub fn is_admin(&self) -> bool {
        self.role == UserRole::Admin
    }

    /// Returns `Ok(())` if the user has the required role, or
    /// `AppError::Forbidden` otherwise.
    pub fn require_role(&self, role: UserRole) -> AppResult<()> {
        if self.role != role {
            return Err(AppError::Forbidden(format!(
                "required role '{}', your role: '{}'",
                role, self.role
            )));
        }
        Ok(())
    }

    /// Shorthand for `require_role(UserRole::Admin)`.
    pub fn require_admin(&self) -> AppResult<()> {
        self.require_role(UserRole::Admin)
    }

    /// Enforce an API-key scope. JWT-authenticated (UI) users are unaffected
    /// and always pass; API-key-authenticated requests must hold the scope.
    pub fn require_scope(&self, scope: &str) -> AppResult<()> {
        if let Some(scopes) = &self.scopes {
            if !scopes.iter().any(|s| s == scope) {
                return Err(AppError::Forbidden(format!(
                    "missing required scope '{scope}'"
                )));
            }
        }
        Ok(())
    }

    /// Require a browser UI (JWT) session. API-key-authenticated requests are
    /// rejected: an automation key must never be able to change account
    /// credentials or mint/regenerate keys with broader scopes than it holds.
    /// Otherwise a leaked read-only key could escalate itself to full access.
    pub fn require_ui_session(&self) -> AppResult<()> {
        if self.scopes.is_some() {
            return Err(AppError::Forbidden(
                "this action requires a UI session and cannot be performed with an API key"
                    .into(),
            ));
        }
        Ok(())
    }

    /// `true` when this request was authenticated with a bot's API key.
    pub fn is_bot(&self) -> bool {
        self.bot.is_some()
    }

    /// Enforce a bot capability flag. JWT/ordinary-API-key requests pass; a
    /// bot request is denied unless its row grants the capability.
    pub fn require_bot_capability(&self, cap: BotCapability) -> AppResult<()> {
        if let Some(bot) = &self.bot {
            let allowed = match cap {
                BotCapability::Upload => bot.can_upload,
                BotCapability::Download => bot.can_download,
                BotCapability::Copy => bot.can_copy,
                BotCapability::Edit => bot.can_edit,
                BotCapability::Delete => bot.can_delete,
                BotCapability::List => bot.can_list,
            };
            if !allowed {
                return Err(AppError::Forbidden(format!(
                    "bot '{}' is not allowed to {}",
                    bot.name,
                    cap.as_str()
                )));
            }
        }
        Ok(())
    }

    /// Enforce the bot's bucket allow-list. A bot with an explicit allow-list
    /// may only touch buckets in it; an unrestricted bot passes.
    pub fn require_bot_bucket(&self, bucket: &str) -> AppResult<()> {
        if let Some(bot) = &self.bot {
            if !bot.bucket_allowed(bucket) {
                return Err(AppError::Forbidden(format!(
                    "bot '{}' cannot access bucket '{}'",
                    bot.name, bucket
                )));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let state = parts
            .extensions
            .get::<Arc<AppState>>()
            .ok_or_else(|| AppError::Internal("AppState not found in extensions".into()))?;

        let auth_header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Unauthorized("missing authorization header".into()))?;

        // RFC 7235: the auth scheme is case-insensitive ("Bearer", "bearer",
        // "BEARER"). The remainder (the credential) is used verbatim — leading
        // or trailing junk makes the lookup fail closed instead of being
        // silently trimmed.
        let (scheme, token) = auth_header.split_once(' ').ok_or_else(|| {
            AppError::Unauthorized(
                "invalid authorization format, expected 'Bearer <token>'".into(),
            )
        })?;

        if !scheme.eq_ignore_ascii_case("bearer") {
            return Err(AppError::Unauthorized(
                "invalid authorization format, expected 'Bearer <token>'".into(),
            ));
        }

        if token.starts_with(&state.config.auth.api_key_prefix) {
            return Self::from_api_key(&state, token).await;
        }

        let claims = state.jwt_service.validate_token(token)?;

        let user_id = Uuid::parse_str(&claims.sub)
            .map_err(|_| AppError::Unauthorized("invalid user id in token".into()))?;

        let user = UserRepository::find_by_id(state.db.pool(), user_id)
            .await?
            .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

        Ok(AuthUser {
            user_id: user.id,
            role: user.role,
            username: user.username,
            email: user.email,
            scopes: None,
            bot: None,
        })
    }
}

impl AuthUser {
    /// Authenticate with an API key presented as `Bearer <key>`. The key must
    /// be active, unexpired, and owned by a user; the request then acts as that
    /// user, with the key's scopes applied.
    async fn from_api_key(state: &AppState, token: &str) -> Result<Self, AppError> {
        let key_hash = hash_api_key(token);
        let api_key = ApiKeyRepository::find_by_key_hash(state.db.pool(), &key_hash)
            .await?
            .ok_or_else(|| AppError::Unauthorized("invalid API key".into()))?;

        if !api_key.is_valid() {
            return Err(AppError::Unauthorized("API key is inactive or expired".into()));
        }

        let owner_id = api_key
            .user_id
            .ok_or_else(|| AppError::Unauthorized("API key is not associated with a user".into()))?;

        let user = UserRepository::find_by_id(state.db.pool(), owner_id)
            .await?
            .ok_or_else(|| AppError::Unauthorized("API key owner not found".into()))?;

        // Best-effort usage tracking; failure must not break the request.
        let _ = ApiKeyRepository::update_last_used(state.db.pool(), api_key.id).await;

        // A bot's key resolves to its owner's identity, but the request is then
        // narrowed by the bot's restrictions (buckets, folders, files,
        // capabilities, upload cap).
        let bot = BotRepository::find_by_key_id(state.db.pool(), api_key.id).await?;

        Ok(AuthUser {
            user_id: user.id,
            role: user.role,
            username: user.username,
            email: user.email,
            scopes: Some(api_key.scopes),
            bot,
        })
    }
}
