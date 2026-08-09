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
    pub config: crate::config::Settings,
}

pub use api::dto;
pub use api::extractors::AuthUser;
pub use api::routes::api_routes;
