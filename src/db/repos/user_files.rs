use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::UserFile;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::db::rows::user_file_row::{UserFileRecord, UserFileRow};

/// Flat row for JOIN queries between user_files and files tables.
#[derive(Debug, Clone, FromRow)]
struct UserFileWithMetaRow {
    pub id: String,
    pub user_id: String,
    pub file_id: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub created_at: String,
    pub bucket_name: Option<String>,
    pub folder_id: Option<String>,
    pub deleted_at: Option<String>,
    pub blake3_hash: String,
    pub size: i64,
    pub ref_count: i32,
}

impl UserFileWithMetaRow {
    fn into_parts(self) -> (UserFile, String, i64, i32) {
        let uf = UserFile::from(UserFileRow {
            id: self.id,
            user_id: self.user_id,
            file_id: self.file_id,
            original_name: self.original_name,
            mime_type: self.mime_type,
            created_at: self.created_at,
            bucket_name: self.bucket_name,
            folder_id: self.folder_id,
            deleted_at: self.deleted_at,
        });
        (uf, self.blake3_hash, self.size, self.ref_count)
    }
}

/// Flat row for admin exports: user_file + file + user info.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserFileExportRow {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub file_id: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub created_at: String,
    pub bucket_name: Option<String>,
    pub folder_id: Option<String>,
    pub blake3_hash: String,
    pub size: i64,
}

pub struct UserFileRepository;

impl UserFileRepository {
    /// Create a user_file entry linking a user to a file.
    pub async fn create(pool: &PgPool, record: UserFileRecord) -> AppResult<UserFile> {
        let now = Utc::now().to_rfc3339();
        let id = record.id.to_string();

        sqlx::query(
            r#"INSERT INTO user_files (id, user_id, file_id, original_name, mime_type, created_at, bucket_name, folder_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        )
        .bind(&id)
        .bind(record.user_id.to_string())
        .bind(record.file_id.to_string())
        .bind(&record.original_name)
        .bind(&record.mime_type)
        .bind(&now)
        .bind(&record.bucket_name)
        .bind(record.folder_id.map(|f| f.to_string()))
        .execute(pool)
        .await
        .map_err(|e| {
            if crate::db::is_unique_violation(&e) {
                AppError::FileAlreadyExists(format!(
                    "a file named '{}' already exists in this location",
                    record.original_name
                ))
            } else {
                AppError::Internal(format!("failed to insert user_file: {e}"))
            }
        })?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("user_file not found after insert".to_string()))
    }

