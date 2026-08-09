use chrono::{DateTime, Utc};
use crate::models::UserFile;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct UserFileRow {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub created_at: String,
    pub bucket_name: Option<String>,
    pub folder_id: Option<String>,
    pub deleted_at: Option<String>,
}

impl From<UserFileRow> for UserFile {
    fn from(row: UserFileRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            user_id: Uuid::parse_str(&row.user_id).expect("invalid uuid in database"),
            file_id: Uuid::parse_str(&row.file_id).expect("invalid uuid in database"),
            original_name: row.original_name,
            mime_type: row.mime_type,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
            bucket_name: row.bucket_name,
            folder_id: row.folder_id.and_then(|s| Uuid::parse_str(&s).ok()),
            deleted_at: row.deleted_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserFileRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub file_id: Uuid,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub bucket_name: Option<String>,
    pub folder_id: Option<Uuid>,
}

impl UserFileRecord {
    pub fn new(user_id: Uuid, file_id: Uuid, original_name: String, mime_type: Option<String>, bucket_name: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            file_id,
            original_name,
            mime_type,
            bucket_name,
            folder_id: None,
        }
    }
}
