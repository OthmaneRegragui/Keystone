use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: Uuid,
    pub user_id: Uuid,
    pub action: String,
    pub resource: String,
    pub resource_id: Option<String>,
    pub details: Option<String>,
    pub ip_address: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl AuditLog {
    pub fn new(
        user_id: Uuid,
        action: String,
        resource: String,
        resource_id: Option<String>,
        ip_address: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            action,
            resource,
            resource_id,
            details: None,
            ip_address,
            created_at: Utc::now(),
        }
    }

    pub fn with_details(mut self, details: String) -> Self {
        self.details = Some(details);
        self
    }

    pub fn action_matches(&self, expected: &str) -> bool {
        self.action == expected
    }

    pub fn resource_matches(&self, expected: &str) -> bool {
        self.resource == expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_audit_log() {
        let user_id = Uuid::new_v4();
        let log = AuditLog::new(
            user_id,
            "upload".to_string(),
            "file".to_string(),
            Some("file-uuid-123".to_string()),
            Some("127.0.0.1".to_string()),
        );

        assert!(!log.id.is_nil());
        assert_eq!(log.user_id, user_id);
        assert_eq!(log.action, "upload");
        assert_eq!(log.resource, "file");
        assert_eq!(log.resource_id.as_deref(), Some("file-uuid-123"));
        assert_eq!(log.ip_address.as_deref(), Some("127.0.0.1"));
        assert!(log.details.is_none());
    }

    #[test]
    fn test_with_details() {
        let log = AuditLog::new(
            Uuid::new_v4(),
            "delete".into(),
            "file".into(),
            None,
            None,
        )
        .with_details("deleted 3 files".to_string());

        assert_eq!(log.details.as_deref(), Some("deleted 3 files"));
    }

    #[test]
    fn test_action_and_resource_matches() {
        let log = AuditLog::new(
            Uuid::new_v4(),
            "create".into(),
            "user".into(),
            None,
            None,
        );

        assert!(log.action_matches("create"));
        assert!(!log.action_matches("delete"));
        assert!(log.resource_matches("user"));
        assert!(!log.resource_matches("file"));
    }
}