    /// Find a user_file by its own ID (excludes soft-deleted).
    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Find a user_file by user_id + file_id (to check if user already has this file).
    pub async fn find_by_user_and_file(
        pool: &PgPool,
        user_id: Uuid,
        file_id: Uuid,
    ) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE user_id = $1 AND file_id = $2 AND deleted_at IS NULL",
        )
        .bind(user_id.to_string())
        .bind(file_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Find a soft-deleted user_file by user_id + file_id + original_name (for re-upload restoration).
    pub async fn find_deleted_by_user_file_and_name(
        pool: &PgPool,
        user_id: Uuid,
        file_id: Uuid,
        original_name: &str,
    ) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE user_id = $1 AND file_id = $2 AND original_name = $3 AND deleted_at IS NOT NULL LIMIT 1",
        )
        .bind(user_id.to_string())
        .bind(file_id.to_string())
        .bind(original_name)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query soft-deleted user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Find an ACTIVE (non-deleted) user_file by user_id + file_id + original_name.
    /// This is the pre-insert duplicate check for the unique index
    /// `idx_user_files_user_file (user_id, file_id, original_name)`.
    pub async fn find_active_by_user_file_and_name(
        pool: &PgPool,
        user_id: Uuid,
        file_id: Uuid,
        original_name: &str,
    ) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE user_id = $1 AND file_id = $2 AND original_name = $3 AND deleted_at IS NULL LIMIT 1",
        )
        .bind(user_id.to_string())
        .bind(file_id.to_string())
        .bind(original_name)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Restore a soft-deleted user_file (set deleted_at back to NULL).
    pub async fn restore(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("UPDATE user_files SET deleted_at = NULL WHERE id = $1 AND deleted_at IS NOT NULL")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to restore user_file: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    /// List all user_files for a specific user, with pagination, search, bucket filter, and folder filter.
    /// Joins with the files table to get size, hash, and ref_count.
    pub async fn list_by_user(
        pool: &PgPool,
        user_id: Uuid,
        offset: i64,
        limit: i64,
        search: Option<&str>,
        bucket: Option<&str>,
        folder_id: Option<Uuid>,
    ) -> AppResult<Vec<(UserFile, String, i64, i32)>> {
        // Returns (user_file, blake3_hash, size, ref_count)
        let user_id_str = user_id.to_string();
        let folder_str = folder_id.map(|f| f.to_string());

        let mut where_clauses = vec!["uf.user_id = $1".to_string(), "uf.deleted_at IS NULL".to_string()];
        let mut bind_values: Vec<String> = vec![user_id_str.clone()];
        let mut param_idx = 2;

        if let Some(b) = bucket {
            where_clauses.push(format!("uf.bucket_name = ${param_idx}"));
            bind_values.push(b.to_string());
            param_idx += 1;
        }

        match folder_str.as_deref() {
            Some(fid) if !fid.is_empty() => {
                // Specific folder: show files in that folder
                where_clauses.push(format!("uf.folder_id = ${param_idx}"));
                bind_values.push(fid.to_string());
                param_idx += 1;
            }
            _ => {
                // Root (no folder_id provided): only show root-level files
                where_clauses.push("uf.folder_id IS NULL".to_string());
            }
        }

        if let Some(s) = search {
            let pattern = format!("%{s}%");
            where_clauses.push(format!("(uf.original_name ILIKE ${param_idx} OR f.blake3_hash ILIKE ${param_idx})"));
            bind_values.push(pattern);
            param_idx += 1;
        }

        let sql = format!(
            r#"SELECT uf.*, f.blake3_hash, f.size, f.ref_count
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE {where_clause}
               ORDER BY uf.created_at DESC
               LIMIT ${param_idx} OFFSET ${param_idx_plus_one}"#,
            where_clause = where_clauses.join(" AND "),
            param_idx = param_idx,
            param_idx_plus_one = param_idx + 1,
        );

        let mut query = sqlx::query_as::<_, UserFileWithMetaRow>(&sql);
        for val in &bind_values {
            query = query.bind(val);
        }
        query = query.bind(limit).bind(offset);

        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to list user files: {e}")))?;

        Ok(rows.into_iter().map(|row| row.into_parts()).collect())
    }

    /// Count total files for a user with optional search, bucket, and folder filter.
    pub async fn count_by_user(
        pool: &PgPool,
        user_id: Uuid,
        search: Option<&str>,
        bucket: Option<&str>,
        folder_id: Option<Uuid>,
    ) -> AppResult<i64> {
        let user_id_str = user_id.to_string();
        let folder_str = folder_id.map(|f| f.to_string());

        let mut where_clauses = vec!["uf.user_id = $1".to_string(), "uf.deleted_at IS NULL".to_string()];
        let mut bind_values: Vec<String> = vec![user_id_str.clone()];
        let mut param_idx = 2;

        if let Some(b) = bucket {
            where_clauses.push(format!("uf.bucket_name = ${param_idx}"));
            bind_values.push(b.to_string());
            param_idx += 1;
        }

        match folder_str.as_deref() {
            Some(fid) if !fid.is_empty() => {
                where_clauses.push(format!("uf.folder_id = ${param_idx}"));
                bind_values.push(fid.to_string());
                param_idx += 1;
            }
            _ => {
                where_clauses.push("uf.folder_id IS NULL".to_string());
            }
        }

        if let Some(s) = search {
            let pattern = format!("%{s}%");
            where_clauses.push(format!("(uf.original_name ILIKE ${param_idx} OR f.blake3_hash ILIKE ${param_idx})"));
            bind_values.push(pattern);
        }

        let sql = format!(
            r#"SELECT COUNT(*)
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE {where_clause}"#,
            where_clause = where_clauses.join(" AND "),
        );

        let mut query = sqlx::query_as::<_, (i64,)>(&sql);
        for val in &bind_values {
            query = query.bind(val);
        }

        let result = query
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to count user files: {e}")))?;

        Ok(result.0)
    }

    /// Soft-delete a user_file entry (set deleted_at timestamp instead of removing the row).
    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query("UPDATE user_files SET deleted_at = $1 WHERE id = $2 AND deleted_at IS NULL")
            .bind(&now)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to soft-delete user_file: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    /// Find a user_file by user_id and user_file id (ownership check, excludes soft-deleted).
    pub async fn find_by_user_and_id(
        pool: &PgPool,
        user_id: Uuid,
        user_file_id: Uuid,
    ) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE user_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(user_id.to_string())
        .bind(user_file_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Rename a user_file's original_name.
    pub async fn update_name(pool: &PgPool, id: Uuid, new_name: &str) -> AppResult<bool> {
        let affected = sqlx::query("UPDATE user_files SET original_name = $1 WHERE id = $2")
            .bind(new_name)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to rename user_file: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Refresh the stored MIME type of a user_file (used when an overwrite or
    /// restore re-uploads content whose client Content-Type has changed).
    pub async fn update_mime_type(pool: &PgPool, id: Uuid, mime_type: &str) -> AppResult<bool> {
        let affected = sqlx::query("UPDATE user_files SET mime_type = $1 WHERE id = $2")
            .bind(mime_type)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update mime_type: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Move a user_file to a different folder (None = root).
    pub async fn update_folder(pool: &PgPool, id: Uuid, folder_id: Option<Uuid>) -> AppResult<bool> {
        let affected = sqlx::query("UPDATE user_files SET folder_id = $1 WHERE id = $2")
            .bind(folder_id.map(|f| f.to_string()))
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to move user_file: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Move a user_file to a different bucket and/or folder (for cross-bucket moves).
    pub async fn update_bucket_and_folder(
        pool: &PgPool,
        id: Uuid,
        bucket_name: Option<String>,
        folder_id: Option<Uuid>,
    ) -> AppResult<bool> {
        let affected = sqlx::query(
            "UPDATE user_files SET bucket_name = $1, folder_id = $2 WHERE id = $3",
        )
        .bind(&bucket_name)
        .bind(folder_id.map(|f| f.to_string()))
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to move user_file: {e}")))?
        .rows_affected();
        Ok(affected > 0)
    }

    // ── Soft-delete admin stats ──

    /// Count and total size of soft-deleted user_files per bucket.
    /// Returns Vec<(bucket_name, count, total_size)>.
    pub async fn deleted_stats_per_bucket(
        pool: &PgPool,
    ) -> AppResult<Vec<(String, i64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"SELECT uf.bucket_name, COUNT(*) as cnt, COALESCE(SUM(f.size), 0)::BIGINT as total
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.deleted_at IS NOT NULL
               GROUP BY uf.bucket_name"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get deleted stats per bucket: {e}")))?;

        Ok(rows)
    }

    /// Global count and total size of all soft-deleted user_files.
    /// Returns (count, total_size).
    pub async fn deleted_stats_global(
        pool: &PgPool,
    ) -> AppResult<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            r#"SELECT COUNT(*), COALESCE(SUM(f.size), 0)::BIGINT
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.deleted_at IS NOT NULL"#,
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get global deleted stats: {e}")))?;

        Ok(row)
    }

    /// List all non-deleted user_files in a bucket, across ALL users (admin only).
    /// Returns file metadata + username for each entry.
    pub async fn list_by_bucket_for_export(
        pool: &PgPool,
        bucket_name: &str,
    ) -> AppResult<Vec<UserFileExportRow>> {
        let rows = sqlx::query_as::<_, UserFileExportRow>(
            r#"SELECT uf.id, uf.user_id, u.username, uf.file_id,
                      uf.original_name, uf.mime_type, uf.created_at,
                      uf.bucket_name, uf.folder_id,
                      f.blake3_hash, f.size
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               JOIN users u ON uf.user_id = u.id
               WHERE uf.bucket_name = $1 AND uf.deleted_at IS NULL
               ORDER BY u.username, uf.created_at"#,
        )
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list bucket files for export: {e}")))?;

        Ok(rows)
    }

    /// Count of active (non-deleted) user_files.
    pub async fn count_active(pool: &PgPool) -> AppResult<i64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files WHERE deleted_at IS NULL",
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count active user files: {e}")))?;

        Ok(row.0)
    }

    /// Total size of active (non-deleted) user_files.
    pub async fn size_active(pool: &PgPool) -> AppResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COALESCE(SUM(f.size), 0)::BIGINT
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.deleted_at IS NULL"#,
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to sum active file sizes: {e}")))?;

        Ok(row.0)
    }

    /// Total size of active (non-deleted) user_files for one user in one bucket.
    /// Used to enforce the per-user storage limit of a group bucket
    /// (`group_buckets.user_storage_limit`, 0 = unlimited).
    pub async fn sum_active_size_by_user_and_bucket(
        pool: &PgPool,
        user_id: Uuid,
        bucket_name: &str,
    ) -> AppResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COALESCE(SUM(f.size), 0)::BIGINT
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.user_id = $1 AND uf.bucket_name = $2 AND uf.deleted_at IS NULL"#,
        )
        .bind(user_id.to_string())
        .bind(bucket_name)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to sum user bucket file sizes: {e}")))?;

        Ok(row.0)
    }

    /// Aggregated usage for a user's active files: (total_files, storage_used,
    /// duplicates_saved). `duplicates_saved` is the disk space the user did not
    /// have to spend because their content deduplicated against existing
    /// physical files (`SUM((f.ref_count - 1) * f.size)`).
    pub async fn summarize_by_user(pool: &PgPool, user_id: Uuid) -> AppResult<(i64, i64, i64)> {
        let row: (i64, i64, i64) = sqlx::query_as(
            r#"SELECT
                   COUNT(*)::BIGINT,
                   COALESCE(SUM(f.size), 0)::BIGINT,
                   COALESCE(SUM((f.ref_count - 1) * f.size), 0)::BIGINT
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.user_id = $1 AND uf.deleted_at IS NULL"#,
        )
        .bind(user_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to summarize user files: {e}")))?;

        Ok(row)
    }

    /// Most recently added active files for a user (any folder/bucket).
    pub async fn recent_by_user(
        pool: &PgPool,
        user_id: Uuid,
        limit: i64,
    ) -> AppResult<Vec<(UserFile, String, i64, i32)>> {
        let rows = sqlx::query_as::<_, UserFileWithMetaRow>(
            r#"SELECT uf.*, f.blake3_hash, f.size, f.ref_count
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.user_id = $1 AND uf.deleted_at IS NULL
               ORDER BY uf.created_at DESC
               LIMIT $2"#,
        )
        .bind(user_id.to_string())
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list recent user files: {e}")))?;

        Ok(rows.into_iter().map(|row| row.into_parts()).collect())
    }

    /// Count of soft-deleted user_files.
    pub async fn count_deleted(pool: &PgPool) -> AppResult<i64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files WHERE deleted_at IS NOT NULL",
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count deleted user files: {e}")))?;

        Ok(row.0)
    }
    /// Total size of soft-deleted user_files.
    pub async fn size_deleted(pool: &PgPool) -> AppResult<i64> {
        let row: (i64,) = sqlx::query_as(
            r#"SELECT COALESCE(SUM(f.size), 0)::BIGINT
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.deleted_at IS NOT NULL"#,
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to sum deleted file sizes: {e}")))?;

        Ok(row.0)
    }

    /// Count and total size of ORPHANED physical files:
    /// files where ALL user_files references are soft-deleted (no active reference exists).
    /// These files waste disk space but nobody can use them.
    /// Returns (count, total_size).
    pub async fn orphaned_physical_files_global(
        pool: &PgPool,
    ) -> AppResult<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            r#"SELECT COUNT(DISTINCT f.id), COALESCE(SUM(DISTINCT f.size), 0)::BIGINT
               FROM files f
               WHERE EXISTS (
                   SELECT 1 FROM user_files uf WHERE uf.file_id = f.id AND uf.deleted_at IS NOT NULL
               )
               AND NOT EXISTS (
                   SELECT 1 FROM user_files uf WHERE uf.file_id = f.id AND uf.deleted_at IS NULL
               )"#,
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count orphaned physical files: {e}")))?;

        Ok(row)
    }

    /// Per-bucket orphaned physical files stats:
    /// physical files where ALL references in this bucket are soft-deleted.
    /// Returns Vec<(bucket_name, orphaned_count, orphaned_size)>.
    pub async fn orphaned_physical_files_per_bucket(
        pool: &PgPool,
    ) -> AppResult<Vec<(String, i64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"SELECT uf.bucket_name, COUNT(DISTINCT f.id), COALESCE(SUM(DISTINCT f.size), 0)::BIGINT
               FROM files f
               JOIN user_files uf ON uf.file_id = f.id AND uf.deleted_at IS NOT NULL
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf2 WHERE uf2.file_id = f.id AND uf2.deleted_at IS NULL
               )
               GROUP BY uf.bucket_name"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get orphaned stats per bucket: {e}")))?;

        Ok(rows)
    }
}
