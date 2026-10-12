use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::AdminSetting;
use sqlx::PgPool;

/// `admin_settings` keys backing the login CAPTCHA.
pub const CAPTCHA_SITE_KEY: &str = "turnstile_site_key";
pub const CAPTCHA_SECRET_KEY: &str = "turnstile_secret_key";

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
        let allow_user_sharing = Self::get_bool(pool, "allow_user_sharing").await?;
        let keys = Self::get_captcha_keys(pool).await?;
        let captcha_enabled = Self::get_bool(pool, "captcha_enabled").await?;
        let site_key = keys.site_key.trim().to_string();
        let secret_configured = !keys.secret_key.trim().is_empty();
        Ok(crate::models::PlatformSettings {
            block_registrations: block_reg,
            allow_user_api_keys,
            allow_user_bots,
            allow_user_password_change,
            allow_user_sharing,
            captcha_site_key: site_key.clone(),
            captcha_enabled,
            captcha_secret_configured: secret_configured,
            captcha_configured: !site_key.is_empty() && secret_configured,
        })
    }

    /// Login CAPTCHA keys, as configured from the admin settings screen.
    pub async fn get_captcha_keys(pool: &PgPool) -> AppResult<CaptchaKeys> {
        let site_key = Self::get(pool, CAPTCHA_SITE_KEY).await?.unwrap_or_default();
        let secret_key = Self::get(pool, CAPTCHA_SECRET_KEY).await?.unwrap_or_default();
        Ok(CaptchaKeys { site_key, secret_key })
    }

    /// Persist the login CAPTCHA configuration.
    ///
    /// The secret key is write-only: `None` leaves the stored value
    /// untouched (so the admin UI can round-trip without ever receiving
    /// it), while `Some("")` clears it.
    pub async fn set_captcha(
        pool: &PgPool,
        enabled: bool,
        site_key: &str,
        secret_key: Option<&str>,
    ) -> AppResult<()> {
        Self::set(pool, CAPTCHA_SITE_KEY, site_key.trim()).await?;
        if let Some(secret) = secret_key {
            Self::set(pool, CAPTCHA_SECRET_KEY, secret.trim()).await?;
        }
        Self::set_bool(pool, "captcha_enabled", enabled).await?;
        Ok(())
    }
}

/// Admin-managed Cloudflare Turnstile keys.
#[derive(Debug, Clone, Default)]
pub struct CaptchaKeys {
    pub site_key: String,
    pub secret_key: String,
}
