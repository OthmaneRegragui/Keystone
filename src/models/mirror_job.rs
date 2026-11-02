use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// An admin-managed scheduled mirror job. Each job copies a user's active
/// files from one bucket into another, preserving their folder layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorJob {
    pub id: Uuid,
    pub name: String,
    pub job_type: String,
    pub source_bucket: String,
    pub target_bucket: String,
    /// Daily UTC start time in `HH:MM` form.
    pub schedule_time: String,
    pub enabled: bool,
    pub prune: bool,
    pub running: bool,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_finished_at: Option<DateTime<Utc>>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_files_copied: i32,
    pub last_files_pruned: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
