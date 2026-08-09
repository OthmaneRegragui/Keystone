use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
    Service,
}

impl Default for UserRole {
    fn default() -> Self {
        Self::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: UserRole,
    pub storage_quota: i64,
    pub storage_used: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn new(
        username: String,
        email: String,
        password_hash: String,
        storage_quota: i64,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            username,
            email,
            password_hash,
            role: UserRole::default(),
            storage_quota,
            storage_used: 0,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        }
    }

    pub fn is_admin(&self) -> bool {
        self.role == UserRole::Admin
    }

    pub fn is_service(&self) -> bool {
        self.role == UserRole::Service
    }

    pub fn storage_remaining(&self) -> i64 {
        (self.storage_quota - self.storage_used).max(0)
    }

    pub fn storage_usage_percent(&self) -> f64 {
        if self.storage_quota == 0 {
            return 0.0;
        }
        (self.storage_used as f64 / self.storage_quota as f64) * 100.0
    }

    /// Whether the user has room for `bytes` of new storage. A quota of
    /// `0` means unlimited (matches the admin UI's "0 = unlimited").
    pub fn has_storage_available(&self, bytes: i64) -> bool {
        if self.storage_quota <= 0 {
            return true;
        }
        self.storage_remaining() >= bytes
    }

    pub fn display_storage_quota(&self) -> String {
        format!(
            "{} / {}",
            crate::utils::format_file_size(self.storage_used),
            crate::utils::format_file_size(self.storage_quota)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_role_display() {
        assert_eq!(UserRole::Admin.to_string(), "admin");
        assert_eq!(UserRole::User.to_string(), "user");
        assert_eq!(UserRole::Service.to_string(), "service");
    }

    #[test]
    fn test_user_role_parse() {
        assert_eq!("admin".parse::<UserRole>().unwrap(), UserRole::Admin);
        assert_eq!("user".parse::<UserRole>().unwrap(), UserRole::User);
        assert_eq!("service".parse::<UserRole>().unwrap(), UserRole::Service);
        assert!("superadmin".parse::<UserRole>().is_err());
    }

    #[test]
    fn test_new_user() {
        let user = User::new(
            "alice".to_string(),
            "alice@example.com".to_string(),
            "hashed_password".to_string(),
            1_073_741_824, // 1 GB
        );

        assert!(!user.id.is_nil());
        assert_eq!(user.username, "alice");
        assert_eq!(user.email, "alice@example.com");
        assert_eq!(user.role, UserRole::User);
        assert_eq!(user.storage_used, 0);
        assert!(user.last_login_at.is_none());
    }

    #[test]
    fn test_storage_calculations() {
        let mut user = User::new(
            "bob".into(),
            "bob@example.com".into(),
            "hash".into(),
            1000,
        );

        assert_eq!(user.storage_remaining(), 1000);
        assert!(!user.has_storage_available(1001));
        assert!(user.has_storage_available(500));

        user.storage_used = 750;
        assert_eq!(user.storage_remaining(), 250);
        assert!((user.storage_usage_percent() - 75.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_storage_quota_zero_means_unlimited() {
        let mut user = User::new("carol".into(), "carol@example.com".into(), "hash".into(), 0);
        assert!(user.has_storage_available(1));
        assert!(user.has_storage_available(1 << 30));
        user.storage_used = 1 << 30;
        assert!(user.has_storage_available(1 << 30));
        assert_eq!(user.storage_usage_percent(), 0.0);
    }

    #[test]
    fn test_is_admin() {
        let mut user = User::new("a".into(), "a@b.com".into(), "h".into(), 100);
        assert!(!user.is_admin());

        user.role = UserRole::Admin;
        assert!(user.is_admin());
    }

    #[test]
    fn test_display_storage_quota() {
        let mut user = User::new("a".into(), "a@b.com".into(), "h".into(), 1_073_741_824);
        user.storage_used = 500_000_000;
        assert_eq!(user.display_storage_quota(), "500 MB / 1.07 GB");
    }

    #[test]
    fn test_storage_remaining_underflow() {
        let mut user = User::new("a".into(), "a@b.com".into(), "h".into(), 100);
        user.storage_used = 200;
        assert_eq!(user.storage_remaining(), 0);
    }
}
