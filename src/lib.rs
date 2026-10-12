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

pub use api::dto;
pub use api::extractors::AuthUser;
pub use api::routes::api_routes;
