use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::{
    MirrorJobRow, MirrorSourceFile, MirrorSourceFolder, PrunedMirrorFile, TargetFolderRow,
    TargetUserFileRow,
};
use crate::error::{AppError, AppResult};
use crate::models::MirrorJob;

pub struct NewMirrorJob {
    pub id: Uuid,
    pub name: String,
    pub job_type: String,
    pub source_bucket: String,
    pub target_bucket: String,
    pub schedule_time: String,
    pub enabled: bool,
    pub prune: bool,
    pub next_run_at: Option<String>,
}

pub struct MirrorJobChanges {
    pub name: String,
    pub source_bucket: String,
    pub target_bucket: String,
    pub schedule_time: String,
    pub enabled: bool,
    pub prune: bool,
    pub next_run_at: Option<String>,
}

/// Fields for a mirror-created target link.
pub struct NewTargetUserFile<'a> {
    pub id: Uuid,
    pub user_id: &'a str,
    pub file_id: &'a str,
    pub original_name: &'a str,
    pub mime_type: Option<&'a str>,
    pub bucket_name: &'a str,
    pub folder_id: Option<&'a str>,
    pub job_id: Uuid,
}

pub struct MirrorJobRepository;

impl MirrorJobRepository {
    pub async fn create(pool: &PgPool, job: NewMirrorJob) -> AppResult<MirrorJob> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query_as::<_, MirrorJobRow>(
            r#"INSERT INTO mirror_jobs
                   (id, name, job_type, source_bucket, target_bucket, schedule_time,
                    enabled, prune, next_run_at, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
               RETURNING *"#,
        )
        .bind(job.id.to_string())
        .bind(job.name)
        .bind(job.job_type)
        .bind(job.source_bucket)
        .bind(job.target_bucket)
        .bind(job.schedule_time)
        .bind(job.enabled)
        .bind(job.prune)
        .bind(job.next_run_at)
        .bind(&now)
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create mirror job: {e}")))?;

        Ok(MirrorJob::from(row))
    }

