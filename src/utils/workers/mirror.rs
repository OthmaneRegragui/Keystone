use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::api::controllers::admin::imports::ensure_blob_in_backend;
use crate::db::repos::mirror_jobs::NewTargetUserFile;
use crate::db::repos::{
    BucketRepository, FileRepository, MirrorJobRepository, StorageObjectRepository,
};
use crate::db::rows::{MirrorSourceFile, MirrorSourceFolder};
use crate::error::{AppError, AppResult};
use crate::models::MirrorJob;
use crate::utils::schedule::next_run_after;
use crate::AppState;

const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// A job running longer than this is considered abandoned (e.g. the process
/// was killed) and is released so the next scheduled run can proceed.
const STALE_RUNNING_AFTER_HOURS: i64 = 12;

struct MirrorStats {
    files_copied: i32,
    files_pruned: i32,
}

/// Background scheduler for admin-managed mirror jobs. One job at a time is
/// executed sequentially to keep disk/network load predictable.
pub async fn run_mirror_scheduler(state: Arc<AppState>, mut shutdown: broadcast::Receiver<()>) {
    info!("mirror scheduler started (poll interval {:?})", POLL_INTERVAL);

    let pool = state.db.pool();
    match MirrorJobRepository::reset_running(pool).await {
        Ok(n) if n > 0 => info!("released {n} mirror job(s) left running by a previous process"),
        Ok(_) => {}
        Err(e) => error!("failed to reset running mirror jobs at startup: {e}"),
    }

    // Jobs created before the scheduler existed may lack a next run time.
    match MirrorJobRepository::list_enabled_without_next_run(pool).await {
        Ok(jobs) => {
            for (id, schedule) in jobs {
                let Ok(id) = Uuid::parse_str(&id) else { continue };
                match next_run_after(&schedule, Utc::now()) {
                    Ok(next) => {
                        let _ = MirrorJobRepository::set_next_run_at(
                            pool,
                            id,
                            Some(&next.to_rfc3339()),
                        )
                        .await;
                    }
                    Err(e) => warn!("mirror job {id} has an invalid schedule '{schedule}': {e}"),
                }
            }
        }
        Err(e) => error!("failed to schedule existing mirror jobs: {e}"),
    }

    // Run a tick immediately so a job that became due while the server was
    // down does not wait for the first interval.
    if let Err(e) = tick(&state).await {
        error!("mirror scheduler tick failed: {e}");
    }

    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {
                if let Err(e) = tick(&state).await {
                    error!("mirror scheduler tick failed: {e}");
                }
            }
            _ = shutdown.recv() => {
                info!("mirror scheduler shutting down");
                break;
            }
        }
    }
}

async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let pool = state.db.pool();

    // Release jobs whose worker disappeared without finishing.
    let cutoff = (Utc::now() - ChronoDuration::hours(STALE_RUNNING_AFTER_HOURS)).to_rfc3339();
    if let Ok(n) = MirrorJobRepository::reset_stale_running(pool, &cutoff).await {
        if n > 0 {
            warn!("released {n} stale running mirror job(s)");
        }
    }

    let now = Utc::now().to_rfc3339();
    let due = MirrorJobRepository::list_due(pool, &now)
        .await
        .map_err(|e| format!("failed to list due mirror jobs: {e}"))?;

    for job in due {
        let claimed = MirrorJobRepository::claim(pool, job.id, &Utc::now().to_rfc3339())
            .await
            .map_err(|e| format!("failed to claim mirror job {}: {e}", job.id))?;
        if let Some(job) = claimed {
            execute_claimed_job(state, job).await;
        }
    }

    Ok(())
}

async fn execute_claimed_job(state: &Arc<AppState>, job: MirrorJob) {
    info!(
        "mirror job '{}' started ({} -> {})",
        job.name, job.source_bucket, job.target_bucket
    );

    let result = run_mirror(state, &job).await;
    let finished = Utc::now();
    let next_run_at = if job.enabled {
        next_run_after(&job.schedule_time, finished)
            .ok()
            .map(|d| d.to_rfc3339())
    } else {
        None
    };

    let pool = state.db.pool();
    match result {
        Ok(stats) => {
            let _ = MirrorJobRepository::finish(
                pool,
                job.id,
                "success",
                None,
                stats.files_copied,
                stats.files_pruned,
                next_run_at.as_deref(),
            )
            .await;
            info!(
                "mirror job '{}' finished: {} copied, {} pruned",
                job.name, stats.files_copied, stats.files_pruned
            );
        }
        Err(e) => {
            let message = e.to_string();
            let _ = MirrorJobRepository::finish(
                pool,
                job.id,
                "error",
                Some(&message),
                0,
                0,
                next_run_at.as_deref(),
            )
            .await;
            error!("mirror job '{}' failed: {}", job.name, message);
        }
    }
}

