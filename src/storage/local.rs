use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use crate::error::{AppError, AppResult};
use crate::utils::traits::StorageBackend;
use tokio::fs;

pub struct LocalFsBackend {
    root: PathBuf,
}

impl LocalFsBackend {
    pub fn new(root_path: impl Into<PathBuf>) -> AppResult<Self> {
        let root = root_path.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Resolve a storage key to a path that is guaranteed to live inside
    /// `self.root`.
    ///
    /// Security contract (defense-in-depth; keys are normally internally
    /// generated blake3 shard paths, but this must hold even for hostile keys):
    /// - Rejects empty keys and keys containing NUL bytes.
    /// - Rejects `..` components (ParentDir), absolute paths (RootDir) and
    ///   Windows drive/UNC prefixes (Prefix) — all path-traversal primitives.
    /// - Rebuilds the key from its normal components (dropping `.`/empty
    ///   segments) and joins it under the root, then verifies the joined path
    ///   still lexically starts with the root.
    ///
    /// Residual risk: the check is lexical only. A symlink placed *inside* the
    /// root by a local attacker with filesystem access could still redirect a
    /// resolved path outside the root (the OS follows the link). Keys are
    /// always server-generated content-hash shard paths, so no external input
    /// reaches this resolver; fully closing the symlink vector would require
    /// canonicalization, which breaks put/delete of not-yet-existing files and
    /// introduces its own TOCTOU window.
    fn resolve(&self, key: &str) -> AppResult<PathBuf> {
        if key.is_empty() {
            return Err(AppError::BadRequest(
                "empty storage key not allowed".to_string(),
            ));
        }
        if key.contains('\0') {
            return Err(AppError::BadRequest(
                "invalid storage key".to_string(),
            ));
        }

        let path = Path::new(key);
        let mut safe = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    return Err(AppError::BadRequest(
                        "path traversal not allowed".to_string(),
                    ));
                }
                Component::RootDir => {
                    return Err(AppError::BadRequest(
                        "absolute storage keys are not allowed".to_string(),
                    ));
                }
                Component::Prefix(_) => {
                    return Err(AppError::BadRequest(
                        "windows drive paths are not allowed".to_string(),
                    ));
                }
                // CurDir components are harmless; they are dropped here.
                Component::CurDir | Component::Normal(_) => {
                    if let Component::Normal(part) = component {
                        safe.push(part);
                    }
                }
            }
        }

        // A key that normalizes to nothing (e.g. "." or "./") would resolve to
        // the root itself, which no storage operation may target.
        if safe.as_os_str().is_empty() {
            return Err(AppError::BadRequest(
                "storage key resolves to the backend root".to_string(),
            ));
        }

        let joined = self.root.join(&safe);

        // Defense in depth: the joined path must stay lexically inside root.
        // (An absolute key would have been rejected above, and `..` is rejected,
        // so this can only trigger on exotic inputs — keep the belt.)
        if !joined.starts_with(&self.root) {
            return Err(AppError::BadRequest(
                "storage key escapes the backend root".to_string(),
            ));
        }
        Ok(joined)
    }
}

#[async_trait]
impl StorageBackend for LocalFsBackend {
    async fn put(&self, key: &str, body: Bytes) -> AppResult<()> {
        let path = self.resolve(key)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&path, &body).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> AppResult<Option<Bytes>> {
        let path = self.resolve(key)?;
        match fs::read(&path).await {
            Ok(data) => Ok(Some(Bytes::from(data))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }

    async fn delete(&self, key: &str) -> AppResult<bool> {
        let path = self.resolve(key)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }

    async fn exists(&self, key: &str) -> AppResult<bool> {
        let path = self.resolve(key)?;
        match fs::metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(AppError::Storage(e.to_string())),
        }
    }

    async fn list(&self, prefix: &str) -> AppResult<Vec<String>> {
        let prefix_path = self.resolve(prefix)?;
        let root = self.root.clone();
        let prefix_str = prefix.to_string();

        let mut keys = Vec::new();
        let mut walker = fs::read_dir(&prefix_path).await?;
        while let Some(entry) = walker.next_entry().await? {
            let full = entry.path();
            let relative = full
                .strip_prefix(&root)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if relative.starts_with(&prefix_str) {
                keys.push(relative);
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn scheme(&self) -> &str {
        "file"
    }
}
