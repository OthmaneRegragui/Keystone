use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::ApiKey;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::api_key_row::{CreateApiKeyData, ApiKeyRow};

pub struct ApiKeyRepository;

impl ApiKeyRepository {
    pub async fn create(pool: &PgPool, data: CreateApiKeyData) -> AppResult<ApiKey> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let scopes_json =
            serde_json::to_string(&data.scopes).unwrap_or_else(|_| "[]".to_string());
        let is_active = true;
        let expires_at = data.expires_at.map(|dt| dt.to_rfc3339());

        sqlx::query(
            r#"INSERT INTO api_keys (id, user_id, name, key_prefix, key_hash, scopes, expires_at, created_at, is_active)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(&id)
        .bind(data.user_id.map(|u| u.to_string()))
        .bind(&data.name)
        .bind(&data.key_prefix)
        .bind(&data.key_hash)
        .bind(&scopes_json)
        .bind(&expires_at)
        .bind(&now)
        .bind(is_active)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert api key: {e}")))?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("api key not found after insert".to_string()))
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<ApiKey>> {
        let row = sqlx::query_as::<_, ApiKeyRow>("SELECT * FROM api_keys WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query api key: {e}")))?;

        Ok(row.map(ApiKey::from))
    }

    pub async fn find_by_key_hash(pool: &PgPool, key_hash: &str) -> AppResult<Option<ApiKey>> {
        let row = sqlx::query_as::<_, ApiKeyRow>("SELECT * FROM api_keys WHERE key_hash = $1")
            .bind(key_hash)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query api key by hash: {e}")))?;

        Ok(row.map(ApiKey::from))
    }

    pub async fn list_by_user(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<ApiKey>> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT * FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list api keys: {e}")))?;

        Ok(rows.into_iter().map(ApiKey::from).collect())
    }

    pub async fn list_bot_keys(pool: &PgPool) -> AppResult<Vec<ApiKey>> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(
            "SELECT * FROM api_keys WHERE user_id IS NULL ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list bot api keys: {e}")))?;

        Ok(rows.into_iter().map(ApiKey::from).collect())
    }

    pub async fn update_last_used(pool: &PgPool, id: Uuid) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query("UPDATE api_keys SET last_used_at = $1 WHERE id = $2")
            .bind(&now)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update api key last used: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("api key {id} not found")));
        }
        Ok(())
    }

    /// Replace the key's scopes. Used when a bot's capabilities change so the
    /// underlying key stays in sync with the bot's granted flags.
    pub async fn update_scopes(pool: &PgPool, id: Uuid, scopes: &[String]) -> AppResult<()> {
        let scopes_json =
            serde_json::to_string(scopes).unwrap_or_else(|_| "[]".to_string());
        let affected = sqlx::query("UPDATE api_keys SET scopes = $1 WHERE id = $2")
            .bind(&scopes_json)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update api key scopes: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("api key {id} not found")));
        }
        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete api key: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    pub async fn deactivate(pool: &PgPool, id: Uuid) -> AppResult<()> {
        let affected = sqlx::query("UPDATE api_keys SET is_active = false WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to deactivate api key: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("api key {id} not found")));
        }
        Ok(())
    }
}
