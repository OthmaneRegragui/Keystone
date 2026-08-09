use chrono::{DateTime, Utc};
use crate::models::{User, UserRole};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct UserRow {
    pub id: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub storage_quota: i64,
    pub storage_used: i64,
    pub created_at: String,
    pub updated_at: String,
    pub last_login_at: Option<String>,
}

impl From<UserRow> for User {
    fn from(row: UserRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            username: row.username,
            email: row.email,
            password_hash: row.password_hash,
            role: parse_role(&row.role),
            storage_quota: row.storage_quota,
            storage_used: row.storage_used,
            created_at: DateTime::parse_from_rfc3339(&row.created_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.updated_at)
                .expect("invalid datetime in database")
                .with_timezone(&Utc),
            last_login_at: row.last_login_at.map(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .expect("invalid datetime in database")
                    .with_timezone(&Utc)
            }),
        }
    }
}

pub fn parse_role(role: &str) -> UserRole {
    match role {
        "admin" => UserRole::Admin,
        "service" => UserRole::Service,
        _ => UserRole::User,
    }
}

pub fn role_to_string(role: UserRole) -> String {
    match role {
        UserRole::Admin => "admin".to_string(),
        UserRole::User => "user".to_string(),
        UserRole::Service => "service".to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct CreateUserData {
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: UserRole,
    pub storage_quota: i64,
}
