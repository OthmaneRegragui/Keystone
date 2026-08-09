use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A virtual folder for organizing files within a bucket.
/// Folders are purely database-level — the actual file storage remains content-addressed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFolder {
    pub id: Uuid,
    pub user_id: Uuid,
    pub bucket_name: String,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

impl UserFolder {
    pub fn new(user_id: Uuid, bucket_name: String, name: String, parent_id: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            bucket_name,
            parent_id,
            name,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_folder() {
        let user_id = Uuid::new_v4();
        let f = UserFolder::new(user_id, "my-bucket".into(), "Documents".into(), None);
        assert!(!f.id.is_nil());
        assert_eq!(f.user_id, user_id);
        assert_eq!(f.bucket_name, "my-bucket");
        assert_eq!(f.name, "Documents");
        assert!(f.parent_id.is_none());
    }

    #[test]
    fn test_new_nested_folder() {
        let user_id = Uuid::new_v4();
        let parent = Uuid::new_v4();
        let f = UserFolder::new(user_id, "my-bucket".into(), "Work".into(), Some(parent));
        assert_eq!(f.parent_id, Some(parent));
    }
}
