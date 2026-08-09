use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::File;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::{FileRecord, FileRow};

pub struct FileRepository;

impl FileRepository {
    pub async fn create(pool: &PgPool, record: FileRecord) -> AppResult<File> {
        let now = Utc::now().to_rfc3339();
        let id = record.id.to_string();

        sqlx::query(
            r#"INSERT INTO files (id, blake3_hash, original_name, mime_type, size, ref_count, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, 1, $6, $6)"#,
        )
        .bind(&id)
        .bind(&record.blake3_hash)
        .bind(&record.original_name)
        .bind(&record.mime_type)
        .bind(record.size)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert file: {e}")))?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("file not found after insert".to_string()))
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<File>> {
        let row = sqlx::query_as::<_, FileRow>("SELECT * FROM files WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file: {e}")))?;

        Ok(row.map(File::from))
    }

    pub async fn find_by_hash(pool: &PgPool, hash: &str) -> AppResult<Option<File>> {
        let row = sqlx::query_as::<_, FileRow>("SELECT * FROM files WHERE blake3_hash = $1")
            .bind(hash)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query file by hash: {e}")))?;

        Ok(row.map(File::from))
    }

    pub async fn list(
        pool: &PgPool,
        offset: i64,
        limit: i64,
        search: Option<&str>,
    ) -> AppResult<Vec<File>> {
        let rows = match search {
            Some(s) => {
                let pattern = format!("%{s}%");
                sqlx::query_as::<_, FileRow>(
                    "SELECT * FROM files WHERE original_name ILIKE $1 OR blake3_hash ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
                )
                .bind(&pattern)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await
            }
            None => {
                sqlx::query_as::<_, FileRow>(
                    "SELECT * FROM files ORDER BY created_at DESC LIMIT $1 OFFSET $2",
                )
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await
            }
        }
        .map_err(|e| AppError::Internal(format!("failed to list files: {e}")))?;

        Ok(rows.into_iter().map(File::from).collect())
    }

    pub async fn count(pool: &PgPool, search: Option<&str>) -> AppResult<i64> {
        let result: (i64,) = match search {
            Some(s) => {
                let pattern = format!("%{s}%");
                sqlx::query_as(
                    "SELECT COUNT(*) FROM files WHERE original_name ILIKE $1 OR blake3_hash ILIKE $1",
                )
                .bind(&pattern)
                .fetch_one(pool)
                .await
            }
            None => {
                sqlx::query_as("SELECT COUNT(*) FROM files")
                    .fetch_one(pool)
                    .await
            }
        }
        .map_err(|e| AppError::Internal(format!("failed to count files: {e}")))?;

        Ok(result.0)
    }

    pub async fn update_ref_count(pool: &PgPool, id: Uuid, delta: i32) -> AppResult<()> {
        let affected = sqlx::query(
            "UPDATE files SET ref_count = ref_count + $1, updated_at = $2 WHERE id = $3",
        )
        .bind(delta)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update ref count: {e}")))?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("file {id} not found")));
        }
        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM files WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete file: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    pub async fn get_zero_ref_files(pool: &PgPool, limit: i64) -> AppResult<Vec<File>> {
        let rows = sqlx::query_as::<_, FileRow>(
            "SELECT * FROM files WHERE ref_count <= 0 ORDER BY updated_at ASC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get zero-ref files: {e}")))?;

        Ok(rows.into_iter().map(File::from).collect())
    }
}
