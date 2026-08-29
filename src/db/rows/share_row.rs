use chrono::{DateTime, Utc};
use crate::models::share::{SharedItem, SharedItemType};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct SharedItemRow {
    pub id: String,
    pub shared_by_user_id: String,
    pub shared_with_user_id: String,
    pub item_type: String,
    pub item_id: String,
    pub created_at: String,
}

impl From<SharedItemRow> for SharedItem {
    fn from(row: SharedItemRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            shared_by_user_id: Uuid::parse_str(&row.shared_by_user_id).expect("invalid uuid in database"),
            shared_with_user_id: Uuid::parse_str(&row.shared_with_user_id).expect("invalid uuid in database"),
            item_type: SharedItemType::parse(&row.item_type).expect("invalid item_type in database"),
            item_id: Uuid::parse_str(&row.item_id).expect("invalid uuid in database"),
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateSharedItemData {
    pub id: Uuid,
    pub shared_by_user_id: Uuid,
    pub shared_with_user_id: Uuid,
    pub item_type: SharedItemType,
    pub item_id: Uuid,
}

impl CreateSharedItemData {
    pub fn new(
        shared_by_user_id: Uuid,
        shared_with_user_id: Uuid,
        item_type: SharedItemType,
        item_id: Uuid,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            shared_by_user_id,
            shared_with_user_id,
            item_type,
            item_id,
        }
    }
}
