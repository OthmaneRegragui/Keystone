use chrono::Utc;
use crate::db::is_unique_violation;
use crate::error::{AppError, AppResult};
use crate::models::Bucket;
use sqlx::PgPool;

pub struct BucketRepository;

impl BucketRepository {
    pub async fn create(pool: &PgPool, name: &str, path: &str) -> AppResult<Bucket> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO buckets (id, name, path, is_active, visible_to_users, created_at) VALUES ($1, $2, $3, true, true, $4)",
        )
        .bind(&id)
        .bind(name)
        .bind(path)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("bucket '{name}' already exists"))
            } else {
                AppError::Internal(format!("failed to create bucket: {e}"))
            }
        })?;

        Ok(Bucket {
            id,
            name: name.to_string(),
            path: path.to_string(),
            is_active: true,
            visible_to_users: true,
            storage_limit: 0,
            created_at: Utc::now(),
        })
    }

    pub async fn list(pool: &PgPool) -> AppResult<Vec<Bucket>> {
        let rows: Vec<(String, String, String, bool, bool, i64, String)> =
            sqlx::query_as("SELECT id, name, path, is_active, visible_to_users, storage_limit, created_at FROM buckets ORDER BY name ASC")
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list buckets: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(id, name, path, is_active, visible, storage_limit, created_at)| Bucket {
                id,
                name,
                path,
                is_active,
                visible_to_users: visible,
                storage_limit,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .unwrap_or_default()
                    .with_timezone(&Utc),
            })
            .collect())
    }

    pub async fn find_by_name(pool: &PgPool, name: &str) -> AppResult<Option<Bucket>> {
        let row: Option<(String, String, String, bool, bool, i64, String)> =
            sqlx::query_as("SELECT id, name, path, is_active, visible_to_users, storage_limit, created_at FROM buckets WHERE name = $1")
                .bind(name)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to find bucket: {e}")))?;

        Ok(row.map(|(id, name, path, is_active, visible, storage_limit, created_at)| Bucket {
            id,
            name,
            path,
            is_active,
            visible_to_users: visible,
            storage_limit,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
        }))
    }

    pub async fn find_by_id(pool: &PgPool, id: &str) -> AppResult<Option<Bucket>> {
        let row: Option<(String, String, String, bool, bool, i64, String)> =
            sqlx::query_as("SELECT id, name, path, is_active, visible_to_users, storage_limit, created_at FROM buckets WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to find bucket by id: {e}")))?;

        Ok(row.map(|(id, name, path, is_active, visible, storage_limit, created_at)| Bucket {
            id,
            name,
            path,
            is_active,
            visible_to_users: visible,
            storage_limit,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
        }))
    }

    pub async fn set_visible(pool: &PgPool, name: &str, visible: bool) -> AppResult<()> {
        let affected = sqlx::query("UPDATE buckets SET visible_to_users = $1 WHERE name = $2")
            .bind(visible)
            .bind(name)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update bucket visibility: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("bucket '{name}' not found")));
        }
        Ok(())
    }

    pub async fn update(pool: &PgPool, current_name: &str, new_name: &str, path: &str, visible: bool, active: bool, storage_limit: i64) -> AppResult<()> {
        let affected = sqlx::query("UPDATE buckets SET name = $1, path = $2, visible_to_users = $3, is_active = $4, storage_limit = $5 WHERE name = $6")
            .bind(new_name)
            .bind(path)
            .bind(visible)
            .bind(active)
            .bind(storage_limit)
            .bind(current_name)
            .execute(pool)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    AppError::Conflict(format!("bucket '{new_name}' already exists"))
                } else {
                    AppError::Internal(format!("failed to update bucket: {e}"))
                }
            })?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("bucket '{current_name}' not found")));
        }
        Ok(())
    }

    /// Update only the path of a bucket.
    pub async fn update_path(pool: &PgPool, name: &str, new_path: &str) -> AppResult<()> {
        let affected = sqlx::query("UPDATE buckets SET path = $1 WHERE name = $2")
            .bind(new_path)
            .bind(name)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update bucket path: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("bucket '{name}' not found")));
        }
        Ok(())
    }

    pub async fn delete(pool: &PgPool, name: &str) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM buckets WHERE name = $1")
            .bind(name)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete bucket: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::BadRequest(
                "bucket not found".to_string(),
            ));
        }
        Ok(true)
    }

    pub async fn get_storage_used_per_bucket(pool: &PgPool) -> AppResult<std::collections::HashMap<String, i64>> {
        let rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT so.backend, COALESCE(SUM(f.size), 0)::BIGINT as total
             FROM storage_objects so
             JOIN files f ON so.file_id = f.id
             GROUP BY so.backend",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get storage usage: {e}")))?;

        let mut map = std::collections::HashMap::new();
        for (backend, total) in rows {
            map.insert(backend, total);
        }
        Ok(map)
    }

    pub async fn list_visible_to_user(pool: &PgPool, user_id: &str) -> AppResult<Vec<Bucket>> {
        let rows: Vec<(String, String, String, bool, bool, i64, String)> = sqlx::query_as(
            "SELECT DISTINCT b.id, b.name, b.path, b.is_active, b.visible_to_users, b.storage_limit, b.created_at
             FROM buckets b
             INNER JOIN group_buckets gb ON gb.bucket_id = b.id
             INNER JOIN group_members gm ON gb.group_id = gm.group_id AND gm.user_id = $1
             WHERE b.is_active = true
             ORDER BY b.name ASC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list visible buckets: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(id, name, path, is_active, visible, storage_limit, created_at)| Bucket {
                id,
                name,
                path,
                is_active,
                visible_to_users: visible,
                storage_limit,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .unwrap_or_default()
                    .with_timezone(&Utc),
            })
            .collect())
    }

    /// Returns buckets accessible to a user with permissions merged across all groups.
    /// Only buckets the user has explicit group access to are returned.
    /// - `can_upload`: true if ANY group grants upload (OR logic)
    /// - `can_download`: true if ANY group grants download (OR logic)
    /// - `user_storage_limit`: MAX across all groups (0 = unlimited)
    pub async fn list_accessible_to_user(pool: &PgPool, user_id: &str) -> AppResult<Vec<AccessibleBucket>> {
        let rows: Vec<AccessibleBucketRow> = sqlx::query_as(
            "SELECT
                b.id,
                b.name,
                COALESCE(BOOL_OR(gb.can_upload), false) AS can_upload,
                COALESCE(BOOL_OR(gb.can_download), false) AS can_download,
                COALESCE(MAX(gb.user_storage_limit), 0) AS user_storage_limit
             FROM buckets b
             INNER JOIN group_buckets gb ON gb.bucket_id = b.id
             INNER JOIN group_members gm ON gb.group_id = gm.group_id AND gm.user_id = $1
             WHERE b.is_active = true
             GROUP BY b.id, b.name
             ORDER BY b.name ASC",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list accessible buckets: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|r| AccessibleBucket {
                id: r.id,
                name: r.name,
                can_upload: r.can_upload,
                can_download: r.can_download,
                user_storage_limit: r.user_storage_limit,
            })
            .collect())
    }
}

/// A bucket with merged permissions for a specific user.
#[derive(Debug)]
pub struct AccessibleBucket {
    pub id: String,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub user_storage_limit: i64,
}

#[derive(sqlx::FromRow)]
struct AccessibleBucketRow {
    id: String,
    name: String,
    can_upload: bool,
    can_download: bool,
    user_storage_limit: i64,
}
