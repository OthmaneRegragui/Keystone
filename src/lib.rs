pub mod config;
pub mod error;
pub mod models;
pub mod utils;
pub mod db;
pub mod storage;
pub mod api;

use tokio::sync::RwLock;

pub struct AppState {
    pub db: crate::db::Database,
    pub jwt_service: crate::utils::auth::jwt::JwtService,
    pub storage: RwLock<crate::storage::StorageRegistry>,
    pub session_service: crate::utils::auth::session::SessionService,
    /// Cloudflare Turnstile verification for unauthenticated endpoints. A
    /// no-op when no captcha keys are configured.
    pub captcha_service: crate::utils::auth::captcha::CaptchaService,
    pub config: crate::config::Settings,
}

impl AppState {
    /// Rebuild and re-register every bucket's storage backend from the current
    /// storage-path configuration.
    ///
    /// Called when a storage path is created or deleted so buckets pick up a
    /// new FTP/local backend without a restart. FTP backends connect lazily, so
    /// this performs no network I/O.
    pub async fn reload_storage_backends(&self) -> crate::error::AppResult<()> {
        let buckets = crate::db::repos::BucketRepository::list(self.db.pool()).await?;
        let paths = crate::db::repos::StoragePathRepository::list(self.db.pool()).await?;
        let connections: std::collections::HashMap<String, crate::models::Connection> =
            crate::db::repos::ConnectionRepository::list(self.db.pool())
                .await?
                .into_iter()
                .map(|c| (c.id.clone(), c))
                .collect();
        let mut registry = self.storage.write().await;
        for bucket in &buckets {
            let owning = crate::storage::find_owning_path(&paths, &bucket.path);
            let connection = owning
                .and_then(|p| p.connection_id.as_ref())
                .and_then(|id| connections.get(id));
            let backend = crate::storage::build_backend(bucket, owning, connection).map_err(|e| {
                crate::error::AppError::Internal(format!(
                    "failed to build storage for bucket '{}': {e}",
                    bucket.name
                ))
            })?;
            registry.register(bucket.name.clone(), backend);
        }
        Ok(())
    }
}

pub use api::dto;
pub use api::extractors::AuthUser;
pub use api::routes::api_routes;
