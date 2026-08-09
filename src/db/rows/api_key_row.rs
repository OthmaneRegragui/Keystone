use chrono::{DateTime, Utc};
use crate::models::ApiKey;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct ApiKeyRow {
    pub id: String,
    pub user_id: Option<String>,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    pub scopes: String,
    pub last_used_at: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: String,
    pub is_active: bool,
}

impl From<ApiKeyRow> for ApiKey {
    fn from(row: ApiKeyRow) -> Self {
        let scopes: Vec<String> = serde_json::from_str(&row.scopes).unwrap_or_default();

        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            user_id: row.user_id.map(|s| Uuid::parse_str(&s).expect("invalid uuid in database")),
            name: row.name,
            key_prefix: row.key_prefix,
            key_hash: row.key_hash,
            scopes,
            last_used_at: row.last_used_at.map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .expect("invalid datetime in database")
                    .with_timezone(&Utc)
            }),
            expires_at: row.expires_at.map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .expect("invalid datetime in database")
                    .with_timezone(&Utc)
            }),
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
            is_active: row.is_active,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateApiKeyData {
    pub user_id: Option<Uuid>,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}
