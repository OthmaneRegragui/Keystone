use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub name: String,
    pub key_prefix: String,
    pub key_hash: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

impl ApiKey {
    pub fn new(
        user_id: Option<Uuid>,
        name: String,
        key_prefix: String,
        key_hash: String,
        scopes: Vec<String>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            user_id,
            name,
            key_prefix,
            key_hash,
            scopes,
            last_used_at: None,
            expires_at,
            created_at: Utc::now(),
            is_active: true,
        }
    }

    pub fn is_bot(&self) -> bool {
        self.user_id.is_none()
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|exp| Utc::now() >= exp)
            .unwrap_or(false)
    }

    pub fn is_valid(&self) -> bool {
        self.is_active && !self.is_expired()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    pub fn has_any_scope(&self, scopes: &[&str]) -> bool {
        scopes.iter().any(|s| self.has_scope(s))
    }

    pub fn record_usage(&mut self) {
        self.last_used_at = Some(Utc::now());
    }

    pub fn deactivate(&mut self) {
        self.is_active = false;
    }

    pub fn matches_prefix(&self, key_start: &str) -> bool {
        self.key_prefix == key_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_key() -> ApiKey {
        ApiKey::new(
            Some(Uuid::new_v4()),
            "test-key".to_string(),
            "ks_abc123".to_string(),
            "hashed_key_value".to_string(),
            vec!["files:read".to_string(), "files:write".to_string()],
            None,
        )
    }

    #[test]
    fn test_new_api_key() {
        let key = sample_key();
        assert!(!key.id.is_nil());
        assert_eq!(key.name, "test-key");
        assert_eq!(key.key_prefix, "ks_abc123");
        assert!(key.is_active);
        assert!(key.last_used_at.is_none());
    }

    #[test]
    fn test_not_expired_without_expiry() {
        let key = sample_key();
        assert!(!key.is_expired());
    }

    #[test]
    fn test_is_expired_with_past_date() {
        let mut key = sample_key();
        key.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(key.is_expired());
        assert!(!key.is_valid());
    }

    #[test]
    fn test_is_valid_active_and_not_expired() {
        let mut key = sample_key();
        key.expires_at = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(key.is_valid());

        key.deactivate();
        assert!(!key.is_valid());
    }

    #[test]
    fn test_scopes() {
        let key = sample_key();
        assert!(key.has_scope("files:read"));
        assert!(key.has_scope("files:write"));
        assert!(!key.has_scope("admin"));

        assert!(key.has_any_scope(&["admin", "files:read"]));
        assert!(!key.has_any_scope(&["admin", "users:write"]));
    }

    #[test]
    fn test_record_usage() {
        let mut key = sample_key();
        assert!(key.last_used_at.is_none());

        key.record_usage();
        assert!(key.last_used_at.is_some());
    }

    #[test]
    fn test_matches_prefix() {
        let key = sample_key();
        assert!(key.matches_prefix("ks_abc123"));
        assert!(!key.matches_prefix("ks_xyz"));
    }

    #[test]
    fn test_deactivate() {
        let mut key = sample_key();
        assert!(key.is_active);

        key.deactivate();
        assert!(!key.is_active);
        assert!(!key.is_valid());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let key = sample_key();
        let json = serde_json::to_string(&key).unwrap();
        let deserialized: ApiKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key.id, deserialized.id);
        assert_eq!(key.scopes, deserialized.scopes);
    }
}
