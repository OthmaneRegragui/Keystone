use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;
use crate::error::{AppError, AppResult};
use crate::db::repos::{FileRepository, StorageObjectRepository, UserFileRepository};
use crate::models::StorageObject;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct OrphanedFilesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// Filter by the last reference's bucket name.
    pub bucket: Option<String>,
    /// Filter by the last reference's owner user id.
    pub owner: Option<String>,
}

/// GET /api/admin/orphaned-files — list physical files with no active user
/// reference: every reference is soft-deleted (still in someone's trash) or
/// was permanently deleted from the trash. They occupy disk but no user can
/// reach them. Admin only.
pub async fn list_orphaned_files(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(params): Query<OrphanedFilesQuery>,
) -> AppResult<Json<OrphanedFilesDto>> {
    auth.require_admin()?;
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let offset = params.offset.unwrap_or(0).max(0);
    let bucket = params.bucket.as_deref().filter(|b| !b.is_empty());
    let owner = params.owner.as_deref().filter(|o| !o.is_empty());

    let (total, total_size_bytes) =
        UserFileRepository::orphaned_files_total(state.db.pool(), bucket, owner).await?;
    let rows = UserFileRepository::orphaned_files_page(state.db.pool(), limit, offset, bucket, owner)
        .await?;

    let files = rows
        .into_iter()
        .map(|r| OrphanedFileDto {
            id: r.file_id,
            hash: r.blake3_hash,
            name: r.original_name,
            size_bytes: r.size,
            created_at: r.created_at,
            bucket: r.bucket_name,
            deleted_at: r.deleted_at,
            owner: r.username,
        })
        .collect();

    Ok(Json(OrphanedFilesDto {
        total,
        total_size_bytes,
        files,
    }))
}

/// GET /api/admin/orphaned-files/facets — distinct buckets and owners among
/// orphaned files, for the filter dropdowns on the admin page. Admin only.
pub async fn list_orphaned_file_facets(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<OrphanedFacetsDto>> {
    auth.require_admin()?;

    let (buckets, owners) = UserFileRepository::orphaned_file_facets(state.db.pool()).await?;

    Ok(Json(OrphanedFacetsDto {
        buckets,
        users: owners
            .into_iter()
            .map(|(id, username)| OrphanedOwnerDto { id, username })
            .collect(),
    }))
}

/// Purge the physical file identified by `file_id`: delete its storage objects
/// from their backends, then the user_files references and finally the files
/// record. Mirrors the GC logic, but applies to orphaned files whose references
/// are all soft-deleted. Returns the number of storage objects removed.
async fn purge_file(state: &Arc<AppState>, file_id: Uuid) -> AppResult<usize> {
    let pool = state.db.pool();

    if !UserFileRepository::is_orphaned_file(pool, file_id).await? {
        return Err(AppError::BadRequest("file is not orphaned".into()));
    }

    let objects: Vec<StorageObject> =
        StorageObjectRepository::find_by_file_id(pool, file_id).await?;

    // Resolve backend handles up-front (read lock is not held across awaits).
    let mut backends: Vec<Option<Arc<dyn crate::storage::backend::StorageBackend>>> = Vec::new();
    {
        let storage = state.storage.read().await;
        for obj in &objects {
            backends.push(storage.get(&obj.backend));
        }
    }

    for (obj, backend) in objects.iter().zip(backends.iter()) {
        if let Some(backend) = backend {
            match backend.delete(&obj.storage_path).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(
                        "failed to delete storage object '{}' from backend '{}': {}",
                        obj.storage_path,
                        obj.backend,
                        e
                    );
                }
            }
        } else {
            tracing::warn!(
                "backend '{}' not found for storage object '{}', skipping",
                obj.backend,
                obj.storage_path
            );
        }

        if let Err(e) = StorageObjectRepository::delete(pool, obj.id).await {
            tracing::warn!("failed to delete storage object record {}: {}", obj.id, e);
        }
    }

    UserFileRepository::delete_by_file(pool, file_id).await?;

    match FileRepository::delete(pool, file_id).await {
        Ok(true) => Ok(objects.len()),
        Ok(false) => Err(AppError::NotFound("file record not found".into())),
        Err(e) => Err(e),
    }
}

/// GET /api/admin/orphaned-files/:id/download — stream an orphaned physical
/// file's bytes as an attachment so admins can inspect it before purging.
/// Admin only.
pub async fn download_orphaned_file(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, AppError> {
    auth.require_admin()?;

    if !UserFileRepository::is_orphaned_file(state.db.pool(), id).await? {
        return Err(AppError::BadRequest("file is not orphaned".into()));
    }

    let file = FileRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let storage_objects =
        StorageObjectRepository::find_by_file_id(state.db.pool(), file.id).await?;
    let storage_obj = storage_objects
        .first()
        .ok_or_else(|| AppError::NotFound("file not found in storage".into()))?;

    let backend = {
        let storage = state.storage.read().await;
        storage
            .get(&storage_obj.backend)
            .ok_or_else(|| AppError::Internal("storage backend not found".into()))?
    };

    let data = backend
        .get(&storage_obj.storage_path)
        .await
        .map_err(|e| AppError::Storage(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("file data not found in storage".into()))?;

    // Name from the most recent reference (tombstone included), falling back
    // to the physical record; mime from the physical row, sanitized at upload.
    let name = UserFileRepository::orphan_display_name(state.db.pool(), id).await?;
    let content_type = file
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // ASCII-only, quote/backslash-free filename so the header can never be
    // poisoned with header injection and never fails to serialize.
    let safe_name: String = name
        .chars()
        .filter(|c| c.is_ascii() && !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let safe_name = if safe_name.is_empty() {
        "download".to_string()
    } else {
        safe_name
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        content_type
            .parse()
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{safe_name}\"")
            .parse()
            .map_err(|_| AppError::Internal("invalid content-disposition header".into()))?,
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::CONTENT_LENGTH, data.len().into());

    Ok((StatusCode::OK, headers, data).into_response())
}

/// DELETE /api/admin/orphaned-files/:id — purge a single orphaned physical
/// file (storage objects, references and record). Admin only.
pub async fn delete_orphaned_file(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;

    FileRepository::find_by_id(state.db.pool(), id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    purge_file(&state, id).await?;

    Ok(Json(MessageResponse {
        message: "orphaned file deleted".into(),
    }))
}

/// DELETE /api/admin/orphaned-files — purge ALL orphaned physical files.
/// Admin only.
pub async fn delete_all_orphaned_files(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<OrphanedDeleteResultDto>> {
    auth.require_admin()?;

    let ids = UserFileRepository::orphaned_file_ids(state.db.pool()).await?;

    let mut deleted = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    for id in ids {
        let parsed = match Uuid::parse_str(&id) {
            Ok(v) => v,
            Err(_) => {
                failed += 1;
                errors.push(format!("{id}: invalid id"));
                continue;
            }
        };
        match purge_file(&state, parsed).await {
            Ok(_) => deleted += 1,
            Err(e) => {
                failed += 1;
                errors.push(format!("{id}: {e}"));
            }
        }
    }

    Ok(Json(OrphanedDeleteResultDto {
        deleted,
        failed,
        errors,
    }))
}
