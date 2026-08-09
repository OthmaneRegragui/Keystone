use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Represents a user's logical ownership of a file.
/// The actual file content is stored via the `File` model (content-addressed),
/// but each user has their own `UserFile` entry with their own name/mime_type.
/// Two users uploading the same content share one physical blob but have separate UserFile entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFile {
    pub id: Uuid,
    pub user_id: Uuid,
    pub file_id: Uuid,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub created_at: DateTime<Utc>,
    pub bucket_name: Option<String>,
    pub folder_id: Option<Uuid>,
    /// When set, the file is soft-deleted (hidden from user but not physically removed).
    pub deleted_at: Option<DateTime<Utc>>,
}

impl UserFile {
    pub fn new(user_id: Uuid, file_id: Uuid, original_name: String, mime_type: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            file_id,
            original_name,
            mime_type,
            created_at: Utc::now(),
            bucket_name: None,
            folder_id: None,
            deleted_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_user_file() {
        let user_id = Uuid::new_v4();
        let file_id = Uuid::new_v4();
        let uf = UserFile::new(user_id, file_id, "doc.pdf".into(), Some("application/pdf".into()));

        assert!(!uf.id.is_nil());
        assert_eq!(uf.user_id, user_id);
        assert_eq!(uf.file_id, file_id);
        assert_eq!(uf.original_name, "doc.pdf");
        assert_eq!(uf.mime_type.as_deref(), Some("application/pdf"));
    }
}
