use chrono::{DateTime, Utc};

#[derive(Debug, Clone)]
pub struct StoragePath {
    pub id: String,
    pub name: String,
    /// Local root directory. When the path assigns a remote connection this is
    /// the *staging* directory; otherwise it is the physical storage root.
    pub path: String,
    pub created_at: DateTime<Utc>,
    /// Saved connection this path mirrors to, if any. `None` = plain local disk.
    pub connection_id: Option<String>,
    /// Remove the staged local copy once a remote upload succeeds.
    pub delete_local_after_upload: bool,
}

impl StoragePath {
    /// Whether this path mirrors its buckets to a remote connection.
    pub fn has_connection(&self) -> bool {
        self.connection_id.is_some()
    }
}
