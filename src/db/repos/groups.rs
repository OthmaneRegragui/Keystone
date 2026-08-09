use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::UserGroup;
use sqlx::PgPool;

pub struct GroupRepository;

impl GroupRepository {
    pub async fn create(pool: &PgPool, name: &str) -> AppResult<UserGroup> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query("INSERT INTO user_groups (id, name, created_at, allow_api_keys, allow_password_change) VALUES ($1, $2, $3, FALSE, FALSE)")
            .bind(&id)
            .bind(name)
            .bind(&now)
            .execute(pool)
            .await
            .map_err(|e| {
                if crate::db::is_unique_violation(&e) {
                    AppError::Conflict(format!("group '{name}' already exists"))
                } else {
                    AppError::Internal(format!("failed to create group: {e}"))
                }
            })?;

        Ok(UserGroup {
            id,
            name: name.to_string(),
            created_at: Utc::now(),
            allow_api_keys: false,
            allow_password_change: false,
        })
    }

    pub async fn list(pool: &PgPool) -> AppResult<Vec<UserGroup>> {
        let rows: Vec<(String, String, String, bool, bool)> =
            sqlx::query_as("SELECT id, name, created_at, allow_api_keys, allow_password_change FROM user_groups ORDER BY name ASC")
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list groups: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|(id, name, created_at, allow_api_keys, allow_password_change)| UserGroup {
                id,
                name,
                created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                    .unwrap_or_default()
                    .with_timezone(&Utc),
                allow_api_keys,
                allow_password_change,
            })
            .collect())
    }

    pub async fn delete(pool: &PgPool, id: &str) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM user_groups WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete group: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn add_member(pool: &PgPool, group_id: &str, user_id: &str) -> AppResult<()> {
        sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(group_id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to add member: {e}")))?;
        Ok(())
    }

    /// Grant every user in `user_ids` membership to every group in `group_ids`.
    /// All-or-nothing via a single transaction; existing memberships are kept
    /// (ON CONFLICT DO NOTHING). Returns the number of rows actually inserted.
    pub async fn add_members_to_groups(pool: &PgPool, user_ids: &[String], group_ids: &[String]) -> AppResult<usize> {
        if user_ids.is_empty() || group_ids.is_empty() {
            return Ok(0);
        }
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin transaction: {e}")))?;
        let mut added: usize = 0;
        for gid in group_ids {
            for uid in user_ids {
                let affected = sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                    .bind(gid)
                    .bind(uid)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| AppError::Internal(format!("failed to add user {uid} to group {gid}: {e}")))?
                    .rows_affected();
                added += affected as usize;
            }
        }
        tx.commit()
            .await
            .map_err(|e| AppError::Internal(format!("failed to commit bulk membership: {e}")))?;
        Ok(added)
    }

    pub async fn remove_member(pool: &PgPool, group_id: &str, user_id: &str) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
            .bind(group_id)
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to remove member: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn list_members(pool: &PgPool, group_id: &str) -> AppResult<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT user_id FROM group_members WHERE group_id = $1")
                .bind(group_id)
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list members: {e}")))?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn add_bucket(pool: &PgPool, group_id: &str, bucket_id: &str, user_storage_limit: i64) -> AppResult<()> {
        sqlx::query("INSERT INTO group_buckets (group_id, bucket_id, user_storage_limit) VALUES ($1, $2, $3) ON CONFLICT (group_id, bucket_id) DO UPDATE SET user_storage_limit = EXCLUDED.user_storage_limit, can_upload = DEFAULT, can_download = DEFAULT")
            .bind(group_id)
            .bind(bucket_id)
            .bind(user_storage_limit)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to add bucket to group: {e}")))?;
        Ok(())
    }

    pub async fn remove_bucket(pool: &PgPool, group_id: &str, bucket_id: &str) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM group_buckets WHERE group_id = $1 AND bucket_id = $2")
            .bind(group_id)
            .bind(bucket_id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to remove bucket from group: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    pub async fn list_buckets(pool: &PgPool, group_id: &str) -> AppResult<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT bucket_id FROM group_buckets WHERE group_id = $1")
                .bind(group_id)
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list group buckets: {e}")))?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn list_group_bucket_details(
        pool: &PgPool,
        group_id: &str,
    ) -> AppResult<Vec<(String, String, String, i64, i64, i64, i64, bool, bool)>> {
        let rows: Vec<(String, String, String, i64, i64, i64, i64, bool, bool)> = sqlx::query_as(
            "SELECT b.id, b.name, b.path, COALESCE(su.total, 0), b.storage_limit, gb.user_storage_limit,
                    (SELECT COUNT(DISTINCT gm.user_id)
                     FROM group_buckets gb2
                     JOIN group_members gm ON gm.group_id = gb2.group_id
                     WHERE gb2.bucket_id = b.id) AS user_count,
                    gb.can_upload, gb.can_download
             FROM group_buckets gb
             JOIN buckets b ON gb.bucket_id = b.id
             LEFT JOIN (
                 SELECT so.backend, SUM(f.size)::BIGINT as total
                 FROM storage_objects so
                 JOIN files f ON so.file_id = f.id
                 GROUP BY so.backend
             ) su ON su.backend = b.name
             WHERE gb.group_id = $1
             ORDER BY b.name ASC",
        )
        .bind(group_id)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list group bucket details: {e}")))?;
        Ok(rows.into_iter().map(|(id, name, path, storage_used, bucket_limit, user_limit, user_count, upload, download)| (id, name, path, storage_used, bucket_limit, user_limit, user_count, upload, download)).collect())
    }

    pub async fn update_bucket_permissions(
        pool: &PgPool,
        group_id: &str,
        bucket_id: &str,
        can_upload: bool,
        can_download: bool,
    ) -> AppResult<()> {
        let affected = sqlx::query(
            "UPDATE group_buckets SET can_upload = $1, can_download = $2 WHERE group_id = $3 AND bucket_id = $4",
        )
        .bind(can_upload)
        .bind(can_download)
        .bind(group_id)
        .bind(bucket_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update bucket permissions: {e}")))?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound("group-bucket link not found".into()));
        }
        Ok(())
    }

    pub async fn set_user_storage_limit(
        pool: &PgPool,
        group_id: &str,
        bucket_id: &str,
        user_storage_limit: i64,
    ) -> AppResult<()> {
        let affected = sqlx::query(
            "UPDATE group_buckets SET user_storage_limit = $1 WHERE group_id = $2 AND bucket_id = $3",
        )
        .bind(user_storage_limit)
        .bind(group_id)
        .bind(bucket_id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to set user storage limit: {e}")))?
        .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound("group-bucket link not found".into()));
        }
        Ok(())
    }

    pub async fn list_user_groups(pool: &PgPool, user_id: &str) -> AppResult<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT group_id FROM group_members WHERE user_id = $1")
                .bind(user_id)
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list user groups: {e}")))?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    pub async fn set_user_groups(pool: &PgPool, user_id: &str, group_ids: &[String]) -> AppResult<()> {
        sqlx::query("DELETE FROM group_members WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to clear user groups: {e}")))?;

        for gid in group_ids {
            sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
                .bind(gid)
                .bind(user_id)
                .execute(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to add user to group: {e}")))?;
        }
        Ok(())
    }

    pub async fn get_by_id(pool: &PgPool, id: &str) -> AppResult<Option<UserGroup>> {
        let row: Option<(String, String, String, bool, bool)> =
            sqlx::query_as("SELECT id, name, created_at, allow_api_keys, allow_password_change FROM user_groups WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to get group: {e}")))?;

        Ok(row.map(|(id, name, created_at, allow_api_keys, allow_password_change)| UserGroup {
            id,
            name,
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
            allow_api_keys,
            allow_password_change,
        }))
    }

    pub async fn update_permissions(
        pool: &PgPool,
        id: &str,
        allow_api_keys: bool,
        allow_password_change: bool,
    ) -> AppResult<bool> {
        let affected = sqlx::query(
            "UPDATE user_groups SET allow_api_keys = $1, allow_password_change = $2 WHERE id = $3",
        )
        .bind(allow_api_keys)
        .bind(allow_password_change)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update group permissions: {e}")))?
        .rows_affected();
        Ok(affected > 0)
    }

    /// Whether the user is a member of at least one group with `allow_api_keys`
    /// enabled. Enforcement uses ANY-group-allow semantics: a single group that
    /// permits it is enough for the member.
    pub async fn user_allows_api_keys(pool: &PgPool, user_id: &str) -> AppResult<bool> {
        let (exists,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(
                SELECT 1
                FROM group_members gm
                JOIN user_groups g ON g.id = gm.group_id
                WHERE gm.user_id = $1 AND g.allow_api_keys
            )",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to check api key group permission: {e}")))?;
        Ok(exists)
    }

    /// Whether the user is a member of at least one group with
    /// `allow_password_change` enabled. Same ANY-group-allow semantics as
    /// [`GroupRepository::user_allows_api_keys`].
    pub async fn user_allows_password_change(pool: &PgPool, user_id: &str) -> AppResult<bool> {
        let (exists,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(
                SELECT 1
                FROM group_members gm
                JOIN user_groups g ON g.id = gm.group_id
                WHERE gm.user_id = $1 AND g.allow_password_change
            )",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to check password change group permission: {e}")))?;
        Ok(exists)
    }
}