async fn run_mirror(state: &Arc<AppState>, job: &MirrorJob) -> AppResult<MirrorStats> {
    let pool = state.db.pool();

    // Buckets can be renamed/deleted between scheduling and execution.
    BucketRepository::find_by_name(pool, &job.source_bucket)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("source bucket '{}' not found", job.source_bucket))
        })?;
    BucketRepository::find_by_name(pool, &job.target_bucket)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("target bucket '{}' not found", job.target_bucket))
        })?;

    let source_folders = MirrorJobRepository::list_source_folders(pool, &job.source_bucket).await?;
    let source_files = MirrorJobRepository::list_source_files(pool, &job.source_bucket).await?;

    let mut folder_map: HashMap<String, String> = HashMap::new();
    let mut seen_target_folders: HashSet<String> = HashSet::new();

    let mut folders_by_user: HashMap<String, Vec<MirrorSourceFolder>> = HashMap::new();
    for folder in source_folders {
        folders_by_user
            .entry(folder.user_id.clone())
            .or_default()
            .push(folder);
    }

    for (user_id, folders) in folders_by_user {
        mirror_user_folders(
            state,
            job,
            &user_id,
            folders,
            &mut folder_map,
            &mut seen_target_folders,
        )
        .await?;
    }

    let mut stats = MirrorStats {
        files_copied: 0,
        files_pruned: 0,
    };

    for source_file in source_files {
        let folder_id = source_file
            .folder_id
            .as_ref()
            .and_then(|id| folder_map.get(id))
            .cloned();

        if source_file.folder_id.is_some() && folder_id.is_none() {
            warn!(
                "mirror job '{}': folder for '{}' was not mapped; placing it at the bucket root",
                job.name, source_file.original_name
            );
        }

        if mirror_user_file(state, job, &source_file, folder_id.as_deref()).await? {
            stats.files_copied += 1;
        }
    }

    if job.prune {
        let pruned = MirrorJobRepository::prune_target_user_files(
            pool,
            job.id,
            &job.target_bucket,
            &job.source_bucket,
        )
        .await?;

        for row in &pruned {
            // Purged tombstones already released their reference. Soft-deleted
            // rows still hold one, so deleting them must release it.
            if row.purged_at.is_none() {
                if let Ok(file_id) = Uuid::parse_str(&row.file_id) {
                    let _ = FileRepository::update_ref_count(pool, file_id, -1).await;
                }
            }
        }
        stats.files_pruned = pruned.len() as i32;

        let seen: Vec<String> = seen_target_folders.into_iter().collect();
        match MirrorJobRepository::prune_empty_mirrored_folders(pool, job.id, &seen).await {
            Ok(n) if n > 0 => {
                info!("mirror job '{}': removed {n} orphaned folder(s)", job.name);
            }
            Ok(_) => {}
            Err(e) => warn!("mirror job '{}': folder prune failed: {e}", job.name),
        }
    }

    Ok(stats)
}

/// Recreate the user's source folder tree in the target bucket, parent first.
async fn mirror_user_folders(
    state: &Arc<AppState>,
    job: &MirrorJob,
    user_id: &str,
    mut folders: Vec<MirrorSourceFolder>,
    folder_map: &mut HashMap<String, String>,
    seen_target_folders: &mut HashSet<String>,
) -> AppResult<()> {
    let parent_map: HashMap<String, Option<String>> = folders
        .iter()
        .map(|f| (f.id.clone(), f.parent_id.clone()))
        .collect();

    // Shallow folders must be resolved before their children so the parent
    // mapping exists when a child is created.
    folders.sort_by_key(|f| folder_depth(&f.id, &parent_map));

    for folder in folders {
        let parent_target = folder
            .parent_id
            .as_ref()
            .and_then(|id| folder_map.get(id))
            .cloned();

        if folder.parent_id.is_some() && parent_target.is_none() {
            warn!(
                "mirror job '{}': parent of folder '{}' was not mapped; placing it at the bucket root",
                job.name, folder.name
            );
        }

        let target_id = match MirrorJobRepository::find_target_folder(
            state.db.pool(),
            user_id,
            &job.target_bucket,
            parent_target.as_deref(),
            &folder.name,
        )
        .await?
        {
            Some(existing) => {
                MirrorJobRepository::refresh_target_folder(
                    state.db.pool(),
                    &existing.id,
                    parent_target.as_deref(),
                    job.id,
                )
                .await?;
                existing.id
            }
            None => {
                let id = Uuid::new_v4();
                MirrorJobRepository::create_target_folder(
                    state.db.pool(),
                    id,
                    user_id,
                    &job.target_bucket,
                    parent_target.as_deref(),
                    &folder.name,
                    job.id,
                )
                .await?;
                id.to_string()
            }
        };

        seen_target_folders.insert(target_id.clone());
        folder_map.insert(folder.id, target_id);
    }

    Ok(())
}

