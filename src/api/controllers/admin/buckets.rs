use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::{BucketRepository, StorageObjectRepository, UserFileRepository};
use crate::storage::local::LocalFsBackend;
use crate::utils::traits::StorageBackend;
use crate::utils::names::validate_component_name;
use tracing::{info, warn};

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

/// Validate a bucket storage path before it becomes the physical root of a
/// `LocalFsBackend`. The path is stored in the DB and passed verbatim to
/// `create_dir_all` + backend resolution, so `..`, relative or root paths
/// would anchor the backend outside the intended storage area (or at `/`).
/// Docs and tests treat bucket paths as absolute; keep it that way.
fn validate_bucket_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("path is required".to_string());
    }
    if path.contains('\0') {
        return Err("path must not contain NUL bytes".to_string());
    }
    if !path.starts_with('/') {
        return Err("path must be an absolute path".to_string());
    }
    if path.chars().any(|c| c.is_control()) {
        return Err("path must not contain control characters".to_string());
    }
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err("path must not be the filesystem root".to_string());
    }
    if segments.iter().any(|s| *s == "..") {
        return Err("path must not contain '..' components".to_string());
    }
    Ok(())
}

pub async fn list_buckets(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<BucketDto>>> {
    auth.require_admin()?;
    let buckets = BucketRepository::list(state.db.pool()).await?;
    let usage = BucketRepository::get_storage_used_per_bucket(state.db.pool()).await.unwrap_or_default();
    let deleted = UserFileRepository::deleted_stats_per_bucket(state.db.pool()).await.unwrap_or_default();
    let deleted_map: std::collections::HashMap<String, (i64, i64)> = deleted
        .into_iter()
        .map(|(name, count, size)| (name, (count, size)))
        .collect();
    let orphaned = UserFileRepository::orphaned_physical_files_per_bucket(state.db.pool()).await.unwrap_or_default();
    let orphaned_map: std::collections::HashMap<String, (i64, i64)> = orphaned
        .into_iter()
        .map(|(name, count, size)| (name, (count, size)))
        .collect();
    Ok(Json(buckets.into_iter().map(|b| {
        let used = usage.get(&b.name).copied().unwrap_or(0);
        let (del_count, del_size) = deleted_map.get(&b.name).copied().unwrap_or((0, 0));
        let (orb_count, orb_size) = orphaned_map.get(&b.name).copied().unwrap_or((0, 0));
        BucketDto::from_bucket(b, used, del_count, del_size, orb_count, orb_size)
    }).collect()))
}

pub async fn create_bucket(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateBucketRequest>,
) -> AppResult<Json<BucketDto>> {
    auth.require_admin()?;
    if body.name.is_empty() || body.path.is_empty() {
        return Err(AppError::Validation("name and path are required".into()));
    }
    validate_component_name(body.name.trim()).map_err(|e| {
        AppError::Validation(format!("invalid bucket name: {e}"))
    })?;
    validate_bucket_path(&body.path).map_err(|e| {
        AppError::Validation(format!("invalid bucket path: {e}"))
    })?;
    let bucket = BucketRepository::create(state.db.pool(), body.name.trim(), &body.path).await?;
    let path_clone = body.path.clone();
    let name_clone = body.name.clone();
    let backend = std::sync::Arc::new(
        LocalFsBackend::new(&path_clone)
            .map_err(|e| AppError::Internal(format!("failed to create storage at '{}': {e}", path_clone)))?
    );
    {
        let mut storage = state.storage.write().await;
        storage.register(name_clone.clone(), backend);
    }
    info!("admin {} created bucket '{}' at '{}'", auth.username, bucket.name, path_clone);
    Ok(Json(BucketDto::from_bucket(bucket, 0, 0, 0, 0, 0)))
}

pub async fn set_bucket_visible(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<SetBucketVisibleRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    BucketRepository::set_visible(state.db.pool(), &body.name, body.visible).await?;
    info!("admin {} set bucket '{}' visible={}", auth.username, body.name, body.visible);
    Ok(Json(MessageResponse { message: format!("bucket '{}' visibility updated", body.name) }))
}

pub async fn update_bucket(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateBucketRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    if body.name.is_empty() || body.path.is_empty() {
        return Err(AppError::Validation("name and path are required".into()));
    }
    validate_component_name(body.name.trim()).map_err(|e| {
        AppError::Validation(format!("invalid bucket name: {e}"))
    })?;
    validate_bucket_path(&body.path).map_err(|e| {
        AppError::Validation(format!("invalid bucket path: {e}"))
    })?;
    if body.storage_limit < 0 {
        return Err(AppError::BadRequest(
            "storage limit cannot be negative".into(),
        ));
    }
    // Check if path or name changed — if so, re-register the backend
    let old_bucket = BucketRepository::find_by_name(state.db.pool(), &body.original_name).await?;
    let path_changed = old_bucket.as_ref().map_or(false, |b| b.path != body.path);
    let name_changed = body.original_name != body.name;
    BucketRepository::update(state.db.pool(), &body.original_name, &body.name, &body.path, body.visible_to_users, body.is_active, body.storage_limit).await?;
    if path_changed || name_changed {
        let backend = std::sync::Arc::new(
            LocalFsBackend::new(&body.path)
                .map_err(|e| AppError::Internal(format!("failed to create storage at '{}': {e}", body.path)))?
        );
        let mut storage = state.storage.write().await;
        // Remove old entry if name changed
        if name_changed {
            storage.remove(&body.original_name);
        }
        storage.register(body.name.clone(), backend);
    }
    info!("admin {} updated bucket '{}' -> '{}'", auth.username, body.original_name, body.name);
    Ok(Json(MessageResponse { message: format!("bucket '{}' updated", body.name) }))
}

pub async fn delete_bucket(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<DeleteBucketRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    // Explicitly remove group_buckets references (bucket_name is not a FK, so no cascade)
    sqlx::query("DELETE FROM group_buckets WHERE bucket_id = (SELECT id FROM buckets WHERE name = $1)")
        .bind(&body.name)
        .execute(state.db.pool())
        .await
        .map_err(|e| AppError::Internal(format!("failed to clean group_buckets: {e}")))?;
    BucketRepository::delete(state.db.pool(), &body.name).await?;
    state.storage.write().await.remove(&body.name);
    info!("admin {} deleted bucket '{}'", auth.username, body.name);
    Ok(Json(MessageResponse { message: format!("bucket '{}' deleted", body.name) }))
}

pub async fn list_storage_backends(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<String>>> {
    auth.require_admin()?;
    Ok(Json(state.storage.read().await.list_backends()))
}

/// Change the storage path of a bucket.
/// 1. Validates the new path exists (or can be created)
/// 2. Moves all physical files from old path to new path
/// 3. Cleans up empty directories left behind in old path
/// 4. Updates the DB bucket path
/// 5. Re-registers the storage backend with the new path
pub async fn change_bucket_path(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ChangeBucketPathRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;

    if body.bucket_name.is_empty() || body.new_path.is_empty() {
        return Err(AppError::Validation("bucket_name and new_path are required".into()));
    }
    validate_bucket_path(&body.new_path).map_err(|e| {
        AppError::Validation(format!("invalid new path: {e}"))
    })?;

    // Get current bucket
    let bucket = BucketRepository::find_by_name(state.db.pool(), &body.bucket_name).await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", body.bucket_name)))?;

    let old_path = bucket.path.clone();
    let new_path = body.new_path.clone();

    // Same path — nothing to do
    if old_path == new_path {
        return Ok(Json(MessageResponse { message: "path is unchanged".into() }));
    }

    // Validate new path: try to create it, fail if a parent doesn't exist
    match std::fs::metadata(&new_path) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(AppError::Validation(format!(
                    "'{new_path}' exists but is not a directory"
                )));
            }
        }
        Err(_) => {
            std::fs::create_dir_all(&new_path).map_err(|e| AppError::Validation(format!(
                "cannot create '{new_path}': {e}"
            )))?;
        }
    }

    // List all physical files in this bucket
    let objects = StorageObjectRepository::list_by_backend(state.db.pool(), &body.bucket_name).await?;
    let file_count = objects.len();

    // Get the old backend to read files from
    let old_backend = {
        let storage = state.storage.read().await;
        storage.get(&body.bucket_name)
            .ok_or_else(|| AppError::Internal(format!("storage backend '{}' not registered in memory", body.bucket_name)))?
    };

    // Create the new backend at the target path
    let new_backend = std::sync::Arc::new(
        LocalFsBackend::new(&new_path)
            .map_err(|e| AppError::Internal(format!("failed to create storage at '{new_path}': {e}")))?
    );

    // Move each file: read from old backend, write to new backend
    let mut moved = 0u64;
    let mut errors = Vec::new();
    for obj in &objects {
        match old_backend.get(&obj.storage_path).await {
            Ok(Some(data)) => {
                match new_backend.put(&obj.storage_path, data).await {
                    Ok(()) => {
                        let _ = old_backend.delete(&obj.storage_path).await;
                        moved += 1;
                    }
                    Err(e) => {
                        errors.push(format!("write '{}': {}", obj.storage_path, e));
                    }
                }
            }
            Ok(None) => {
                warn!("storage object '{}' (path '{}') not found on disk — skipping", obj.id, obj.storage_path);
            }
            Err(e) => {
                errors.push(format!("read '{}': {}", obj.storage_path, e));
            }
        }
    }

    // Clean up empty directories left behind in old path
    cleanup_empty_dirs(&old_path);

    // Update DB
    BucketRepository::update_path(state.db.pool(), &body.bucket_name, &new_path).await?;

    // Re-register the new backend in storage registry
    {
        let mut storage = state.storage.write().await;
        storage.register(body.bucket_name.clone(), new_backend);
    }

    let err_count = errors.len();
    let summary = if err_count > 0 {
        format!(
            "moved {moved}/{file_count} files to '{new_path}' ({err_count} errors: {})",
            errors.join("; ")
        )
    } else {
        format!("moved {moved} files to '{new_path}'")
    };

    info!(
        "admin {} changed bucket '{}' path: '{}' → '{}' ({})",
        auth.username, body.bucket_name, old_path, new_path, summary
    );

    Ok(Json(MessageResponse { message: summary }))
}

/// Recursively remove empty directories bottom-up inside `dir`.
/// Stops at `dir` itself (does NOT delete `dir`).
fn cleanup_empty_dirs(dir: &str) {
    let root = std::path::Path::new(dir);
    if !root.is_dir() {
        return;
    }
    // Walk recursively, then clean bottom-up
    fn walk_and_clean(dir: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_and_clean(&path);
                }
            }
        }
        // After processing children, try to remove if empty
        // (skip the root dir itself)
        if dir.read_dir().ok().map_or(false, |mut e| e.next().is_none()) {
            let _ = std::fs::remove_dir(dir);
        }
    }
    walk_and_clean(root);
}
