use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

use crate::models::MirrorJob;

#[derive(Debug, Clone, FromRow)]
pub struct MirrorJobRow {
    pub id: String,
    pub name: String,
    pub job_type: String,
    pub source_bucket: String,
    pub target_bucket: String,
    pub schedule_time: String,
    pub enabled: bool,
    pub prune: bool,
    pub running: bool,
    pub last_run_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub next_run_at: Option<String>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_files_copied: i32,
    pub last_files_pruned: i32,
    pub created_at: String,
    pub updated_at: String,
}

fn parse_dt(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("invalid datetime in database")
        .with_timezone(&Utc)
}

fn parse_optional_dt(value: Option<String>) -> Option<DateTime<Utc>> {
    value.as_deref().map(parse_dt)
}

impl From<MirrorJobRow> for MirrorJob {
    fn from(row: MirrorJobRow) -> Self {
        Self {
            id: Uuid::parse_str(&row.id).expect("invalid uuid in database"),
            name: row.name,
            job_type: row.job_type,
            source_bucket: row.source_bucket,
            target_bucket: row.target_bucket,
            schedule_time: row.schedule_time,
            enabled: row.enabled,
            prune: row.prune,
            running: row.running,
            last_run_at: parse_optional_dt(row.last_run_at),
            last_finished_at: parse_optional_dt(row.last_finished_at),
            next_run_at: parse_optional_dt(row.next_run_at),
            last_status: row.last_status,
            last_error: row.last_error,
            last_files_copied: row.last_files_copied,
            last_files_pruned: row.last_files_pruned,
            created_at: parse_dt(&row.created_at),
            updated_at: parse_dt(&row.updated_at),
        }
    }
}

/// A source link in the bucket being mirrored, joined with its physical file.
#[derive(Debug, Clone, FromRow)]
pub struct MirrorSourceFile {
    pub user_id: String,
    pub file_id: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub folder_id: Option<String>,
    pub blake3_hash: String,
    pub size: i64,
}

/// A source folder in the bucket being mirrored.
#[derive(Debug, Clone, FromRow)]
pub struct MirrorSourceFolder {
    pub id: String,
    pub user_id: String,
    pub parent_id: Option<String>,
    pub name: String,
}

/// An existing target folder, including soft-deleted ones.
#[derive(Debug, Clone, FromRow)]
pub struct TargetFolderRow {
    pub id: String,
    pub deleted_at: Option<String>,
    pub mirror_job_id: Option<String>,
}

/// An existing target link, including soft-deleted/purged ones.
#[derive(Debug, Clone, FromRow)]
pub struct TargetUserFileRow {
    pub id: String,
    pub folder_id: Option<String>,
    pub deleted_at: Option<String>,
    pub purged_at: Option<String>,
    pub mirror_job_id: Option<String>,
}

/// A pruned target link: the physical file whose ref_count may need releasing.
#[derive(Debug, Clone, FromRow)]
pub struct PrunedMirrorFile {
    pub file_id: String,
    pub purged_at: Option<String>,
}
