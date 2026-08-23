use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::{Bot, BotPathRule, BotRuleStatus, UserFile};
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
    pub purged_at: Option<String>,
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
            purged_at: self.purged_at,
        });
        (uf, self.blake3_hash, self.size, self.ref_count)
    }
}

/// Escape `LIKE` wildcards so rule paths match literally under `ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            c => out.push(c),
        }
    }
    out
}

/// The buckets that appear in a rule list, in first-appearance order.
fn distinct_buckets(rules: &[BotPathRule]) -> Vec<&str> {
    let mut seen: Vec<&str> = Vec::new();
    for r in rules {
        if !seen.contains(&r.bucket.as_str()) {
            seen.push(&r.bucket);
        }
    }
    seen
}

/// Build a SQL condition (plus its bound values) that restricts the `file_rows`
/// derived table to the files the bot's path rules allow.
///
/// Per bucket the semantics match [`Bot::path_allowed`]:
///   - a bucket with no rules is fully accessible;
///   - a bucket with rules is fail-closed: an allow rule must cover the file's
///     path and no block rule may cover it (block wins);
///   - private files (NULL bucket) always pass.
///
/// `file_path` is the folder path + original name computed by the recursive CTE
/// in the enclosing query. Returns `None` when the bot carries no rules.
fn bot_path_filter(bot: &Bot, param_idx: &mut usize, binds: &mut Vec<String>) -> Option<String> {
    let rules = bot.path_rules.as_ref()?;
    if rules.is_empty() {
        return None;
    }

    let mut bucket_conds = Vec::new();
    for bucket in distinct_buckets(rules) {
        let mut allows: Vec<String> = Vec::new();
        let mut blocks: Vec<String> = Vec::new();

        for r in rules.iter().filter(|r| r.bucket == bucket) {
            let path = r.path.trim_end_matches('/').to_string();
            let path_idx = *param_idx;
            *param_idx += 1;
            let like_idx = *param_idx;
            *param_idx += 1;
            let like_pat = format!("{}/%", like_escape(&path));
            let cond = format!(
                "(${path_idx} = '' OR file_path = ${path_idx} OR file_path LIKE ${like_idx} ESCAPE '\\')"
            );
            binds.push(path);
            binds.push(like_pat);
            if r.status == BotRuleStatus::Allow {
                allows.push(cond);
            } else {
                blocks.push(cond);
            }
        }

        let bucket_idx = *param_idx;
        *param_idx += 1;
        binds.push(bucket.to_string());

        let allow_clause = if allows.is_empty() {
            "FALSE".to_string()
        } else {
            format!("({})", allows.join(" OR "))
        };
        let block_clause = if blocks.is_empty() {
            "TRUE".to_string()
        } else {
            format!("NOT ({})", blocks.join(" OR "))
        };
        bucket_conds.push(format!(
            "(bucket_name = ${bucket_idx} AND {allow_clause} AND {block_clause})"
        ));
    }

    Some(format!(
        "(bucket_name IS NULL OR {})",
        bucket_conds.join(" OR ")
    ))
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

/// Flat row for the admin orphaned-files view: a physical file with no
/// remaining active references, annotated with its most recent reference
/// (name, bucket, owner, when it became unreachable).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OrphanedFileRow {
    pub file_id: String,
    pub blake3_hash: String,
    pub original_name: String,
    pub size: i64,
    pub created_at: String,
    pub bucket_name: Option<String>,
    pub deleted_at: Option<String>,
    pub username: Option<String>,
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

    /// Restore a soft-deleted file and optionally move it to a different folder.
    pub async fn restore_to_folder(pool: &PgPool, id: Uuid, folder_id: Option<Uuid>) -> AppResult<bool> {
        let affected = sqlx::query(
            "UPDATE user_files SET deleted_at = NULL, folder_id = $1 WHERE id = $2 AND deleted_at IS NOT NULL",
        )
        .bind(folder_id.map(|f| f.to_string()))
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to restore user_file to folder: {e}")))?
        .rows_affected();

        Ok(affected > 0)
    }

    /// List non-deleted files in a folder for a user. Returns (UserFile, blake3_hash, size).
    pub async fn list_by_folder(
        pool: &PgPool,
        user_id: Uuid,
        folder_id: Uuid,
    ) -> AppResult<Vec<(UserFile, String, i64)>> {
        let rows = sqlx::query_as::<_, UserFileWithMetaRow>(
            r#"SELECT uf.*, f.blake3_hash, f.size, f.ref_count
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.user_id = $1 AND uf.folder_id = $2 AND uf.deleted_at IS NULL
               ORDER BY uf.original_name ASC"#,
        )
        .bind(user_id.to_string())
        .bind(folder_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list files in folder: {e}")))?;

        Ok(rows.into_iter().map(|row| {
            let uf = UserFile::from(UserFileRow {
                id: row.id,
                user_id: row.user_id,
                file_id: row.file_id,
                original_name: row.original_name,
                mime_type: row.mime_type,
                created_at: row.created_at,
                bucket_name: row.bucket_name,
                folder_id: row.folder_id,
                deleted_at: row.deleted_at,
                purged_at: row.purged_at,
            });
            (uf, row.blake3_hash, row.size)
        }).collect())
    }

    // ── Trash (soft-deleted files visible to the owning user) ──

    /// List soft-deleted files for a user (the user's trash bin).
    /// Joins with the files table to get size and hash.
    pub async fn list_trashed_by_user(
        pool: &PgPool,
        user_id: Uuid,
        offset: i64,
        limit: i64,
    ) -> AppResult<(Vec<(UserFile, String, i64)>, i64)> {
        let count_row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files WHERE user_id = $1 AND deleted_at IS NOT NULL AND purged_at IS NULL",
        )
        .bind(user_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count trashed files: {e}")))?;

        let rows = sqlx::query_as::<_, UserFileWithMetaRow>(
            r#"SELECT uf.*, f.blake3_hash, f.size, f.ref_count
               FROM user_files uf
               JOIN files f ON uf.file_id = f.id
               WHERE uf.user_id = $1 AND uf.deleted_at IS NOT NULL AND uf.purged_at IS NULL
               ORDER BY uf.deleted_at DESC
               LIMIT $2 OFFSET $3"#,
        )
        .bind(user_id.to_string())
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list trashed files: {e}")))?;

        let files: Vec<(UserFile, String, i64)> = rows.into_iter().map(|row| {
            let uf = UserFile::from(UserFileRow {
                id: row.id,
                user_id: row.user_id,
                file_id: row.file_id,
                original_name: row.original_name,
                mime_type: row.mime_type,
                created_at: row.created_at,
                bucket_name: row.bucket_name,
                folder_id: row.folder_id,
                deleted_at: row.deleted_at,
                purged_at: row.purged_at,
            });
            (uf, row.blake3_hash, row.size)
        }).collect();

        Ok((files, count_row.0))
    }

    /// Find a soft-deleted user_file by user_id and user_file id (for trash operations).
    pub async fn find_trashed_by_user_and_id(
        pool: &PgPool,
        user_id: Uuid,
        user_file_id: Uuid,
    ) -> AppResult<Option<UserFile>> {
        let row = sqlx::query_as::<_, UserFileRow>(
            "SELECT * FROM user_files WHERE user_id = $1 AND id = $2 AND deleted_at IS NOT NULL AND purged_at IS NULL",
        )
        .bind(user_id.to_string())
        .bind(user_file_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query trashed user_file: {e}")))?;

        Ok(row.map(UserFile::from))
    }

    /// Permanently delete a soft-deleted user_file row.
    /// Does NOT touch the physical file or storage objects — only removes the user's reference.
    pub async fn hard_delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query(
            "DELETE FROM user_files WHERE id = $1 AND deleted_at IS NOT NULL",
        )
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to hard-delete user_file: {e}")))?
        .rows_affected();

        Ok(affected > 0)
    }

    /// Turn a trashed user_file into a tombstone: the row survives with
    /// `purged_at` set so the physical file stays attributable in the admin
    /// orphans view, but the entry disappears from the trash and no longer
    /// occupies its (user, file, name) slot. The caller is responsible for
    /// decrementing the physical file's ref_count.
    pub async fn mark_purged(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE user_files SET purged_at = $1 WHERE id = $2 AND deleted_at IS NOT NULL AND purged_at IS NULL",
        )
        .bind(now)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to purge user_file: {e}")))?
        .rows_affected();

        Ok(affected > 0)
    }

    /// Mark all trashed files inside the given folders as purged (tombstone),
    /// returning the file ids whose ref_count must be decremented.
    pub async fn mark_purged_in_folders(
        pool: &PgPool,
        user_id: Uuid,
        folder_ids: &[String],
    ) -> AppResult<Vec<String>> {
        if folder_ids.is_empty() {
            return Ok(Vec::new());
        }
        let now = Utc::now().to_rfc3339();
        let placeholders: Vec<String> = folder_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 3))
            .collect();
        let sql = format!(
            "UPDATE user_files SET purged_at = $1 WHERE user_id = $2 AND folder_id IN ({}) AND deleted_at IS NOT NULL AND purged_at IS NULL RETURNING file_id",
            placeholders.join(", ")
        );
        let mut query = sqlx::query_as::<_, (String,)>(&sql)
            .bind(now)
            .bind(user_id.to_string());
        for fid in folder_ids {
            query = query.bind(fid);
        }
        let rows = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to purge user_files: {e}")))?;

        Ok(rows.into_iter().map(|(fid,)| fid).collect())
    }

    /// List all user_files for a specific user, with pagination, search, bucket filter, and folder filter.
    /// Joins with the files table to get size, hash, and ref_count.
    ///
    /// Bot path rules are enforced via a derived `file_path` column (folder
    /// path + original name, computed by a recursive CTE over `user_folders`)
    /// so listing/counting stay accurate under pagination.
    pub async fn list_by_user(
        pool: &PgPool,
        user_id: Uuid,
        offset: i64,
        limit: i64,
        search: Option<&str>,
        bucket: Option<&str>,
        folder_id: Option<Uuid>,
        bot: Option<&crate::models::Bot>,
    ) -> AppResult<Vec<(UserFile, String, i64, i32)>> {
        // Returns (user_file, blake3_hash, size, ref_count)
        let user_id_str = user_id.to_string();
        let folder_str = folder_id.map(|f| f.to_string());

        let mut where_clauses: Vec<String> = Vec::new();
        let mut bind_values: Vec<String> = vec![user_id_str.clone()];
        let mut param_idx = 2;

        if let Some(b) = bucket {
            where_clauses.push(format!("bucket_name = ${param_idx}"));
            bind_values.push(b.to_string());
            param_idx += 1;
        }

        match folder_str.as_deref() {
            Some(fid) if !fid.is_empty() => {
                // Specific folder: show files in that folder
                where_clauses.push(format!("folder_id = ${param_idx}"));
                bind_values.push(fid.to_string());
                param_idx += 1;
            }
            _ => {
                // Root (no folder_id provided): only show root-level files
                where_clauses.push("folder_id IS NULL".to_string());
            }
        }

        // Narrow the query to the bot's path rules (see `bot_path_filter`).
        // Private files without a bucket always pass, mirroring the old
        // bucket allow-list behaviour.
        if let Some(bot) = bot {
            if let Some(cond) = bot_path_filter(bot, &mut param_idx, &mut bind_values) {
                where_clauses.push(cond);
            }
        }

        if let Some(s) = search {
            let pattern = format!("%{s}%");
            where_clauses.push(format!("(original_name ILIKE ${param_idx} OR blake3_hash ILIKE ${param_idx})"));
            bind_values.push(pattern);
            param_idx += 1;
        }

        let where_clause = if where_clauses.is_empty() {
            "TRUE".to_string()
        } else {
            where_clauses.join(" AND ")
        };

        let sql = format!(
            r#"WITH RECURSIVE folder_paths AS (
                   SELECT id, user_id, bucket_name, parent_id, name, name AS rel
                   FROM user_folders
                   WHERE parent_id IS NULL AND user_id = $1
                   UNION ALL
                   SELECT f.id, f.user_id, f.bucket_name, f.parent_id, f.name,
                          fp.rel || '/' || f.name
                   FROM user_folders f
                   JOIN folder_paths fp ON f.parent_id = fp.id
               ),
               file_rows AS (
                   SELECT uf.*, f.blake3_hash, f.size, f.ref_count,
                          CASE WHEN fp.rel IS NULL THEN '/' || uf.original_name
                               ELSE '/' || fp.rel || '/' || uf.original_name END AS file_path
                   FROM user_files uf
                   JOIN files f ON uf.file_id = f.id
                   LEFT JOIN folder_paths fp ON fp.id = uf.folder_id
                   WHERE uf.user_id = $1 AND uf.deleted_at IS NULL
               )
               SELECT id, user_id, file_id, original_name, mime_type, created_at,
                      bucket_name, folder_id, deleted_at, purged_at, blake3_hash, size, ref_count
               FROM file_rows
               WHERE {where_clause}
               ORDER BY created_at DESC
               LIMIT ${param_idx} OFFSET ${param_idx_plus_one}"#,
            where_clause = where_clause,
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
        bot: Option<&crate::models::Bot>,
    ) -> AppResult<i64> {
        let user_id_str = user_id.to_string();
        let folder_str = folder_id.map(|f| f.to_string());

        let mut where_clauses: Vec<String> = Vec::new();
        let mut bind_values: Vec<String> = vec![user_id_str.clone()];
        let mut param_idx = 2;

        if let Some(b) = bucket {
            where_clauses.push(format!("bucket_name = ${param_idx}"));
            bind_values.push(b.to_string());
            param_idx += 1;
        }

        match folder_str.as_deref() {
            Some(fid) if !fid.is_empty() => {
                where_clauses.push(format!("folder_id = ${param_idx}"));
                bind_values.push(fid.to_string());
                param_idx += 1;
            }
            _ => {
                where_clauses.push("folder_id IS NULL".to_string());
            }
        }

        if let Some(bot) = bot {
            if let Some(cond) = bot_path_filter(bot, &mut param_idx, &mut bind_values) {
                where_clauses.push(cond);
            }
        }

        if let Some(s) = search {
            let pattern = format!("%{s}%");
            where_clauses.push(format!("(original_name ILIKE ${param_idx} OR blake3_hash ILIKE ${param_idx})"));
            bind_values.push(pattern);
        }

        let where_clause = if where_clauses.is_empty() {
            "TRUE".to_string()
        } else {
            where_clauses.join(" AND ")
        };

        let sql = format!(
            r#"WITH RECURSIVE folder_paths AS (
                   SELECT id, user_id, bucket_name, parent_id, name, name AS rel
                   FROM user_folders
                   WHERE parent_id IS NULL AND user_id = $1
                   UNION ALL
                   SELECT f.id, f.user_id, f.bucket_name, f.parent_id, f.name,
                          fp.rel || '/' || f.name
                   FROM user_folders f
                   JOIN folder_paths fp ON f.parent_id = fp.id
               ),
               file_rows AS (
                   SELECT uf.*, f.blake3_hash, f.size, f.ref_count,
                          CASE WHEN fp.rel IS NULL THEN '/' || uf.original_name
                               ELSE '/' || fp.rel || '/' || uf.original_name END AS file_path
                   FROM user_files uf
                   JOIN files f ON uf.file_id = f.id
                   LEFT JOIN folder_paths fp ON fp.id = uf.folder_id
                   WHERE uf.user_id = $1 AND uf.deleted_at IS NULL
               )
               SELECT COUNT(*)
               FROM file_rows
               WHERE {where_clause}"#,
            where_clause = where_clause,
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
               WHERE uf.deleted_at IS NOT NULL AND uf.purged_at IS NULL
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
    /// files with NO active user_files reference. This covers both files whose
    /// every reference is soft-deleted (in some user's trash) and files whose
    /// references were permanently deleted from the trash.
    /// These files waste disk space but nobody can use them.
    /// Returns (count, total_size).
    pub async fn orphaned_physical_files_global(
        pool: &PgPool,
    ) -> AppResult<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            r#"SELECT COUNT(*), COALESCE(SUM(f.size), 0)::BIGINT
               FROM files f
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf WHERE uf.file_id = f.id AND uf.deleted_at IS NULL
               )"#,
        )
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count orphaned physical files: {e}")))?;

        Ok(row)
    }

    /// Per-bucket orphaned physical files stats:
    /// physical files whose EVERY reference is soft-deleted, attributed to the
    /// bucket of their most recent reference. Files with no references left at
    /// all have no bucket to attribute them to and are counted only in the
    /// global stats.
    /// Returns Vec<(bucket_name, orphaned_count, orphaned_size)>.
    pub async fn orphaned_physical_files_per_bucket(
        pool: &PgPool,
    ) -> AppResult<Vec<(String, i64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            r#"SELECT lr.bucket_name, COUNT(*) AS cnt, COALESCE(SUM(f.size), 0)::BIGINT AS total
               FROM files f
               JOIN LATERAL (
                   SELECT uf.bucket_name FROM user_files uf
                   WHERE uf.file_id = f.id
                   ORDER BY uf.deleted_at DESC NULLS LAST, uf.created_at DESC
                   LIMIT 1
               ) lr ON true
               WHERE lr.bucket_name IS NOT NULL
                 AND NOT EXISTS (
                     SELECT 1 FROM user_files uf2 WHERE uf2.file_id = f.id AND uf2.deleted_at IS NULL
                 )
               GROUP BY lr.bucket_name"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to get orphaned stats per bucket: {e}")))?;

        Ok(rows)
    }

    /// Total count and combined size of orphaned physical files (admin detail view),
    /// optionally filtered by the last reference's bucket and owner user id.
    /// Orphaned = NO active reference (all references soft-deleted or permanently deleted).
    pub async fn orphaned_files_total(
        pool: &PgPool,
        bucket: Option<&str>,
        owner_id: Option<&str>,
    ) -> AppResult<(i64, i64)> {
        let row: (i64, i64) = sqlx::query_as(
            r#"SELECT COUNT(*), COALESCE(SUM(f.size), 0)::BIGINT
               FROM files f
               LEFT JOIN LATERAL (
                   SELECT uf.original_name, uf.bucket_name, uf.user_id
                   FROM user_files uf
                   WHERE uf.file_id = f.id
                   ORDER BY uf.deleted_at DESC NULLS LAST, uf.created_at DESC
                   LIMIT 1
               ) lr ON true
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf2 WHERE uf2.file_id = f.id AND uf2.deleted_at IS NULL
               )
               AND ($1 IS NULL OR lr.bucket_name = $1)
               AND ($2 IS NULL OR lr.user_id = $2)"#,
        )
        .bind(bucket)
        .bind(owner_id)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count orphaned files: {e}")))?;

        Ok(row)
    }

    /// Filter facets for the orphaned-files admin page: distinct buckets and
    /// owners (attributed via each file's most recent reference).
    /// Returns (buckets, owners as (user_id, username)).
    pub async fn orphaned_file_facets(
        pool: &PgPool,
    ) -> AppResult<(Vec<String>, Vec<(String, String)>)> {
        let rows: Vec<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
            r#"SELECT DISTINCT lr.bucket_name, lr.user_id, u.username
               FROM files f
               LEFT JOIN LATERAL (
                   SELECT uf.bucket_name, uf.user_id
                   FROM user_files uf
                   WHERE uf.file_id = f.id
                   ORDER BY uf.deleted_at DESC NULLS LAST, uf.created_at DESC
                   LIMIT 1
               ) lr ON true
               LEFT JOIN users u ON u.id = lr.user_id
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf2 WHERE uf2.file_id = f.id AND uf2.deleted_at IS NULL
               )"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list orphaned file facets: {e}")))?;

        let mut buckets: Vec<String> = rows.iter().filter_map(|(b, _, _)| b.clone()).collect();
        buckets.sort();
        buckets.dedup();

        let mut owners: Vec<(String, String)> = rows
            .iter()
            .filter_map(|(_, uid, uname)| {
                let id = uid.clone()?;
                Some((id.clone(), uname.clone().unwrap_or(id)))
            })
            .collect();
        owners.sort_by(|a, b| a.1.cmp(&b.1));
        owners.dedup_by(|a, b| a.0 == b.0);

        Ok((buckets, owners))
    }

    /// All physical file ids that are orphaned: NO active user_files reference.
    /// Used by the admin "delete all" action.
    pub async fn orphaned_file_ids(pool: &PgPool) -> AppResult<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"SELECT f.id
               FROM files f
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf WHERE uf.file_id = f.id AND uf.deleted_at IS NULL
               )"#,
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list orphaned file ids: {e}")))?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Whether a physical file is orphaned: it has NO active reference
    /// (every reference soft-deleted or permanently deleted).
    /// Display name for an orphaned physical file: its most recent
    /// reference's original_name, or the physical record's own name when no
    /// references remain. Used by the admin orphan download endpoint.
    pub async fn orphan_display_name(pool: &PgPool, file_id: Uuid) -> AppResult<String> {
        let row: Option<(String,)> = sqlx::query_as(
            r#"SELECT COALESCE(lr.original_name, f.original_name)
               FROM files f
               LEFT JOIN LATERAL (
                   SELECT uf.original_name FROM user_files uf
                   WHERE uf.file_id = f.id
                   ORDER BY uf.deleted_at DESC NULLS LAST, uf.created_at DESC
                   LIMIT 1
               ) lr ON true
               WHERE f.id = $1"#,
        )
        .bind(file_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to resolve orphan name: {e}")))?;
        Ok(row.map(|(n,)| n).unwrap_or_else(|| "download".into()))
    }

    pub async fn is_orphaned_file(pool: &PgPool, file_id: Uuid) -> AppResult<bool> {
        let row: (bool,) = sqlx::query_as(
            r#"SELECT NOT EXISTS (
                   SELECT 1 FROM user_files uf WHERE uf.file_id = $1 AND uf.deleted_at IS NULL
               )"#,
        )
        .bind(file_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to check orphaned status: {e}")))?;

        Ok(row.0)
    }

    /// Hard-delete every user_files row referencing a file (used when purging
    /// an orphaned physical file; all such references are already soft-deleted).
    pub async fn delete_by_file(pool: &PgPool, file_id: Uuid) -> AppResult<()> {
        sqlx::query("DELETE FROM user_files WHERE file_id = $1")
            .bind(file_id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete user_files rows: {e}")))?;

        Ok(())
    }

    /// One page of orphaned physical files for the admin UI. For each file the
    /// most recent reference is used to show name, bucket, owner and when it
    /// became unreachable; files with no references left fall back to the
    /// physical record's own name with null bucket/owner/date.
    pub async fn orphaned_files_page(
        pool: &PgPool,
        limit: i64,
        offset: i64,
        bucket: Option<&str>,
        owner_id: Option<&str>,
    ) -> AppResult<Vec<OrphanedFileRow>> {
        let rows = sqlx::query_as::<_, OrphanedFileRow>(
            r#"SELECT f.id AS file_id, f.blake3_hash,
                      COALESCE(lr.original_name, f.original_name) AS original_name,
                      f.size, f.created_at,
                      lr.bucket_name AS bucket_name, lr.deleted_at, u.username
               FROM files f
               LEFT JOIN LATERAL (
                   SELECT uf.original_name, uf.bucket_name, uf.deleted_at, uf.user_id
                   FROM user_files uf
                   WHERE uf.file_id = f.id
                   ORDER BY uf.deleted_at DESC NULLS LAST, uf.created_at DESC
                   LIMIT 1
               ) lr ON true
               LEFT JOIN users u ON u.id = lr.user_id
               WHERE NOT EXISTS (
                   SELECT 1 FROM user_files uf2 WHERE uf2.file_id = f.id AND uf2.deleted_at IS NULL
               )
               AND ($1 IS NULL OR lr.bucket_name = $1)
               AND ($2 IS NULL OR lr.user_id = $2)
               ORDER BY COALESCE(lr.deleted_at, f.created_at) DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(bucket)
        .bind(owner_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list orphaned files: {e}")))?;

        Ok(rows)
    }
}
