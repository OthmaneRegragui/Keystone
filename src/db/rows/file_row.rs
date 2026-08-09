use chrono::{DateTime, Utc};
use crate::models::File;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct FileRow {
    pub id: String,
    pub blake3_hash: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub size: i64,
    pub ref_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

impl From<FileRow> for File {
    fn from(row: FileRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            blake3_hash: row.blake3_hash,
            original_name: row.original_name,
            mime_type: row.mime_type,
            size: row.size,
            ref_count: row.ref_count,
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
pub struct FileRecord {
    pub id: Uuid,
    pub blake3_hash: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub size: i64,
}

impl FileRecord {
    pub fn new(blake3_hash: String, original_name: String, mime_type: Option<String>, size: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            blake3_hash,
            original_name,
            mime_type,
            size,
        }
    }
}
