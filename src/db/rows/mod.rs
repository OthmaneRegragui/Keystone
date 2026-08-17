pub mod api_key_row;
pub mod bot_row;
pub mod file_row;
pub mod folder_row;
pub mod storage_object_row;
pub mod user_row;
pub mod user_file_row;

pub use api_key_row::{ApiKeyRow, CreateApiKeyData};
pub use bot_row::{bot_columns_serialized, BotRow, CreateBotData};
pub use file_row::{FileRecord, FileRow};
pub use folder_row::{FolderRecord, FolderRow};
pub use storage_object_row::{CreateStorageObjectData, StorageObjectRow};
pub use user_row::{CreateUserData, UserRow};
pub use user_file_row::{UserFileRecord, UserFileRow};

use chrono::{DateTime, Utc};
use crate::models::AuditLog;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct AuditLogRow {
    pub id: String,
    pub user_id: String,
    pub action: String,
    pub resource: String,
    pub resource_id: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: String,
}

impl From<AuditLogRow> for AuditLog {
    fn from(row: AuditLogRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            user_id: Uuid::parse_str(&row.user_id).expect("invalid uuid in database"),
            action: row.action,
            resource: row.resource,
            resource_id: row.resource_id,
            details: row.details,
            ip_address: row.ip_address,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateAuditLogData {
    pub user_id: Uuid,
    pub action: String,
    pub resource: String,
    pub resource_id: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
}
