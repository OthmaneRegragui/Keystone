use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct StoragePath {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: DateTime<Utc>,
}
