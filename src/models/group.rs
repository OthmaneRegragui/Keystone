use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserGroup {
    pub id: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub allow_api_keys: bool,
    pub allow_password_change: bool,
    pub allow_bots: bool,
}