fn folder_depth(id: &str, parent_map: &HashMap<String, Option<String>>) -> usize {
    let mut depth = 0usize;
    let mut current = Some(id.to_string());
    let mut visited: HashSet<String> = HashSet::new();

    while let Some(folder_id) = current {
        if !visited.insert(folder_id.clone()) {
            break;
        }
        match parent_map.get(&folder_id) {
            Some(Some(parent)) => {
                depth += 1;
                current = Some(parent.clone());
            }
            _ => break,
        }
    }

    depth
}

/// Mirror one source link. Returns `true` when a new target link was created.
async fn mirror_user_file(
    state: &Arc<AppState>,
    job: &MirrorJob,
    source: &MirrorSourceFile,
    folder_id: Option<&str>,
) -> AppResult<bool> {
    let pool = state.db.pool();

    // Avoid reading the blob again when the target backend already holds it.
    let existing_file = FileRepository::find_by_hash(pool, &source.blake3_hash).await?;
    let target_has_copy = match &existing_file {
        Some(file) => StorageObjectRepository::find_by_file_id(pool, file.id)
            .await?
            .iter()
            .any(|obj| obj.backend == job.target_bucket),
        None => false,
    };

    let (file, created_file_row) = if target_has_copy {
        (existing_file.expect("checked above"), false)
    } else {
        let data =
            load_source_blob(state, &source.file_id, &source.blake3_hash, &job.source_bucket)
                .await?;
        ensure_blob_in_backend(
            state,
            &job.target_bucket,
            &source.blake3_hash,
            &data,
            source.mime_type.as_deref(),
        )
        .await?
    };

    let file_id = file.id.to_string();

    if let Some(existing) = MirrorJobRepository::find_target_user_file(
        pool,
        &source.user_id,
        &file_id,
        &source.original_name,
        &job.target_bucket,
    )
    .await?
    {
        let was_purged = existing.purged_at.is_some();
        let needs_refresh = existing.deleted_at.is_some()
            || existing.folder_id.as_deref() != folder_id
            || existing.mirror_job_id.is_none();

        if needs_refresh {
            MirrorJobRepository::refresh_target_user_file(pool, &existing.id, folder_id, job.id)
                .await?;
        }

        // A purged tombstone released its ref_count; resurrecting it must
        // take a reference again. A merely soft-deleted row never released it.
        if was_purged {
            FileRepository::update_ref_count(pool, file.id, 1).await?;
        }

        return Ok(false);
    }

    MirrorJobRepository::insert_target_user_file(
        pool,
        NewTargetUserFile {
            id: Uuid::new_v4(),
            user_id: &source.user_id,
            file_id: &file_id,
            original_name: &source.original_name,
            mime_type: source.mime_type.as_deref(),
            bucket_name: &job.target_bucket,
            folder_id,
            job_id: job.id,
        },
    )
    .await?;

    // A pre-existing physical file gains a reference; a freshly created
    // `files` row already carries ref_count = 1 for this link.
    if !created_file_row {
        FileRepository::update_ref_count(pool, file.id, 1).await?;
    }

    Ok(true)
}

/// Read a file's bytes from any backend that holds a copy, preferring the
/// source bucket's own backend.
async fn load_source_blob(
    state: &AppState,
    file_id: &str,
    hash: &str,
    source_bucket: &str,
) -> AppResult<bytes::Bytes> {
    let file_id = Uuid::parse_str(file_id)
        .map_err(|e| AppError::Internal(format!("invalid file id in mirror source: {e}")))?;

    let objects = StorageObjectRepository::find_by_file_id(state.db.pool(), file_id).await?;
    let mut ordered: Vec<_> = objects.iter().collect();
    ordered.sort_by_key(|obj| if obj.backend == source_bucket { 0 } else { 1 });

    let mut last_error: Option<String> = None;
    for obj in ordered {
        let backend = { state.storage.read().await.get(&obj.backend) };
        let Some(backend) = backend else {
            last_error = Some(format!("storage backend '{}' is not registered", obj.backend));
            continue;
        };
        match backend.get(&obj.storage_path).await {
            Ok(Some(data)) => return Ok(data),
            Ok(None) => {
                last_error = Some(format!(
                    "blob '{}' is missing from backend '{}'",
                    obj.storage_path, obj.backend
                ));
            }
            Err(e) => {
                last_error = Some(format!(
                    "failed to read blob '{}' from backend '{}': {e}",
                    obj.storage_path, obj.backend
                ));
            }
        }
    }

    // Fallback: the content-addressed key under the source bucket's backend.
    if hash.len() >= 4 {
        let key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);
        let backend = { state.storage.read().await.get(source_bucket) };
        if let Some(backend) = backend {
            if let Ok(Some(data)) = backend.get(&key).await {
                return Ok(data);
            }
        }
    }

    Err(AppError::Storage(last_error.unwrap_or_else(|| {
        format!("no physical copy found for file {file_id}")
    })))
}
