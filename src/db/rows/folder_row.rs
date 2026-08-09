use chrono::{DateTime, Utc};
use crate::models::UserFolder;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct FolderRow {
    pub id: String,
    pub user_id: String,
    pub bucket_name: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub created_at: String,
}

impl From<FolderRow> for UserFolder {
    fn from(row: FolderRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            user_id: Uuid::parse_str(&row.user_id).expect("invalid uuid in database"),
            bucket_name: row.bucket_name,
            parent_id: row.parent_id.map(|s| Uuid::parse_str(&s).expect("invalid uuid in database")),
            name: row.name,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FolderRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub bucket_name: String,
    pub parent_id: Option<Uuid>,
    pub name: String,
}

impl FolderRecord {
    pub fn new(user_id: Uuid, bucket_name: String, name: String, parent_id: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            bucket_name,
            parent_id,
            name,
        }
    }
}
