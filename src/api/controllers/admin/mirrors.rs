use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use chrono::Utc;
use tracing::info;
use uuid::Uuid;

use crate::api::extractors::AuthUser;
use crate::db::repos::mirror_jobs::{MirrorJobChanges, NewMirrorJob};
use crate::db::repos::{BucketRepository, MirrorJobRepository};
use crate::dto::*;
use crate::error::{AppError, AppResult};
use crate::utils::schedule::{next_run_after, parse_schedule_time};
use crate::AppState;

const MAX_JOB_NAME_LEN: usize = 200;

fn validate_job_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("job name is required".into()));
    }
    if name.chars().count() > MAX_JOB_NAME_LEN {
        return Err(AppError::Validation(format!(
            "job name must be at most {MAX_JOB_NAME_LEN} characters"
        )));
    }
    Ok(name.to_string())
}

async fn validate_buckets(state: &AppState, source: &str, target: &str) -> AppResult<()> {
    if source == target {
        return Err(AppError::Validation(
            "source and target buckets must be different".into(),
        ));
    }
    if BucketRepository::find_by_name(state.db.pool(), source)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound(format!(
            "source bucket '{source}' not found"
        )));
    }
    if BucketRepository::find_by_name(state.db.pool(), target)
        .await?
        .is_none()
    {
        return Err(AppError::NotFound(format!(
            "target bucket '{target}' not found"
        )));
    }
    Ok(())
}

fn validate_schedule(schedule_time: &str) -> AppResult<String> {
    parse_schedule_time(schedule_time).map_err(AppError::Validation)?;
    Ok(schedule_time.trim().to_string())
}

fn schedule_next(schedule_time: &str, enabled: bool) -> AppResult<Option<String>> {
    if !enabled {
        return Ok(None);
    }
    let next = next_run_after(schedule_time, Utc::now()).map_err(AppError::Validation)?;
    Ok(Some(next.to_rfc3339()))
}

/// GET /api/admin/mirror-jobs
pub async fn list_mirror_jobs(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<MirrorJobDto>>> {
    auth.require_admin()?;
    let jobs = MirrorJobRepository::list(state.db.pool()).await?;
    Ok(Json(jobs.into_iter().map(MirrorJobDto::from).collect()))
}

/// POST /api/admin/mirror-jobs
pub async fn create_mirror_job(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateMirrorJobRequest>,
) -> AppResult<Json<MirrorJobDto>> {
    auth.require_admin()?;

    let name = validate_job_name(&body.name)?;
    validate_buckets(&state, &body.source_bucket, &body.target_bucket).await?;
    let schedule_time = validate_schedule(&body.schedule_time)?;
    let next_run_at = schedule_next(&schedule_time, body.enabled)?;

    let job = MirrorJobRepository::create(
        state.db.pool(),
        NewMirrorJob {
            id: Uuid::new_v4(),
            name,
            job_type: "mirror_bucket".to_string(),
            source_bucket: body.source_bucket.clone(),
            target_bucket: body.target_bucket.clone(),
            schedule_time,
            enabled: body.enabled,
            prune: body.prune,
            next_run_at,
        },
    )
    .await?;

    info!(
        "admin {} created mirror job '{}' ({} -> {}, daily {})",
        auth.username, job.name, job.source_bucket, job.target_bucket, job.schedule_time
    );
    Ok(Json(MirrorJobDto::from(job)))
}

/// PUT /api/admin/mirror-jobs/:id
pub async fn update_mirror_job(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateMirrorJobRequest>,
) -> AppResult<Json<MirrorJobDto>> {
    auth.require_admin()?;

    let existing = MirrorJobRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("mirror job {id} not found")))?;
    if existing.running {
        return Err(AppError::Conflict(
            "cannot edit a mirror job while it is running".into(),
        ));
    }

    let name = validate_job_name(&body.name)?;
    validate_buckets(&state, &body.source_bucket, &body.target_bucket).await?;
    let schedule_time = validate_schedule(&body.schedule_time)?;
    let next_run_at = schedule_next(&schedule_time, body.enabled)?;

    let job = MirrorJobRepository::update(
        state.db.pool(),
        id,
        MirrorJobChanges {
            name,
            source_bucket: body.source_bucket,
            target_bucket: body.target_bucket,
            schedule_time,
            enabled: body.enabled,
            prune: body.prune,
            next_run_at,
        },
    )
    .await?
    .ok_or_else(|| AppError::NotFound(format!("mirror job {id} not found")))?;

    info!("admin {} updated mirror job '{}'", auth.username, job.name);
    Ok(Json(MirrorJobDto::from(job)))
}

/// DELETE /api/admin/mirror-jobs/:id
pub async fn delete_mirror_job(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;

    let existing = MirrorJobRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("mirror job {id} not found")))?;
    if existing.running {
        return Err(AppError::Conflict(
            "cannot delete a mirror job while it is running".into(),
        ));
    }

    MirrorJobRepository::delete(state.db.pool(), id).await?;
    info!("admin {} deleted mirror job '{}'", auth.username, existing.name);
    Ok(Json(MessageResponse {
        message: format!("mirror job '{}' deleted", existing.name),
    }))
}

/// POST /api/admin/mirror-jobs/:id/run
/// Re-arm the job for immediate execution. Disabled jobs run once without
/// being re-enabled.
pub async fn run_mirror_job_now(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MirrorJobDto>> {
    auth.require_admin()?;

    let job = MirrorJobRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("mirror job {id} not found")))?;
    if job.running {
        return Err(AppError::Conflict("mirror job is already running".into()));
    }

    let now = Utc::now().to_rfc3339();
    MirrorJobRepository::set_next_run_at(state.db.pool(), id, Some(&now)).await?;
    let job = MirrorJobRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("mirror job {id} not found")))?;

    info!(
        "admin {} queued mirror job '{}' for immediate run",
        auth.username, job.name
    );
    Ok(Json(MirrorJobDto::from(job)))
}
