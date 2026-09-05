use chrono::{DateTime, Utc};
use crate::error::{AppError, AppResult};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Refresh tokens are 64 hex chars (32 random bytes). Anything far larger is
/// an abuse attempt; cap before hashing to bound CPU/memory on the hot path.
const MAX_REFRESH_TOKEN_LEN: usize = 1024;

/// Name of the httpOnly cookie that carries the refresh token for browser
/// sessions. Storing it here (instead of in JS-accessible localStorage) means
/// an XSS cannot exfiltrate the long-lived credential.
pub const REFRESH_COOKIE_NAME: &str = "keystone_refresh";

#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
}

/// Flat DB row for `refresh_tokens`. UUIDs are TEXT columns (matching the rest
/// of the schema); timestamps are `TIMESTAMPTZ` so expiry comparisons are exact.
#[derive(Debug, Clone, sqlx::FromRow)]
struct RefreshTokenRow {
    id: String,
    user_id: String,
    token_hash: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    revoked: bool,
}

/// Refresh-token store backed by Postgres.
///
/// Unlike the previous in-memory `HashMap`, tokens survive restarts and are
/// shared across instances, and rotation/revocation is atomic at the database
/// level. Only SHA-256 hashes of tokens are ever stored; the raw token is
/// returned to the client exactly once.
pub struct SessionService {
    pool: PgPool,
    expiry_minutes: u64,
}

impl SessionService {
    pub fn new(pool: PgPool, expiry_minutes: u64) -> Self {
        Self {
            pool,
            expiry_minutes,
        }
    }

    /// Refresh-token lifetime in seconds (used for the cookie `Max-Age` so the
    /// browser and the store agree on when the session ends).
    pub fn expiry_seconds(&self) -> u64 {
        self.expiry_minutes * 60
    }

    fn generate_raw_token() -> String {
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        hex::encode(bytes)
    }

    fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn row_to_token(row: RefreshTokenRow) -> AppResult<RefreshToken> {
        Ok(RefreshToken {
            id: Uuid::parse_str(&row.id).map_err(|_| {
                AppError::Internal("invalid refresh token id in database".into())
            })?,
            user_id: Uuid::parse_str(&row.user_id).map_err(|_| {
                AppError::Internal("invalid refresh token user id in database".into())
            })?,
            token_hash: row.token_hash,
            expires_at: row.expires_at,
            created_at: row.created_at,
            revoked: row.revoked,
        })
    }

    const COLUMNS: &'static str =
        "id, user_id, token_hash, expires_at, created_at, revoked";

    pub async fn create_refresh_token(
        &self,
        user_id: Uuid,
    ) -> AppResult<(String, RefreshToken)> {
        let raw_token = Self::generate_raw_token();
        let token_hash = Self::hash_token(&raw_token);
        let now = Utc::now();

        let refresh_token = RefreshToken {
            id: Uuid::new_v4(),
            user_id,
            token_hash: token_hash.clone(),
            expires_at: now + chrono::Duration::minutes(self.expiry_minutes as i64),
            created_at: now,
            revoked: false,
        };

        // Drop the user's expired rows so the table cannot grow without bound
        // (the previous in-memory store pruned on every insert too). Best-effort:
        // a failing prune must not break login.
        let _ = sqlx::query(
            "DELETE FROM refresh_tokens WHERE user_id = $1 AND expires_at <= NOW() AND revoked = false",
        )
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await;

        let sql = format!(
            "INSERT INTO refresh_tokens ({}) VALUES ($1, $2, $3, $4, $5, $6)",
            Self::COLUMNS
        );
        sqlx::query(&sql)
            .bind(refresh_token.id.to_string())
            .bind(user_id.to_string())
            .bind(&token_hash)
            .bind(refresh_token.expires_at)
            .bind(refresh_token.created_at)
            .bind(false)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to store refresh token: {e}")))?;

        Ok((raw_token, refresh_token))
    }

