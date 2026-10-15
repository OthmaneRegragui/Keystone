use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::{ConnectionRepository, StorageConfig, StoragePathRepository};
use tracing::info;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

/// Fallback base used when `STORAGE_LOCAL_PATHS` is not configured.
const DEFAULT_ENV_BASE: &str = "./storage";

/// First directory of `STORAGE_LOCAL_PATHS`; every new storage path is created under it.
fn env_base(state: &AppState) -> String {
    state
        .config
        .storage
        .local_paths
        .first()
        .cloned()
        .unwrap_or_else(|| DEFAULT_ENV_BASE.to_string())
}

/// Resolve `env_base` to a normalized absolute path, failing closed when the
/// configured base is the filesystem root, contains `..` components, or is
/// otherwise unusable. Stored `storage_paths.path` rows are compared against
/// `buckets.path` and are meant to be absolute, so a broken base must never
/// leak into them.
fn resolve_env_base(state: &AppState) -> Result<String, AppError> {
    let base = env_base(state);
    if base.is_empty() || base.contains('\0') {
        return Err(AppError::Internal(
            "storage base is empty or contains NUL bytes (STORAGE_LOCAL_PATHS)".into(),
        ));
    }
    let expanded = if base.starts_with('/') {
        base
    } else {
        // Relative bases (e.g. the "./storage" default) are anchored at CWD.
        std::env::current_dir()
            .map_err(|e| AppError::Internal(format!("cannot resolve storage base: {e}")))?
            .join(&base)
            .to_string_lossy()
            .into_owned()
    };
    let segments: Vec<&str> = expanded.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(AppError::Internal(
            "storage base is the filesystem root; refusing to create storage paths under '/'".into(),
        ));
    }
    if segments.iter().any(|s| *s == "..") {
        return Err(AppError::Internal(
            "storage base contains '..' components; refusing to create paths outside the base".into(),
        ));
    }
    Ok(format!("/{}", segments.join("/")))
}

pub async fn list_storage_paths(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<StoragePathDto>>> {
    auth.require_admin()?;
    let paths = StoragePathRepository::list(state.db.pool()).await?;
    // Resolve each path's assigned connection once, so the DTO can show its name
    // and type without a query per row.
    let connections: std::collections::HashMap<String, crate::models::Connection> =
        ConnectionRepository::list(state.db.pool())
            .await?
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
    let mut dtos = Vec::with_capacity(paths.len());
    for sp in paths {
        let file_count = StoragePathRepository::file_count_for_path(state.db.pool(), &sp.path).await.unwrap_or(0);
        let total_size = StoragePathRepository::total_size_for_path(state.db.pool(), &sp.path).await.unwrap_or(0);
        let bucket_count = StoragePathRepository::bucket_count_for_path(state.db.pool(), &sp.path).await.unwrap_or(0);
        let connection = sp.connection_id.as_ref().and_then(|id| connections.get(id));
        dtos.push(StoragePathDto::from_path(sp, connection, file_count, total_size, bucket_count));
    }
    Ok(Json(dtos))
}

pub async fn create_storage_path(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateStoragePathRequest>,
) -> AppResult<Json<StoragePathDto>> {
    auth.require_admin()?;
    if body.name.trim().is_empty() {
        return Err(AppError::Validation("name is required".into()));
    }

    let slug = slugify(&body.name);
    if slug.is_empty() {
        return Err(AppError::Validation("name must contain letters or numbers".into()));
    }
    // `slugify` output is restricted to `[a-z0-9-]`, so once the base is
    // validated the combined path is guaranteed to stay inside it.
    let base = resolve_env_base(&state)?;
    let path = format!("{}/{}", base, slug);

    // An assigned connection must exist; a missing one is a client error rather
    // than a dangling reference.
    let connection = match body.connection_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => Some(
            ConnectionRepository::find_by_id(state.db.pool(), id)
                .await?
                .ok_or_else(|| AppError::Validation(format!("connection '{id}' not found")))?,
        ),
        None => None,
    };
    if connection.is_none() && body.delete_local_after_upload {
        return Err(AppError::Validation(
            "delete_local_after_upload requires an assigned connection".into(),
        ));
    }

    let config = StorageConfig {
        connection_id: connection.as_ref().map(|c| c.id.clone()),
        delete_local_after_upload: body.delete_local_after_upload,
    };
    let sp = StoragePathRepository::create_with(state.db.pool(), &body.name, &path, config).await?;
    // A new path can "adopt" buckets that already live under it (their stored
    // path shares this prefix), so rebuild every backend to pick up the new
    // ownership. FTP backends connect lazily, so this is a cheap, offline step.
    state.reload_storage_backends().await?;
    info!("admin {} created storage path '{}' at '{}'", auth.username, sp.name, sp.path);
    Ok(Json(StoragePathDto::from_path(sp, connection.as_ref(), 0, 0, 0)))
}

pub async fn get_storage_base(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<StorageBaseDto>> {
    auth.require_admin()?;
    Ok(Json(StorageBaseDto {
        env_base: env_base(&state),
    }))
}

/// Lowercase the input, keep only ASCII alphanumeric characters, collapse every
/// run of other characters into a single `-`, and trim leading/trailing `-`.
fn slugify(input: &str) -> String {
    let mut out = String::new();
    for c in input.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

pub async fn delete_storage_path(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<DeleteStoragePathRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    StoragePathRepository::delete(state.db.pool(), &body.id).await?;
    // Buckets that lived under the removed path fall back to local disk, so
    // rebuild their backends immediately instead of waiting for a restart.
    state.reload_storage_backends().await?;
    info!("admin {} deleted storage path '{}'", auth.username, body.id);
    Ok(Json(MessageResponse { message: "storage path deleted".into() }))
}
