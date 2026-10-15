pub mod backend;
pub mod ftp;
pub mod health;
pub mod local;

use std::collections::HashMap;
use std::sync::Arc;

use backend::StorageBackend;

use crate::error::{AppError, AppResult};
use crate::models::{Bucket, Connection, StoragePath};

/// Find the storage path that owns `bucket_path`: the longest configured path
/// that is an ancestor of (or equal to) the bucket's path. Returns `None` when
/// the bucket is under no configured path, in which case it stays on local disk.
///
/// Buckets only store their absolute path, not a foreign key, so ownership is
/// derived by prefix — the same convention `bucket_count_for_path` uses.
pub fn find_owning_path<'a>(paths: &'a [StoragePath], bucket_path: &str) -> Option<&'a StoragePath> {
    let bucket = std::path::Path::new(bucket_path);
    paths
        .iter()
        .filter(|p| bucket.starts_with(&p.path))
        .max_by_key(|p| p.path.len())
}

/// Build the storage backend for `bucket` from its owning storage path and that
/// path's assigned connection.
///
/// A path that assigns an FTP/FTPS connection yields a backend that stages
/// objects under the bucket's local directory and mirrors them to the remote
/// server, in a per-bucket remote directory so buckets under one path never
/// collide. Every other case (a plain local path, an unassigned path, or a
/// bucket with no owning path) is a plain local-disk backend. Construction
/// never touches the network: FTP connections are opened lazily per operation.
pub fn build_backend(
    bucket: &Bucket,
    owning_path: Option<&StoragePath>,
    connection: Option<&Connection>,
) -> AppResult<Arc<dyn StorageBackend>> {
    let local_root = std::path::PathBuf::from(&bucket.path);

    match (owning_path, connection) {
        (Some(sp), Some(conn)) if conn.is_ftp() => {
            let mut config: ftp::FtpConfig = serde_json::from_str(&conn.config).map_err(|e| {
                AppError::Validation(format!(
                    "connection '{}' has invalid FTP settings: {e}",
                    conn.name
                ))
            })?;
            if config.host.trim().is_empty() {
                return Err(AppError::Validation(format!(
                    "FTP connection '{}' has no host configured",
                    conn.name
                )));
            }
            config.base_path = join_remote(&config.base_path, &bucket.name);
            let backend = ftp::FtpBackend::new(local_root, config, sp.delete_local_after_upload)?;
            Ok(Arc::new(backend))
        }
        _ => Ok(Arc::new(local::LocalFsBackend::new(local_root)?)),
    }
}

/// Join a remote base path and a bucket name into a normalised remote root,
/// dropping empty segments so a blank base yields just the bucket name.
fn join_remote(base: &str, bucket: &str) -> String {
    let base = base.trim_matches('/');
    let bucket = bucket.trim_matches('/');
    match (base.is_empty(), bucket.is_empty()) {
        (true, _) => bucket.to_string(),
        (_, true) => base.to_string(),
        _ => format!("{base}/{bucket}"),
    }
}

pub struct StorageRegistry {
    backends: HashMap<String, Arc<dyn StorageBackend>>,
}

impl StorageRegistry {
    pub fn new() -> Self {
        Self {
            backends: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: impl Into<String>, backend: Arc<dyn StorageBackend>) {
        self.backends.insert(name.into(), backend);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn StorageBackend>> {
        self.backends.get(name).cloned()
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.backends.remove(name).is_some()
    }

    pub fn list_backends(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }
}
