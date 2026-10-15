use chrono::{DateTime, Utc};

/// A saved, reusable remote-server configuration.
///
/// A connection is referenced by zero or more storage paths. The per-type
/// settings live in `config` as a JSON document interpreted according to
/// `connection_type` (`"ftp"` today), so supporting a new backend needs no
/// schema change — only a new `connection_type` and its parser.
#[derive(Debug, Clone)]
pub struct Connection {
    pub id: String,
    pub name: String,
    /// Backend kind, e.g. `"ftp"`.
    pub connection_type: String,
    /// Type-specific settings serialised as JSON (never returned verbatim).
    pub config: String,
    pub created_at: DateTime<Utc>,
}

impl Connection {
    /// Whether this connection points at an FTP/FTPS server.
    pub fn is_ftp(&self) -> bool {
        self.connection_type == "ftp"
    }
}
