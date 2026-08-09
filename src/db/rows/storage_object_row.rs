use chrono::{DateTime, Utc};
use crate::models::StorageObject;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct StorageObjectRow {
    pub id: String,
    pub file_id: String,
    pub backend: String,
    pub storage_path: String,
    pub created_at: String,
}

impl From<StorageObjectRow> for StorageObject {
    fn from(row: StorageObjectRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            file_id: Uuid::parse_str(&row.file_id).expect("invalid uuid in database"),
            backend: row.backend,
            storage_path: row.storage_path,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateStorageObjectData {
    pub file_id: Uuid,
    pub backend: String,
    pub storage_path: String,
}
