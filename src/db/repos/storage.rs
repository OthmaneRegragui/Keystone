use crate::error::{AppError, AppResult};
use crate::models::StorageObject;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::storage_object_row::{CreateStorageObjectData, StorageObjectRow};

pub struct StorageObjectRepository;

impl StorageObjectRepository {
    pub async fn create(pool: &PgPool, data: CreateStorageObjectData) -> AppResult<StorageObject> {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        // Idempotent per `(file_id, backend)`: a concurrent upload of the same
        // content written to the same backend must not create a second row.
        sqlx::query(
            "INSERT INTO storage_objects (id, file_id, backend, storage_path, created_at) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (file_id, backend) DO NOTHING",
        )
        .bind(&id)
        .bind(data.file_id.to_string())
        .bind(&data.backend)
        .bind(&data.storage_path)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert storage object: {e}")))?;

        let row = sqlx::query_as::<_, StorageObjectRow>(
            "SELECT * FROM storage_objects WHERE file_id = $1 AND backend = $2",
        )
        .bind(data.file_id.to_string())
        .bind(&data.backend)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to fetch storage object: {e}")))?
        .ok_or_else(|| AppError::Internal("storage object not found after insert".to_string()))?;

        Ok(StorageObject::from(row))
    }

    /// Number of *other* physical files with a live reference that point at the
    /// same storage path. GC uses this as a safety net so a blob is never
    /// deleted while another `files` row still references it.
    pub async fn count_other_files_for_path(
        pool: &PgPool,
        storage_path: &str,
        exclude_file_id: Uuid,
    ) -> AppResult<i64> {
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*)::BIGINT
               FROM storage_objects so
               JOIN files f ON f.id = so.file_id
               WHERE so.storage_path = $1 AND so.file_id <> $2 AND f.ref_count > 0"#,
        )
        .bind(storage_path)
        .bind(exclude_file_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count other files for path: {e}")))?;

        Ok(count)
    }

    pub async fn find_by_file_id(
        pool: &PgPool,
        file_id: Uuid,
    ) -> AppResult<Vec<StorageObject>> {
        let rows = sqlx::query_as::<_, StorageObjectRow>(
            "SELECT * FROM storage_objects WHERE file_id = $1 ORDER BY created_at DESC",
        )
        .bind(file_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query storage objects: {e}")))?;

        Ok(rows.into_iter().map(StorageObject::from).collect())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM storage_objects WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete storage object: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    pub async fn find_orphaned(pool: &PgPool) -> AppResult<Vec<StorageObject>> {
        let rows = sqlx::query_as::<_, StorageObjectRow>(
            r#"SELECT so.* FROM storage_objects so
               LEFT JOIN files f ON so.file_id = f.id
               WHERE f.id IS NULL"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find orphaned storage objects: {e}")))?;

        Ok(rows.into_iter().map(StorageObject::from).collect())
    }

    /// List all storage objects for a given backend (bucket name).
    pub async fn list_by_backend(pool: &PgPool, backend: &str) -> AppResult<Vec<StorageObject>> {
        let rows = sqlx::query_as::<_, StorageObjectRow>(
            "SELECT * FROM storage_objects WHERE backend = $1",
        )
        .bind(backend)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list storage objects by backend: {e}")))?;

        Ok(rows.into_iter().map(StorageObject::from).collect())
    }
}
