use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::AdminSetting;
use sqlx::PgPool;

pub struct AdminSettingRepository;

impl AdminSettingRepository {
    pub async fn get(pool: &PgPool, key: &str) -> AppResult<Option<String>> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM admin_settings WHERE key = $1")
                .bind(key)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to get setting: {e}")))?;
        Ok(row.map(|r| r.0))
    }

    pub async fn set(pool: &PgPool, key: &str, value: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO admin_settings (key, value, updated_at) VALUES ($1, $2, $3) ON CONFLICT(key) DO UPDATE SET value = $2, updated_at = $3",
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to set setting: {e}")))?;
        Ok(())
    }

    pub async fn list(pool: &PgPool) -> AppResult<Vec<AdminSetting>> {
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT key, value, updated_at FROM admin_settings ORDER BY key")
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list settings: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(key, value, updated_at)| AdminSetting {
                key,
                value,
                updated_at: chrono::DateTime::parse_from_rfc3339(&updated_at)
                    .unwrap_or_default()
                    .with_timezone(&Utc),
            })
            .collect())
    }

    pub async fn get_bool(pool: &PgPool, key: &str) -> AppResult<bool> {
        let val = Self::get(pool, key).await?;
        Ok(val.map(|v| v == "true").unwrap_or(false))
    }

    pub async fn set_bool(pool: &PgPool, key: &str, val: bool) -> AppResult<()> {
        Self::set(pool, key, if val { "true" } else { "false" }).await
    }

    pub async fn get_platform_settings(pool: &PgPool) -> AppResult<crate::models::PlatformSettings> {
        let block_reg = Self::get_bool(pool, "block_registrations").await?;
        let allow_user_api_keys = Self::get_bool(pool, "allow_user_api_keys").await?;
        let allow_user_bots = Self::get_bool(pool, "allow_user_bots").await?;
        let allow_user_password_change = Self::get_bool(pool, "allow_user_password_change").await?;
        Ok(crate::models::PlatformSettings {
            block_registrations: block_reg,
            allow_user_api_keys,
            allow_user_bots,
            allow_user_password_change,
        })
    }
}