    pub async fn list(pool: &PgPool) -> AppResult<Vec<MirrorJob>> {
        let rows = sqlx::query_as::<_, MirrorJobRow>(
            "SELECT * FROM mirror_jobs ORDER BY created_at ASC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list mirror jobs: {e}")))?;

        Ok(rows.into_iter().map(MirrorJob::from).collect())
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<MirrorJob>> {
        let row = sqlx::query_as::<_, MirrorJobRow>(
            "SELECT * FROM mirror_jobs WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find mirror job: {e}")))?;

        Ok(row.map(MirrorJob::from))
    }

    pub async fn update(
        pool: &PgPool,
        id: Uuid,
        changes: MirrorJobChanges,
    ) -> AppResult<Option<MirrorJob>> {
        let now = Utc::now().to_rfc3339();
        let row = sqlx::query_as::<_, MirrorJobRow>(
            r#"UPDATE mirror_jobs
               SET name = $2, source_bucket = $3, target_bucket = $4,
                   schedule_time = $5, enabled = $6, prune = $7,
                   next_run_at = $8, updated_at = $9
               WHERE id = $1
               RETURNING *"#,
        )
        .bind(id.to_string())
        .bind(changes.name)
        .bind(changes.source_bucket)
        .bind(changes.target_bucket)
        .bind(changes.schedule_time)
        .bind(changes.enabled)
        .bind(changes.prune)
        .bind(changes.next_run_at)
        .bind(&now)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update mirror job: {e}")))?;

        Ok(row.map(MirrorJob::from))
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM mirror_jobs WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete mirror job: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Jobs whose scheduled time has arrived and that are not already running.
    /// Disabled jobs are normally skipped because their `next_run_at` is NULL;
    /// "Run now" re-arms a disabled job by setting `next_run_at` to now.
    pub async fn list_due(pool: &PgPool, now: &str) -> AppResult<Vec<MirrorJob>> {
        let rows = sqlx::query_as::<_, MirrorJobRow>(
            r#"SELECT * FROM mirror_jobs
               WHERE next_run_at IS NOT NULL AND next_run_at <= $1 AND NOT running
               ORDER BY next_run_at ASC"#,
        )
        .bind(now)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list due mirror jobs: {e}")))?;

        Ok(rows.into_iter().map(MirrorJob::from).collect())
    }

    /// Atomically claim a due job so two scheduler ticks cannot run it twice.
    pub async fn claim(pool: &PgPool, id: Uuid, now: &str) -> AppResult<Option<MirrorJob>> {
        let row = sqlx::query_as::<_, MirrorJobRow>(
            r#"UPDATE mirror_jobs
               SET running = TRUE, running_since = $2, last_run_at = $2, updated_at = $2
               WHERE id = $1 AND NOT running
               RETURNING *"#,
        )
        .bind(id.to_string())
        .bind(now)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to claim mirror job: {e}")))?;

        Ok(row.map(MirrorJob::from))
    }

    pub async fn finish(
        pool: &PgPool,
        id: Uuid,
        status: &str,
        error: Option<&str>,
        files_copied: i32,
        files_pruned: i32,
        next_run_at: Option<&str>,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE mirror_jobs
               SET running = FALSE, running_since = NULL, last_finished_at = $2,
                   last_status = $3, last_error = $4, last_files_copied = $5,
                   last_files_pruned = $6, next_run_at = $7, updated_at = $2
               WHERE id = $1"#,
        )
        .bind(id.to_string())
        .bind(&now)
        .bind(status)
        .bind(error)
        .bind(files_copied)
        .bind(files_pruned)
        .bind(next_run_at)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to finish mirror job: {e}")))?;

        Ok(())
    }

    /// Clear the running marker for jobs interrupted by a process restart.
    pub async fn reset_running(pool: &PgPool) -> AppResult<u64> {
        let affected = sqlx::query(
            "UPDATE mirror_jobs SET running = FALSE, running_since = NULL WHERE running = TRUE",
        )
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to reset running mirror jobs: {e}")))?
        .rows_affected();
        Ok(affected)
    }

    /// Clear the running marker for jobs that have been stuck since before
    /// `cutoff` (e.g. the process was killed hard and the startup reset was
    /// missed).
    pub async fn reset_stale_running(pool: &PgPool, cutoff: &str) -> AppResult<u64> {
        let affected = sqlx::query(
            r#"UPDATE mirror_jobs
               SET running = FALSE, running_since = NULL
               WHERE running AND running_since IS NOT NULL AND running_since < $1"#,
        )
        .bind(cutoff)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to reset stale mirror jobs: {e}")))?
        .rows_affected();
        Ok(affected)
    }

    pub async fn set_next_run_at(
        pool: &PgPool,
        id: Uuid,
        next_run_at: Option<&str>,
    ) -> AppResult<()> {
        sqlx::query(
            "UPDATE mirror_jobs SET next_run_at = $2, updated_at = $3 WHERE id = $1",
        )
        .bind(id.to_string())
        .bind(next_run_at)
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to schedule mirror job: {e}")))?;
        Ok(())
    }

    /// Enabled jobs that have never been scheduled (e.g. created before the
    /// scheduler existed). Returns `(id, schedule_time)`.
    pub async fn list_enabled_without_next_run(
        pool: &PgPool,
    ) -> AppResult<Vec<(String, String)>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT id, schedule_time FROM mirror_jobs WHERE enabled AND next_run_at IS NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| {
            AppError::Internal(format!("failed to list unscheduled mirror jobs: {e}"))
        })?;
        Ok(rows)
    }

    // ── Mirror source/target helpers ─────────────────────────────

    pub async fn list_source_files(
        pool: &PgPool,
        bucket_name: &str,
    ) -> AppResult<Vec<MirrorSourceFile>> {
        let rows = sqlx::query_as::<_, MirrorSourceFile>(
            r#"SELECT uf.user_id, uf.file_id, uf.original_name, uf.mime_type,
                      uf.folder_id, f.blake3_hash, f.size
               FROM user_files uf
               JOIN files f ON f.id = uf.file_id
               WHERE uf.bucket_name = $1
                 AND uf.deleted_at IS NULL
                 AND uf.purged_at IS NULL
               ORDER BY uf.user_id, uf.created_at ASC"#,
        )
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list mirror source files: {e}")))?;
        Ok(rows)
    }

    pub async fn list_source_folders(
        pool: &PgPool,
        bucket_name: &str,
    ) -> AppResult<Vec<MirrorSourceFolder>> {
        let rows = sqlx::query_as::<_, MirrorSourceFolder>(
            r#"SELECT id, user_id, parent_id, name
               FROM user_folders
               WHERE bucket_name = $1 AND deleted_at IS NULL
               ORDER BY created_at ASC"#,
        )
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list mirror source folders: {e}")))?;
        Ok(rows)
    }

    pub async fn find_target_folder(
        pool: &PgPool,
        user_id: &str,
        bucket_name: &str,
        parent_id: Option<&str>,
        name: &str,
    ) -> AppResult<Option<TargetFolderRow>> {
        let row = sqlx::query_as::<_, TargetFolderRow>(
            r#"SELECT id, deleted_at, mirror_job_id
               FROM user_folders
               WHERE user_id = $1 AND bucket_name = $2
                 AND parent_id IS NOT DISTINCT FROM $3
                 AND name = $4
               ORDER BY (deleted_at IS NULL) DESC, created_at ASC
               LIMIT 1"#,
        )
        .bind(user_id)
        .bind(bucket_name)
        .bind(parent_id)
        .bind(name)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find target folder: {e}")))?;
        Ok(row)
    }

    pub async fn create_target_folder(
        pool: &PgPool,
        id: Uuid,
        user_id: &str,
        bucket_name: &str,
        parent_id: Option<&str>,
        name: &str,
        job_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query(
            r#"INSERT INTO user_folders
                   (id, user_id, bucket_name, parent_id, name, created_at, mirror_job_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(id.to_string())
        .bind(user_id)
        .bind(bucket_name)
        .bind(parent_id)
        .bind(name)
        .bind(Utc::now().to_rfc3339())
        .bind(job_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create target folder: {e}")))?;
        Ok(())
    }

    pub async fn refresh_target_folder(
        pool: &PgPool,
        id: &str,
        parent_id: Option<&str>,
        job_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query(
            r#"UPDATE user_folders
               SET deleted_at = NULL,
                   parent_id = $2,
                   mirror_job_id = COALESCE(mirror_job_id, $3)
               WHERE id = $1"#,
        )
        .bind(id)
        .bind(parent_id)
        .bind(job_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to refresh target folder: {e}")))?;
        Ok(())
    }

    pub async fn find_target_user_file(
        pool: &PgPool,
        user_id: &str,
        file_id: &str,
        original_name: &str,
        bucket_name: &str,
    ) -> AppResult<Option<TargetUserFileRow>> {
        let row = sqlx::query_as::<_, TargetUserFileRow>(
            r#"SELECT id, folder_id, deleted_at, purged_at, mirror_job_id
               FROM user_files
               WHERE user_id = $1 AND file_id = $2 AND original_name = $3
                 AND bucket_name = $4
               ORDER BY (deleted_at IS NULL) DESC, created_at ASC
               LIMIT 1"#,
        )
        .bind(user_id)
        .bind(file_id)
        .bind(original_name)
        .bind(bucket_name)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find target user file: {e}")))?;
        Ok(row)
    }

    pub async fn insert_target_user_file(
        pool: &PgPool,
        file: NewTargetUserFile<'_>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"INSERT INTO user_files
                   (id, user_id, file_id, original_name, mime_type, created_at,
                    bucket_name, folder_id, mirror_job_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        )
        .bind(file.id.to_string())
        .bind(file.user_id)
        .bind(file.file_id)
        .bind(file.original_name)
        .bind(file.mime_type)
        .bind(Utc::now().to_rfc3339())
        .bind(file.bucket_name)
        .bind(file.folder_id)
        .bind(file.job_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create target user file: {e}")))?;
        Ok(())
    }

    pub async fn refresh_target_user_file(
        pool: &PgPool,
        id: &str,
        folder_id: Option<&str>,
        job_id: Uuid,
    ) -> AppResult<()> {
        sqlx::query(
            r#"UPDATE user_files
               SET deleted_at = NULL,
                   purged_at = NULL,
                   folder_id = $2,
                   mirror_job_id = COALESCE(mirror_job_id, $3)
               WHERE id = $1"#,
        )
        .bind(id)
        .bind(folder_id)
        .bind(job_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to refresh target user file: {e}")))?;
        Ok(())
    }

    /// Delete target links owned by this job whose matching source link no
    /// longer exists. Returns the physical file ids that need a ref_count
    /// release (`purged_at IS NULL` rows still hold a reference).
    pub async fn prune_target_user_files(
        pool: &PgPool,
        job_id: Uuid,
        target_bucket: &str,
        source_bucket: &str,
    ) -> AppResult<Vec<PrunedMirrorFile>> {
        let rows = sqlx::query_as::<_, PrunedMirrorFile>(
            r#"DELETE FROM user_files tf
               WHERE tf.mirror_job_id = $1
                 AND tf.bucket_name = $2
                 AND NOT EXISTS (
                     SELECT 1 FROM user_files sf
                     WHERE sf.bucket_name = $3
                       AND sf.deleted_at IS NULL
                       AND sf.purged_at IS NULL
                       AND sf.user_id = tf.user_id
                       AND sf.file_id = tf.file_id
                       AND sf.original_name = tf.original_name
                 )
               RETURNING tf.file_id, tf.purged_at"#,
        )
        .bind(job_id.to_string())
        .bind(target_bucket)
        .bind(source_bucket)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to prune target user files: {e}")))?;
        Ok(rows)
    }

    /// Delete empty folders created by this job that were not produced by the
    /// latest mirror pass. Repeated until stable so a whole orphaned subtree
    /// disappears from the leaves upward.
    pub async fn prune_empty_mirrored_folders(
        pool: &PgPool,
        job_id: Uuid,
        seen_folder_ids: &[String],
    ) -> AppResult<u64> {
        let mut total = 0u64;
        loop {
            let deleted = sqlx::query_scalar::<_, String>(
                r#"DELETE FROM user_folders f
                   WHERE f.mirror_job_id = $1
                     AND NOT (f.id = ANY($2::text[]))
                     AND NOT EXISTS (
                         SELECT 1 FROM user_files uf WHERE uf.folder_id = f.id
                     )
                     AND NOT EXISTS (
                         SELECT 1 FROM user_folders c WHERE c.parent_id = f.id
                     )
                   RETURNING f.id"#,
            )
            .bind(job_id.to_string())
            .bind(seen_folder_ids)
            .fetch_all(pool)
            .await
            .map_err(|e| {
                AppError::Internal(format!("failed to prune empty mirror folders: {e}"))
            })?;

            if deleted.is_empty() {
                break;
            }
            total += deleted.len() as u64;
        }
        Ok(total)
    }
}
