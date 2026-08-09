use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub id: Uuid,
    pub blake3_hash: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub size: i64,
    pub ref_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl File {
    pub fn new(blake3_hash: String, original_name: String, size: i64) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            blake3_hash,
            original_name,
            mime_type: None,
            size,
            ref_count: 1,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn is_deduplicated(&self) -> bool {
        self.ref_count > 1
    }

    pub fn display_size(&self) -> String {
        crate::utils::format_file_size(self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_file() {
        let file = File::new(
            "abc123".to_string(),
            "test.txt".to_string(),
            1024,
        );

        assert!(!file.id.is_nil());
        assert_eq!(file.blake3_hash, "abc123");
        assert_eq!(file.original_name, "test.txt");
        assert_eq!(file.size, 1024);
        assert_eq!(file.ref_count, 1);
        assert!(file.mime_type.is_none());
        assert_eq!(file.created_at, file.updated_at);
    }

    #[test]
    fn test_is_deduplicated() {
        let mut file = File::new("hash".into(), "name".into(), 100);
        assert!(!file.is_deduplicated());

        file.ref_count = 2;
        assert!(file.is_deduplicated());
    }

    #[test]
    fn test_display_size() {
        let file = File::new("h".into(), "n".into(), 1_500_000);
        assert_eq!(file.display_size(), "1.50 MB");
    }

    #[test]
    fn test_serialization_roundtrip() {
        let file = File::new(
            "hash123".to_string(),
            "document.pdf".to_string(),
            2048,
        );
        let json = serde_json::to_string(&file).unwrap();
        let deserialized: File = serde_json::from_str(&json).unwrap();
        assert_eq!(file.id, deserialized.id);
        assert_eq!(file.blake3_hash, deserialized.blake3_hash);
    }
}
