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

        sqlx::query(
            "INSERT INTO storage_objects (id, file_id, backend, storage_path, created_at) VALUES ($1, $2, $3, $4, $5)",
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
            "SELECT * FROM storage_objects WHERE id = $1",
        )
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to fetch storage object: {e}")))?
        .ok_or_else(|| AppError::Internal("storage object not found after insert".to_string()))?;

        Ok(StorageObject::from(row))
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
