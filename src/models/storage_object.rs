use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageObject {
    pub id: Uuid,
    pub file_id: Uuid,
    pub backend: String,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}

impl StorageObject {
    pub fn new(file_id: Uuid, backend: String, storage_path: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            file_id,
            backend,
            storage_path,
            created_at: Utc::now(),
        }
    }

    pub fn extension(&self) -> Option<&str> {
        let filename = self.filename()?;
        filename.rsplit('.').next().filter(|s| *s != filename && !s.is_empty())
    }

    pub fn filename(&self) -> Option<&str> {
        self.storage_path.rsplit('/').next().filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_storage_object() {
        let file_id = Uuid::new_v4();
        let obj = StorageObject::new(
            file_id,
            "local".to_string(),
            "ab/cd/ef123456.jpg".to_string(),
        );

        assert!(!obj.id.is_nil());
        assert_eq!(obj.file_id, file_id);
        assert_eq!(obj.backend, "local");
        assert_eq!(obj.storage_path, "ab/cd/ef123456.jpg");
    }

    #[test]
    fn test_extension() {
        let obj = StorageObject::new(
            Uuid::new_v4(),
            "local".into(),
            "abc/file.tar.gz".into(),
        );
        assert_eq!(obj.extension(), Some("gz"));

        let no_ext = StorageObject::new(
            Uuid::new_v4(),
            "local".into(),
            "abc/Makefile".into(),
        );
        assert_eq!(no_ext.extension(), None);
    }

    #[test]
    fn test_filename() {
        let obj = StorageObject::new(
            Uuid::new_v4(),
            "s3".into(),
            "uploads/2024/01/document.pdf".into(),
        );
        assert_eq!(obj.filename(), Some("document.pdf"));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let obj = StorageObject::new(
            Uuid::new_v4(),
            "local".into(),
            "path/to/file.bin".into(),
        );
        let json = serde_json::to_string(&obj).unwrap();
        let deserialized: StorageObject = serde_json::from_str(&json).unwrap();
        assert_eq!(obj.id, deserialized.id);
        assert_eq!(obj.storage_path, deserialized.storage_path);
    }
}
