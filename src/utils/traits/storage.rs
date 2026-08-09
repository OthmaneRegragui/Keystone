use async_trait::async_trait;
use axum::body::Bytes;

use crate::error::AppResult;

/// Backend-agnostic async storage trait.
///
/// Implementations provide concrete storage backends (e.g. local filesystem).
/// All keys are assumed to be unique string identifiers within a single backend namespace.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Store a blob under the given key.
    ///
    /// If the key already exists the implementation MUST overwrite the previous value.
    async fn put(&self, key: &str, body: Bytes) -> AppResult<()>;

    /// Retrieve a blob by key.
    ///
    /// Returns `Ok(None)` when the key does not exist.
    async fn get(&self, key: &str) -> AppResult<Option<Bytes>>;

    /// Delete a blob by key.
    ///
    /// Returns `Ok(true)` when the key existed and was removed, `Ok(false)` when the
    /// key was not found.
    async fn delete(&self, key: &str) -> AppResult<bool>;

    /// Check whether a blob with the given key exists.
    async fn exists(&self, key: &str) -> AppResult<bool>;

    /// List all keys that start with `prefix`.
    ///
    /// The returned keys MUST NOT include the prefix itself (i.e. they are relative).
    async fn list(&self, prefix: &str) -> AppResult<Vec<String>>;

    /// The scheme prefix used by this backend (e.g. `"local"`).
    fn scheme(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    struct InMemoryBackend {
        store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    impl InMemoryBackend {
        fn new() -> Self {
            Self {
                store: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl StorageBackend for InMemoryBackend {
        async fn put(&self, key: &str, body: Bytes) -> AppResult<()> {
            self.store
                .lock()
                .await
                .insert(key.to_string(), body.to_vec());
            Ok(())
        }

        async fn get(&self, key: &str) -> AppResult<Option<Bytes>> {
            Ok(self
                .store
                .lock()
                .await
                .get(key)
                .map(|v| Bytes::from(v.clone())))
        }

        async fn delete(&self, key: &str) -> AppResult<bool> {
            Ok(self.store.lock().await.remove(key).is_some())
        }

        async fn exists(&self, key: &str) -> AppResult<bool> {
            Ok(self.store.lock().await.contains_key(key))
        }

        async fn list(&self, prefix: &str) -> AppResult<Vec<String>> {
            Ok(self
                .store
                .lock()
                .await
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect())
        }

        fn scheme(&self) -> &str {
            "memory"
        }
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let backend = InMemoryBackend::new();
        let data = Bytes::from("hello world");

        backend.put("test/key", data.clone()).await.unwrap();
        let retrieved = backend.get("test/key").await.unwrap();
        assert_eq!(retrieved, Some(data));
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let backend = InMemoryBackend::new();
        assert!(backend.get("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_existing() {
        let backend = InMemoryBackend::new();
        backend.put("k", Bytes::from("v")).await.unwrap();

        let deleted = backend.delete("k").await.unwrap();
        assert!(deleted);
        assert!(backend.get("k").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let backend = InMemoryBackend::new();
        let deleted = backend.delete("missing").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_exists() {
        let backend = InMemoryBackend::new();
        assert!(!backend.exists("k").await.unwrap());

        backend.put("k", Bytes::from("v")).await.unwrap();
        assert!(backend.exists("k").await.unwrap());
    }

    #[tokio::test]
    async fn test_list_prefix() {
        let backend = InMemoryBackend::new();
        backend.put("a/1", Bytes::new()).await.unwrap();
        backend.put("a/2", Bytes::new()).await.unwrap();
        backend.put("b/1", Bytes::new()).await.unwrap();

        let mut keys = backend.list("a/").await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["a/1", "a/2"]);
    }

    #[tokio::test]
    async fn test_overwrite() {
        let backend = InMemoryBackend::new();
        backend.put("k", Bytes::from("v1")).await.unwrap();
        backend.put("k", Bytes::from("v2")).await.unwrap();

        let val = backend.get("k").await.unwrap().unwrap();
        assert_eq!(val, Bytes::from("v2"));
    }

    #[test]
    fn test_scheme() {
        let backend = InMemoryBackend::new();
        assert_eq!(backend.scheme(), "memory");
    }
}