    pub async fn validate_refresh_token(&self, token: &str) -> AppResult<RefreshToken> {
        if token.len() > MAX_REFRESH_TOKEN_LEN {
            return Err(AppError::Unauthorized("invalid refresh token".into()));
        }
        let token_hash = Self::hash_token(token);

        let sql = format!(
            "SELECT {} FROM refresh_tokens WHERE token_hash = $1",
            Self::COLUMNS
        );
        let row = sqlx::query_as::<_, RefreshTokenRow>(&sql)
            .bind(&token_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query refresh token: {e}")))?;

        let refresh_token = match row {
            Some(r) => Self::row_to_token(r)?,
            None => return Err(AppError::Unauthorized("invalid refresh token".into())),
        };

        if refresh_token.revoked {
            return Err(AppError::Unauthorized(
                "refresh token has been revoked".into(),
            ));
        }

        if Utc::now() >= refresh_token.expires_at {
            return Err(AppError::Unauthorized(
                "refresh token has expired".into(),
            ));
        }

        Ok(refresh_token)
    }

    pub async fn revoke_token(&self, token_id: Uuid) -> AppResult<()> {
        let affected = sqlx::query("UPDATE refresh_tokens SET revoked = true WHERE id = $1")
            .bind(token_id.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to revoke refresh token: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound("refresh token not found".into()));
        }
        Ok(())
    }

    /// Revoke every live refresh token belonging to a user. Used after a
    /// password change so a stolen refresh token does not survive it.
    pub async fn revoke_all_for_user(&self, user_id: Uuid) {
        let _ = sqlx::query(
            "UPDATE refresh_tokens SET revoked = true WHERE user_id = $1 AND revoked = false",
        )
        .bind(user_id.to_string())
        .execute(&self.pool)
        .await;
    }

    /// Atomically revoke the presented token and issue a replacement inside a
    /// single transaction. The old row is locked with `FOR UPDATE`, so a stolen
    /// token used concurrently by two clients can only succeed once: the second
    /// rotation blocks until the first commits, then sees `revoked = true` and
    /// is rejected instead of silently minting another token.
    pub async fn rotate_token(
        &self,
        old_token: &str,
    ) -> AppResult<(String, RefreshToken)> {
        if old_token.len() > MAX_REFRESH_TOKEN_LEN {
            return Err(AppError::Unauthorized("invalid refresh token".into()));
        }

        let token_hash = Self::hash_token(old_token);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin transaction: {e}")))?;

        let sql = format!(
            "SELECT {} FROM refresh_tokens WHERE token_hash = $1 FOR UPDATE",
            Self::COLUMNS
        );
        let row = sqlx::query_as::<_, RefreshTokenRow>(&sql)
            .bind(&token_hash)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query refresh token: {e}")))?;

        let old = match row {
            Some(r) => Self::row_to_token(r)?,
            None => {
                return Err(AppError::Unauthorized("invalid refresh token".into()));
            }
        };

        if old.revoked {
            return Err(AppError::Unauthorized(
                "refresh token has been revoked".into(),
            ));
        }

        if Utc::now() >= old.expires_at {
            return Err(AppError::Unauthorized(
                "refresh token has expired".into(),
            ));
        }

        sqlx::query("UPDATE refresh_tokens SET revoked = true WHERE id = $1")
            .bind(old.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to revoke refresh token: {e}")))?;

        let raw_token = Self::generate_raw_token();
        let new_hash = Self::hash_token(&raw_token);
        let now = Utc::now();

        let refresh_token = RefreshToken {
            id: Uuid::new_v4(),
            user_id: old.user_id,
            token_hash: new_hash.clone(),
            expires_at: now + chrono::Duration::minutes(self.expiry_minutes as i64),
            created_at: now,
            revoked: false,
        };

        let sql = format!(
            "INSERT INTO refresh_tokens ({}) VALUES ($1, $2, $3, $4, $5, $6)",
            Self::COLUMNS
        );
        sqlx::query(&sql)
            .bind(refresh_token.id.to_string())
            .bind(refresh_token.user_id.to_string())
            .bind(&new_hash)
            .bind(refresh_token.expires_at)
            .bind(refresh_token.created_at)
            .bind(false)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to store rotated refresh token: {e}")))?;

        tx.commit()
            .await
            .map_err(|e| AppError::Internal(format!("failed to commit token rotation: {e}")))?;

        Ok((raw_token, refresh_token))
    }
}