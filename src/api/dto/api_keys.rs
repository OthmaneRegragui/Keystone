use chrono::{DateTime, Utc};
use crate::models::ApiKey;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateApiKeyRequest {
    #[validate(length(min = 1, max = 100))]
    pub name: String,
    /// At most 10 scopes; membership itself is enforced by `validate_scopes`.
    #[validate(length(max = 10))]
    pub scopes: Vec<String>,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeOwnApiKeyRequest {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct ApiKeyDto {
    pub id: Uuid,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

impl From<ApiKey> for ApiKeyDto {
    fn from(key: ApiKey) -> Self {
        Self {
            id: key.id,
            name: key.name,
            prefix: key.key_prefix,
            scopes: key.scopes,
            last_used_at: key.last_used_at,
            expires_at: key.expires_at,
            created_at: key.created_at,
            is_active: key.is_active,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ApiKeyCreatedResponse {
    pub id: Uuid,
    pub name: String,
    pub full_key: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
