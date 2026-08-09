pub mod pool;
pub mod rows;
pub mod repos;

pub use pool::{run_migrations, Database};
pub use repos::{
    AdminSettingRepository, ApiKeyRepository, AuditLogRepository, BucketRepository,
    FileRepository, GroupRepository, StorageObjectRepository, StoragePathRepository,
    UserRepository, UserFileRepository,
};

/// Returns true if the sqlx error is a unique-constraint violation (PostgreSQL SQLSTATE 23505).
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .map(|d| d.code().as_deref() == Some("23505"))
        .unwrap_or(false)
}
