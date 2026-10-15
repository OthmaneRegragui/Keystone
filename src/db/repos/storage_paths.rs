use chrono::Utc;
use crate::db::is_unique_violation;
use crate::error::{AppError, AppResult};
use crate::models::StoragePath;
use sqlx::PgPool;

/// Backend configuration stored alongside a storage path.
///
/// Kept separate from the full [`StoragePath`] model so create/update call
/// sites pass only what selects the backend; identity and the local root are
/// handled separately. `local()` is the conservative default, so a path that
/// never assigns a connection behaves exactly as before.
#[derive(Debug, Clone)]
pub struct StorageConfig {
    /// Saved connection this path mirrors to, if any. `None` = plain local disk.
    pub connection_id: Option<String>,
    /// Remove the staged local copy once a remote upload succeeds.
    pub delete_local_after_upload: bool,
}

impl StorageConfig {
    pub fn local() -> Self {
        Self {
            connection_id: None,
            delete_local_after_upload: false,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self::local()
    }
}

/// Columns selected for every `StoragePath` read, in `StoragePathRow` order.
const PATH_COLUMNS: &str = "id, name, path, created_at, connection_id, delete_local_after_upload";

/// Raw `storage_paths` row. `created_at` is stored as an RFC 3339 string.
#[derive(sqlx::FromRow)]
struct StoragePathRow {
    id: String,
    name: String,
    path: String,
    created_at: String,
    connection_id: Option<String>,
    delete_local_after_upload: bool,
}

impl StoragePathRow {
    fn into_path(self) -> StoragePath {
        StoragePath {
            id: self.id,
            name: self.name,
            path: self.path,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
            connection_id: self.connection_id,
            delete_local_after_upload: self.delete_local_after_upload,
        }
    }
}

pub struct StoragePathRepository;

impl StoragePathRepository {
    pub async fn list(pool: &PgPool) -> AppResult<Vec<StoragePath>> {
        let rows: Vec<StoragePathRow> =
            sqlx::query_as(&format!("SELECT {PATH_COLUMNS} FROM storage_paths ORDER BY name ASC"))
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list storage paths: {e}")))?;

        Ok(rows.into_iter().map(StoragePathRow::into_path).collect())
    }

    pub async fn find_by_id(pool: &PgPool, id: &str) -> AppResult<Option<StoragePath>> {
        let row: Option<StoragePathRow> =
            sqlx::query_as(&format!("SELECT {PATH_COLUMNS} FROM storage_paths WHERE id = $1"))
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to find storage path: {e}")))?;

        Ok(row.map(StoragePathRow::into_path))
    }

    pub async fn create(pool: &PgPool, name: &str, path: &str) -> AppResult<StoragePath> {
        Self::create_with(pool, name, path, StorageConfig::local()).await
    }

    pub async fn create_with(
        pool: &PgPool,
        name: &str,
        path: &str,
        config: StorageConfig,
    ) -> AppResult<StoragePath> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO storage_paths \
             (id, name, path, created_at, connection_id, delete_local_after_upload) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&id)
        .bind(name)
        .bind(path)
        .bind(&now)
        .bind(&config.connection_id)
        .bind(config.delete_local_after_upload)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("storage path '{name}' already exists"))
            } else {
                AppError::Internal(format!("failed to create storage path: {e}"))
            }
        })?;

        Ok(StoragePath {
            id,
            name: name.to_string(),
            path: path.to_string(),
            created_at: Utc::now(),
            connection_id: config.connection_id,
            delete_local_after_upload: config.delete_local_after_upload,
        })
    }

    pub async fn update(
        pool: &PgPool,
        id: &str,
        name: &str,
        path: &str,
        config: &StorageConfig,
    ) -> AppResult<()> {
        let affected = sqlx::query(
            "UPDATE storage_paths SET name = $2, path = $3, connection_id = $4, \
             delete_local_after_upload = $5 WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(path)
        .bind(&config.connection_id)
        .bind(config.delete_local_after_upload)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("storage path '{name}' already exists"))
            } else {
                AppError::Internal(format!("failed to update storage path: {e}"))
            }
        })?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound("storage path not found".into()));
        }
        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: &str) -> AppResult<()> {
        let affected = sqlx::query("DELETE FROM storage_paths WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete storage path: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound("storage path not found".into()));
        }
        Ok(())
    }

    /// Count user_files whose bucket uses this storage path or a nested path under it.
    pub async fn file_count_for_path(pool: &PgPool, storage_path: &str) -> AppResult<i64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files uf
             INNER JOIN buckets b ON b.name = uf.bucket_name
             WHERE (b.path = $1 OR b.path LIKE $1 || '/%') AND uf.deleted_at IS NULL",
        )
        .bind(storage_path)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count files for path: {e}")))?;

        Ok(row.map(|(c,)| c).unwrap_or(0))
    }

    /// Count user_files whose bucket uses this storage path or a nested path under it (including deleted).
    pub async fn total_files_for_path(pool: &PgPool, storage_path: &str) -> AppResult<i64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files uf
             INNER JOIN buckets b ON b.name = uf.bucket_name
             WHERE (b.path = $1 OR b.path LIKE $1 || '/%')",
        )
        .bind(storage_path)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count files for path: {e}")))?;

        Ok(row.map(|(c,)| c).unwrap_or(0))
    }

    /// Total size of user_files whose bucket uses this storage path or a nested path under it.
    pub async fn total_size_for_path(pool: &PgPool, storage_path: &str) -> AppResult<i64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT COALESCE(SUM(f.size), 0)::BIGINT FROM user_files uf
             INNER JOIN buckets b ON b.name = uf.bucket_name
             INNER JOIN files f ON f.id = uf.file_id
             WHERE (b.path = $1 OR b.path LIKE $1 || '/%') AND uf.deleted_at IS NULL",
        )
        .bind(storage_path)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to sum size for path: {e}")))?;

        Ok(row.map(|(s,)| s).unwrap_or(0))
    }

    /// Number of buckets using this path or a nested path under it.
    pub async fn bucket_count_for_path(pool: &PgPool, storage_path: &str) -> AppResult<i64> {
        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT COUNT(*) FROM buckets WHERE (path = $1 OR path LIKE $1 || '/%')",
        )
        .bind(storage_path)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count buckets for path: {e}")))?;

        Ok(row.map(|(c,)| c).unwrap_or(0))
    }
}
