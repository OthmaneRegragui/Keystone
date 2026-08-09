use chrono::Utc;
use crate::db::is_unique_violation;
use crate::error::{AppError, AppResult};
use crate::models::StoragePath;
use sqlx::PgPool;

pub struct StoragePathRepository;

impl StoragePathRepository {
    pub async fn list(pool: &PgPool) -> AppResult<Vec<StoragePath>> {
        let rows: Vec<(String, String, String, String)> =
            sqlx::query_as("SELECT id, name, path, created_at FROM storage_paths ORDER BY name ASC")
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list storage paths: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(id, name, path, created_at)| StoragePath {
                id,
                name,
                path,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .unwrap_or_default()
                    .with_timezone(&Utc),
            })
            .collect())
    }

    pub async fn find_by_id(pool: &PgPool, id: &str) -> AppResult<Option<StoragePath>> {
        let row: Option<(String, String, String, String)> =
            sqlx::query_as("SELECT id, name, path, created_at FROM storage_paths WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to find storage path: {e}")))?;

        Ok(row.map(|(id, name, path, created_at)| StoragePath {
            id,
            name,
            path,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
        }))
    }

    pub async fn create(pool: &PgPool, name: &str, path: &str) -> AppResult<StoragePath> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO storage_paths (id, name, path, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&id)
        .bind(name)
        .bind(path)
        .bind(&now)
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
        })
    }

    pub async fn update(pool: &PgPool, id: &str, name: &str, path: &str) -> AppResult<()> {
        let affected = sqlx::query("UPDATE storage_paths SET name = $2, path = $3 WHERE id = $1")
            .bind(id)
            .bind(name)
            .bind(path)
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
