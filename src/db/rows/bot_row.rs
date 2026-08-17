use chrono::{DateTime, Utc};
use crate::models::{Bot, BotPathRule};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct BotRow {
    pub id: String,
    pub user_id: String,
    pub key_id: String,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub can_copy: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_list: bool,
    pub path_rules: Option<String>,
    pub upload_limit_bytes: i64,
    pub uploaded_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<BotRow> for Bot {
    fn from(row: BotRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            user_id: Uuid::parse_str(&row.user_id).expect("invalid uuid in database"),
            key_id: Uuid::parse_str(&row.key_id).expect("invalid uuid in database"),
            name: row.name,
            can_upload: row.can_upload,
            can_download: row.can_download,
            can_copy: row.can_copy,
            can_edit: row.can_edit,
            can_delete: row.can_delete,
            can_list: row.can_list,
            path_rules: row.path_rules.map(|s| {
                serde_json::from_str::<Vec<BotPathRule>>(&s).unwrap_or_default()
            }),
            upload_limit_bytes: row.upload_limit_bytes,
            uploaded_bytes: row.uploaded_bytes,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.updated_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateBotData {
    pub user_id: Uuid,
    pub key_id: Uuid,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub can_copy: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_list: bool,
    pub path_rules: Option<Vec<BotPathRule>>,
    pub upload_limit_bytes: i64,
}

/// Serialize `path_rules` to its JSON column value. `None` stays `None`
/// (unrestricted); an empty list is stored as `[]`.
pub fn bot_columns_serialized(data: &CreateBotData) -> Option<String> {
    data.path_rules
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_else(|_| "[]".to_string()))
}
