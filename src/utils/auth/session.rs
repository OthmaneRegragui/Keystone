use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use crate::error::{AppError, AppResult};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
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

struct SessionStore {
    tokens: HashMap<String, RefreshToken>,
}

pub struct SessionService {
    store: Arc<RwLock<SessionStore>>,
    expiry_minutes: u64,
}

impl SessionService {
    pub fn new(expiry_minutes: u64) -> Self {
        Self {
            store: Arc::new(RwLock::new(SessionStore {
                tokens: HashMap::new(),
            })),
            expiry_minutes,
        }
    }

    /// Refresh-token lifetime in seconds (used for the cookie `Max-Age` so the
    /// browser and the store agree on when the session ends).
    pub fn expiry_seconds(&self) -> u64 {
        self.expiry_minutes * 60
    }

    /// Drop expired entries so the in-memory store cannot grow without bound
    /// (an attacker can otherwise mint one refresh token per login forever).
    /// Must be called while holding the write lock.
    fn prune_expired(store: &mut SessionStore) {
        let now = Utc::now();
        store.tokens.retain(|_, t| t.expires_at > now);
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

        let mut store = self.store.write().await;
        Self::prune_expired(&mut store);
        store.tokens.insert(token_hash, refresh_token.clone());

        Ok((raw_token, refresh_token))
    }

    pub async fn validate_refresh_token(&self, token: &str) -> AppResult<RefreshToken> {
        if token.len() > MAX_REFRESH_TOKEN_LEN {
            return Err(AppError::Unauthorized("invalid refresh token".into()));
        }
        let token_hash = Self::hash_token(token);
        let store = self.store.read().await;

        let refresh_token = store
            .tokens
            .get(&token_hash)
            .ok_or_else(|| AppError::Unauthorized("invalid refresh token".into()))?;

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

        Ok(refresh_token.clone())
    }

    pub async fn revoke_token(&self, token_id: Uuid) -> AppResult<()> {
        let mut store = self.store.write().await;

        let token = store.tokens.values_mut().find(|t| t.id == token_id);

        match token {
            Some(t) => {
                t.revoked = true;
                Ok(())
            }
            None => Err(AppError::NotFound("refresh token not found".into())),
        }
    }

    /// Revoke every live refresh token belonging to a user. Used after a
    /// password change so a stolen refresh token does not survive it.
    pub async fn revoke_all_for_user(&self, user_id: Uuid) {
        let mut store = self.store.write().await;
        for t in store.tokens.values_mut() {
            if t.user_id == user_id {
                t.revoked = true;
            }
        }
    }

    /// Atomically revoke the presented token and issue a replacement under a
    /// single write lock. A stolen token used concurrently by two clients can
    /// therefore only succeed once: the second rotation sees `revoked` and is
    /// rejected instead of silently minting another token.
    pub async fn rotate_token(
        &self,
        old_token: &str,
    ) -> AppResult<(String, RefreshToken)> {
        if old_token.len() > MAX_REFRESH_TOKEN_LEN {
            return Err(AppError::Unauthorized("invalid refresh token".into()));
        }

        let token_hash = Self::hash_token(old_token);
        let mut store = self.store.write().await;

        let old = store
            .tokens
            .get(&token_hash)
            .ok_or_else(|| AppError::Unauthorized("invalid refresh token".into()))?;

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

        let user_id = old.user_id;
        if let Some(t) = store.tokens.get_mut(&token_hash) {
            t.revoked = true;
        }

        let raw_token = Self::generate_raw_token();
        let new_hash = Self::hash_token(&raw_token);
        let now = Utc::now();

        let refresh_token = RefreshToken {
            id: Uuid::new_v4(),
            user_id,
            token_hash: new_hash.clone(),
            expires_at: now + chrono::Duration::minutes(self.expiry_minutes as i64),
            created_at: now,
            revoked: false,
        };

        Self::prune_expired(&mut store);
        store.tokens.insert(new_hash, refresh_token.clone());

        Ok((raw_token, refresh_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_validate_refresh_token() {
        let service = SessionService::new(60);
        let user_id = Uuid::new_v4();

        let (raw_token, stored) = service.create_refresh_token(user_id).await.unwrap();
        assert_eq!(stored.user_id, user_id);
        assert!(!stored.revoked);

        let validated = service.validate_refresh_token(&raw_token).await.unwrap();
        assert_eq!(validated.id, stored.id);
    }

    #[tokio::test]
    async fn test_validate_invalid_token() {
        let service = SessionService::new(60);
        assert!(service
            .validate_refresh_token("nonexistent_token")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn test_revoke_token() {
        let service = SessionService::new(60);
        let user_id = Uuid::new_v4();

        let (_, stored) = service.create_refresh_token(user_id).await.unwrap();
        service.revoke_token(stored.id).await.unwrap();

        let store = service.store.read().await;
        let token = store.tokens.get(&stored.token_hash).unwrap();
        assert!(token.revoked);
    }

    #[tokio::test]
    async fn test_rotate_token() {
        let service = SessionService::new(60);
        let user_id = Uuid::new_v4();

        let (old_raw, old_stored) = service.create_refresh_token(user_id).await.unwrap();
        let (new_raw, new_stored) = service.rotate_token(&old_raw).await.unwrap();

        assert_ne!(old_stored.id, new_stored.id);
        assert_eq!(new_stored.user_id, user_id);

        // Old token should be revoked
        let store = service.store.read().await;
        assert!(store.tokens[&old_stored.token_hash].revoked);

        // New token should be valid
        drop(store);
        assert!(service.validate_refresh_token(&new_raw).await.is_ok());

        // Old token should be invalid
        assert!(service.validate_refresh_token(&old_raw).await.is_err());
    }
}
