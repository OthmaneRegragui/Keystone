use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSetting {
    pub key: String,
    pub value: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bucket {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_active: bool,
    pub visible_to_users: bool,
    pub storage_limit: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformSettings {
    pub block_registrations: bool,
    pub allow_user_api_keys: bool,
    pub allow_user_bots: bool,
    pub allow_user_password_change: bool,
    pub allow_user_sharing: bool,
    /// Cloudflare Turnstile site key. Public: rendered into the login page.
    pub captcha_site_key: String,
    /// The admin's on/off toggle, as requested. This is deliberately reported
    /// separately from [`Self::captcha_configured`]: otherwise the admin screen
    /// could not tell "I turned it off" from "I turned it on but have not
    /// entered both keys yet", and the second case would silently look like the
    /// first.
    pub captcha_enabled: bool,
    /// Whether a Turnstile secret key is stored. The secret itself is never
    /// returned by any API.
    pub captcha_secret_configured: bool,
    /// Whether both keys are present, i.e. the captcha would actually be
    /// enforced. The login captcha is active only when this *and*
    /// `captcha_enabled` are both true.
    pub captcha_configured: bool,
}
