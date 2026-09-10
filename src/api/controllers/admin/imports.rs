use std::collections::HashSet;
use std::io::Read;
use std::sync::Arc;

use axum::extract::{Multipart, Path, State};
use axum::Json;
use bytes::Bytes;
use serde::Serialize;
use tracing::info;
use uuid::Uuid;

use crate::api::extractors::AuthUser;
use crate::db::repos::buckets::BucketRepository;
use crate::db::repos::files::FileRepository;
use crate::db::repos::folders::FolderRepository;
use crate::db::repos::storage::StorageObjectRepository;
use crate::db::repos::user_files::UserFileRepository;
use crate::db::repos::users::UserRepository;
use crate::db::rows::{CreateStorageObjectData, FileRecord, FolderRecord, UserFileRecord};
use crate::dto::BucketIndexExportDto;
use crate::error::{AppError, AppResult};
use crate::models::File;
use crate::utils::hashing::blake3::hash_bytes;
use crate::utils::names::validate_component_name;
use crate::AppState;

// ─── Response ─────────────────────────────────────────────────

/// Hard cap on the number of ZIP entries processed in a single import.
/// Bound per loop iteration to prevent CPU/memory DoS from pathological archives.
const MAX_ZIP_ENTRIES: usize = 20_000;

/// Total decompressed bytes across all entries may not exceed this multiple of
/// the configured per-upload limit (`storage.max_upload_size_mb`).
const MAX_TOTAL_UNCOMPRESSED_MULT: u64 = 10;

/// The uploaded ZIP itself may not exceed this multiple of `max_upload_size_mb`.
const MAX_ZIP_UPLOAD_MULT: u64 = 4;

/// Hard cap on the number of users accepted in an index JSON import.
const MAX_INDEX_USERS: usize = 20_000;

/// Read a ZIP entry's decompressed data under a hard byte limit (zip-bomb
/// protection). The declared size from the central directory is checked first,
/// and the actual read is clamped to `per_entry_limit + 1` bytes so a lying
/// archive can never inflate past the limit.
fn read_entry_bounded(
    entry: &mut zip::read::ZipFile,
    per_entry_limit: u64,
) -> Result<Vec<u8>, String> {
    if entry.size() > per_entry_limit {
        return Err(format!(
            "declared size {} exceeds limit of {} bytes",
            entry.size(),
            per_entry_limit
        ));
    }
    let mut buf = Vec::new();
    let mut limited = entry.by_ref().take(per_entry_limit + 1);
    limited
        .read_to_end(&mut buf)
        .map_err(|e| format!("read error: {e}"))?;
    if buf.len() as u64 > per_entry_limit {
        return Err(format!(
            "decompressed size {} exceeds limit of {} bytes",
            buf.len(),
            per_entry_limit
        ));
    }
    Ok(buf)
}

/// True when the entry's Unix mode marks it as a symlink. Symlinks are never
/// imported: they carry no file data and must not be materialized on disk.
fn entry_is_symlink(entry: &zip::read::ZipFile) -> bool {
    entry.unix_mode().map(|m| m & 0o170000 == 0o120000).unwrap_or(false)
}

/// Read a multipart file field while streaming, enforcing a hard byte limit as
/// bytes arrive. The server sets no global request body limit, so an unbounded
/// `field.bytes()` would let a client stream an arbitrarily large upload into
/// memory before any size check runs.
async fn read_field_bounded(
    field: &mut axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<Bytes, String> {
    let mut buf = Vec::new();
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                buf.extend_from_slice(&chunk);
                if buf.len() > limit {
                    return Err(format!("upload exceeds limit of {limit} bytes"));
                }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("failed to read upload: {e}")),
        }
    }
    Ok(Bytes::from(buf))
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub files_imported: usize,
    pub folders_created: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportFileResult {
    pub name: String,
    pub size: i64,
    pub hash: String,
}

// ─── Import ZIP ───────────────────────────────────────────────

pub async fn import_bucket_zip(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Json<ImportResult>> {
    auth.require_admin()?;

    // Verify bucket exists
    BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Zip-bomb / DoS limits derived from the configured per-upload limit.
    // Computed before reading the upload so the multipart read itself is bounded.
    let per_entry_limit = (state.config.storage.max_upload_size_mb * 1024 * 1024) as u64;
    let total_limit = MAX_TOTAL_UNCOMPRESSED_MULT * per_entry_limit;
    let upload_limit = MAX_ZIP_UPLOAD_MULT * per_entry_limit;

    // Extract ZIP file from multipart
    let mut zip_data: Option<Bytes> = None;
    while let Ok(Some(mut field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            zip_data = Some(
                read_field_bounded(&mut field, upload_limit as usize)
                    .await
                    .map_err(|e| AppError::BadRequest(format!("failed to read uploaded file: {e}")))?,
            );
            break;
        }
    }

    let zip_bytes = zip_data.ok_or_else(|| AppError::BadRequest("no file field in upload".into()))?;

    if zip_bytes.len() as u64 > upload_limit {
        return Err(AppError::BadRequest(format!(
            "ZIP too large: {} bytes (max: {} MB)",
            zip_bytes.len(),
            upload_limit / (1024 * 1024)
        )));
    }

    // Read ZIP archive
    let reader = std::io::Cursor::new(&zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| AppError::BadRequest(format!("invalid zip file: {e}")))?;

    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(AppError::BadRequest(format!(
            "ZIP contains too many entries: {} (max: {})",
            archive.len(),
            MAX_ZIP_ENTRIES
        )));
    }

    // The target bucket is also the storage backend for this import: buckets
    // keep their own physical copies, so every blob lands in this bucket's own
    // storage directory rather than a shared "first available" backend.
    let backend_name = bucket_name.clone();

    let mut result = ImportResult {
        files_imported: 0,
        folders_created: 0,
        errors: Vec::new(),
    };

    // Pre-fetch known bucket names so we can strip any leading bucket prefix
    // from ZIP paths. This lets export ZIPs from *any* bucket be imported
    // into this bucket without manual path rewriting.
    let known_buckets: Vec<String> = match BucketRepository::list(state.db.pool()).await {
        Ok(buckets) => buckets.into_iter().map(|b| b.name).collect(),
        Err(_) => Vec::new(),
    };

    let mut total_uncompressed: u64 = 0;

    // Process each ZIP entry
    for i in 0..archive.len() {
        // ── Extract metadata + data from the entry synchronously ──
        // (ZipFile is not Send, so we must drop it before any .await)
        let (entry_path, username, file_name, folder_segments, raw_key, data_or_err) = {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(e) => {
                    result.errors.push(format!("entry #{i}: {e}"));
                    continue;
                }
            };

            // Skip directories (trailing /)
            let entry_path = entry.name().to_string();
            if entry_path.ends_with('/') || entry.is_dir() {
                continue;
            }

            // Skip symlink entries — never materialize links from an archive.
            if entry_is_symlink(&entry) {
                result
                    .errors
                    .push(format!("skipped '{entry_path}': symlink entries are not imported"));
                continue;
            }

            // Normalise path: strip optional leading bucket name prefix so
            // export ZIPs from *any* bucket can be imported here.
            let normalized = entry_path.replace('\\', "/");
            let trimmed = normalized.trim_start_matches('/');

            // If the first segment matches any known bucket, strip it
            let trimmed = if let Some(first_slash) = trimmed.find('/') {
                let first_segment = &trimmed[..first_slash];
                if known_buckets.iter().any(|b| b == first_segment) {
                    &trimmed[first_slash + 1..]
                } else {
                    trimmed
                }
            } else {
                trimmed
            };

            // Raw backups (the raw storage-tree export) contain entries whose
            // tail is the blake3 shard path `xx/yy/<hash>`. They carry no owner
            // metadata, so they are restored as content-addressed blobs only.
            let raw_key = raw_blob_key(trimmed);

            let parts: Vec<&str> = trimmed.split('/').collect();

            if parts.len() < 2 {
                result
                    .errors
                    .push(format!("skipped '{}': path must be username/file or bucket/username/file", entry_path));
                continue;
            }

            let username = parts[0].to_string();
            let file_name = (*parts.last().unwrap_or(&"")).to_string();
            let folder_segments: Vec<String> = if parts.len() > 2 {
                parts[1..parts.len() - 1].iter().map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };

            if username.is_empty() || file_name.is_empty() {
                result
                    .errors
                    .push(format!("skipped '{}': invalid path segments", entry_path));
                continue;
            }

            // Read file data under a hard size limit (zip-bomb protection)
            let read_result = match read_entry_bounded(&mut entry, per_entry_limit) {
                Ok(data) => Ok(Bytes::from(data)),
                Err(e) => {
                    result.errors.push(format!("skipped '{entry_path}': {e}"));
                    continue;
                }
            };

            (entry_path, username, file_name, folder_segments, raw_key, read_result)
        }; // ZipFile dropped here – safe to .await now

        let data = match data_or_err {
            Ok(d) => d,
            Err(e) => {
                result.errors.push(e);
                continue;
            }
        };

        total_uncompressed += data.len() as u64;
        if total_uncompressed > total_limit {
            result.errors.push(format!(
                "aborted: total decompressed size exceeds limit of {} bytes",
                total_limit
            ));
            break;
        }

        // ── Async operations start here ──

        // Raw storage-tree entry (no owner metadata) — restore the blob only;
        // the indexer JSON brings the user links afterwards.
        if let Some(_raw_key) = &raw_key {
            store_blob_by_hash(&state, &backend_name, data.clone(), &entry_path, &mut result).await;
            continue;
        }

        // Find or skip user
        let user = match UserRepository::find_by_username(state.db.pool(), &username).await {
            Ok(Some(u)) => u,
            Ok(None) => {
                result
                    .errors
                    .push(format!("skipped '{}': user '{}' not found", entry_path, username));
                continue;
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("skipped '{}': db error looking up user: {e}", entry_path));
                continue;
            }
        };

        // Resolve folder hierarchy
        let folder_id = if folder_segments.is_empty() {
            None
        } else {
            let segs: Vec<&str> = folder_segments.iter().map(|s| s.as_str()).collect();
            match resolve_or_create_folders(
                state.db.pool(),
                user.id,
                &bucket_name,
                &segs,
            )
            .await
            {
                Ok(fid) => {
                    result.folders_created += fid.1;
                    Some(fid.0)
                }
                Err(e) => {
                    result.errors.push(format!(
                        "skipped '{}': folder error: {e}",
                        entry_path
                    ));
                    continue;
                }
            }
        };

        // Import the file
        match import_file_data(
            &state,
            &backend_name,
            &bucket_name,
            user.id,
            &file_name,
            None::<&str>, // ZIP entries don't carry mime type
            folder_id,
            data,
        )
        .await
        {
            Ok(_) => {
                result.files_imported += 1;
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("failed to import '{}': {e}", entry_path));
            }
        }
    }

    info!(
        "admin {} imported ZIP into bucket '{}': {} files, {} folders, {} errors",
        auth.username,
        bucket_name,
        result.files_imported,
        result.folders_created,
        result.errors.len()
    );

    Ok(Json(result))
}

// ─── Import Single File ───────────────────────────────────────

pub async fn import_bucket_file(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Json<ImportFileResult>> {
    auth.require_admin()?;

    // Verify bucket exists
    BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Enforce the same per-file size limit as regular uploads, computed before
    // the multipart read so the stream itself is capped.
    let max_bytes = (state.config.storage.max_upload_size_mb * 1024 * 1024) as usize;

    // Extract fields from multipart
    let mut file_data: Option<Bytes> = None;
    let mut original_name: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut user_id: Option<String> = None;
    let mut folder_path: Option<String> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => {
                original_name = field.file_name().map(|s| s.to_string());
                content_type = field.content_type().map(|s| s.to_string());
                file_data = Some(
                    read_field_bounded(&mut field, max_bytes)
                        .await
                        .map_err(|e| AppError::BadRequest(format!("failed to read file: {e}")))?,
                );
            }
            Some("user_id") => {
                user_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("invalid user_id: {e}")))?,
                );
            }
            Some("folder_path") => {
                folder_path = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("invalid folder_path: {e}")))?,
                );
            }
            _ => {}
        }
    }

    let data = file_data.ok_or_else(|| AppError::BadRequest("no file field in upload".into()))?;

    if data.len() > max_bytes {
        return Err(AppError::BadRequest(format!(
            "file too large: {} bytes (max: {} MB)",
            data.len(),
            state.config.storage.max_upload_size_mb
        )));
    }

    let user_id = user_id.ok_or_else(|| AppError::BadRequest("no user_id field".into()))?;
    let file_name = original_name.unwrap_or_else(|| "unnamed".to_string());

    let uid = Uuid::parse_str(&user_id)
        .map_err(|_| AppError::BadRequest(format!("invalid user_id '{}'", user_id)))?;

    // Verify user exists
    let user = UserRepository::find_by_id(state.db.pool(), uid)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("user '{}' not found", user_id)))?;

    // Resolve folder path if provided
    let folder_id = if let Some(ref fp) = folder_path {
        if !fp.trim().is_empty() {
            let segments: Vec<&str> = fp.split('/').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                None
            } else {
                Some(
                    resolve_or_create_folders(state.db.pool(), uid, &bucket_name, &segments)
                        .await
                        .map_err(|e| AppError::BadRequest(format!("folder error: {e}")))?
                        .0,
                )
            }
        } else {
            None
        }
    } else {
        None
    };

    // The target bucket is also the storage backend for this import: buckets
    // keep their own physical copies, so every blob lands in this bucket's own
    // storage directory rather than a shared "first available" backend.
    let backend_name = bucket_name.clone();

    let hash = import_file_data(
        &state,
        &backend_name,
        &bucket_name,
        uid,
        &file_name,
        content_type.as_deref(),
        folder_id,
        data.clone(),
    )
    .await?;

    info!(
        "admin {} imported file '{}' into bucket '{}' for user {}",
        auth.username, file_name, bucket_name, user.username
    );

    Ok(Json(ImportFileResult {
        name: file_name,
        size: data.len() as i64,
        hash,
    }))
}

// ─── Import Indexer (JSON) ───────────────────────────────────

pub async fn import_bucket_index(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
    Json(payload): Json<BucketIndexExportDto>,
) -> AppResult<Json<ImportResult>> {
    auth.require_admin()?;

    // Verify bucket exists (the JSON bucket name is informational; the URL
    // parameter determines the target bucket so backups can be restored
    // across buckets with different names/paths).
    BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    if payload.users.len() > MAX_INDEX_USERS {
        return Err(AppError::BadRequest(format!(
            "index contains too many users: {} (max: {})",
            payload.users.len(),
            MAX_INDEX_USERS
        )));
    }

    let mut result = ImportResult {
        files_imported: 0,
        folders_created: 0,
        errors: Vec::new(),
    };

    for user_dto in &payload.users {
        // Find user by username
        let user = match UserRepository::find_by_username(state.db.pool(), &user_dto.username).await
        {
            Ok(Some(u)) => u,
            Ok(None) => {
                result
                    .errors
                    .push(format!("skipped user '{}': not found", user_dto.username));
                continue;
            }
            Err(e) => {
                result.errors.push(format!(
                    "skipped user '{}': db error: {e}",
                    user_dto.username
                ));
                continue;
            }
        };

        // Process folders first — create any that don't exist yet
        for folder_dto in &user_dto.folders {
            let segments: Vec<&str> =
                folder_dto.full_path.split('/').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() {
                continue;
            }
            match resolve_or_create_folders(state.db.pool(), user.id, &bucket_name, &segments).await
            {
                Ok((_, created)) => {
                    result.folders_created += created;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': folder '{}': {e}",
                        user_dto.username, folder_dto.full_path
                    ));
                }
            }
        }

        // Process files
        for file_dto in &user_dto.files {
            // Resolve folder path
            let folder_id = if let Some(ref fp) = file_dto.folder {
                let segments: Vec<&str> = fp.split('/').filter(|s| !s.is_empty()).collect();
                if segments.is_empty() {
                    None
                } else {
                    match resolve_or_create_folders(
                        state.db.pool(),
                        user.id,
                        &bucket_name,
                        &segments,
                    )
                    .await
                    {
                        Ok((fid, created)) => {
                            result.folders_created += created;
                            Some(fid)
                        }
                        Err(e) => {
                            result.errors.push(format!(
                                "user '{}': file '{}': folder error: {e}",
                                user_dto.username, file_dto.name
                            ));
                            continue;
                        }
                    }
                }
            } else {
                None
            };

            // Look up the physical blob by its blake3 hash
            let existing_file = match FileRepository::find_by_hash(state.db.pool(), &file_dto.hash).await {
                Ok(Some(f)) => f,
                Ok(None) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': blob with hash '{}' not found. Import files via ZIP first.",
                        user_dto.username, file_dto.name, file_dto.hash
                    ));
                    continue;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': db error looking up hash: {e}",
                        user_dto.username, file_dto.name
                    ));
                    continue;
                }
            };

            // Skip only if this user already has this file linked by the same
            // name IN THIS BUCKET — a link in another bucket must not prevent
            // the restore from materialising the file in the target bucket.
            match UserFileRepository::find_active_in_bucket_by_user_file_and_name(
                state.db.pool(),
                user.id,
                existing_file.id,
                &file_dto.name,
                &bucket_name,
            )
            .await
            {
                Ok(Some(_)) => {
                    // Already linked in this bucket — skip silently (idempotent)
                    continue;
                }
                Ok(None) => { /* proceed */ }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': check error: {e}",
                        user_dto.username, file_dto.name
                    ));
                    continue;
                }
            }

            // Create user_file entry. The name comes from client JSON — apply
            // the same component rules used everywhere else so hostile names
            // (e.g. with '/', '..', or control characters) can never reach the
            // database and later break exports or downloads.
            if let Err(e) = validate_component_name(&file_dto.name) {
                result.errors.push(format!(
                    "user '{}': file '{}': invalid name: {e}",
                    user_dto.username, file_dto.name
                ));
                continue;
            }
            let user_file_record = UserFileRecord {
                id: Uuid::new_v4(),
                user_id: user.id,
                file_id: existing_file.id,
                original_name: file_dto.name.clone(),
                mime_type: file_dto.mime_type.clone(),
                bucket_name: Some(bucket_name.clone()),
                folder_id,
            };
            let row_id = user_file_record.id;

            match UserFileRepository::create(state.db.pool(), user_file_record).await {
                Ok(_) => {
                    // Charge the user's storage with the REAL blob size — the
                    // size in the JSON is client-controlled and must not drive
                    // storage accounting. Atomic so concurrent imports cannot
                    // lose updates. Enforce the quota: if the reservation is
                    // rejected, remove the row we just created so no ghost
                    // reference is left.
                    match UserRepository::charge_storage(
                        state.db.pool(),
                        user.id,
                        existing_file.size,
                    )
                    .await
                    {
                        Ok(true) => {
                            result.files_imported += 1;
                        }
                        Ok(false) => {
                            let _ = UserFileRepository::hard_delete_by_id(
                                state.db.pool(), row_id,
                            )
                            .await;
                            result.errors.push(format!(
                                "user '{}': file '{}': storage quota exceeded",
                                user_dto.username, file_dto.name
                            ));
                        }
                        Err(e) => {
                            let _ = UserFileRepository::hard_delete_by_id(
                                state.db.pool(), row_id,
                            )
                            .await;
                            result.errors.push(format!(
                                "user '{}': file '{}': quota charge error: {e}",
                                user_dto.username, file_dto.name
                            ));
                        }
                    }
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': insert error: {e}",
                        user_dto.username, file_dto.name
                    ));
                }
            }
        }
    }

    info!(
        "admin {} imported index into bucket '{}': {} files, {} folders, {} errors",
        auth.username,
        bucket_name,
        result.files_imported,
        result.folders_created,
        result.errors.len()
    );

    Ok(Json(result))
}

// ─── Combined Import (ZIP + JSON) ────────────────────────────

struct ZipEntryData {
    entry_path: String,
    data: Bytes,
}

/// Detect a raw storage-tree blob entry (produced by the raw export). Such
/// entries end with the blake3 shard path `xx/yy/<hash>` — possibly behind
/// leading bucket/path segments. Returns the bucket-relative storage key
/// (`xx/yy/<hash>`) when it matches.
fn raw_blob_key(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    let n = parts.len();
    let hash = parts[n - 1];
    let shard1 = parts[n - 2];
    let shard0 = parts[n - 3];
    let is_hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    if shard0.len() == 2 && shard1.len() == 2 && is_hex(shard0) && is_hex(shard1) && hash.len() >= 32 && is_hex(hash)
    {
        Some(format!("{shard0}/{shard1}/{hash}"))
    } else {
        None
    }
}

/// Ensure the bucket `backend_name` holds its own physical copy of the blob
/// with hash `hash`, registering a `storage_object` for it. Buckets are
/// independent storage scopes — a backup restored into one bucket must not
/// depend on another bucket's copy — so a copy is materialized into this
/// bucket's backend even when the same content already exists elsewhere.
/// Within a single bucket, content is deduplicated by hash. Returns the file
/// record and whether its `files` row was created here (a brand-new blob
/// already starts with ref_count = 1 for the link that follows).
async fn ensure_blob_in_backend(
    state: &AppState,
    backend_name: &str,
    hash: &str,
    data: &Bytes,
    mime_type: Option<&str>,
) -> AppResult<(File, bool)> {
    if let Some(existing) = FileRepository::find_by_hash(state.db.pool(), hash).await? {
        let objects =
            StorageObjectRepository::find_by_file_id(state.db.pool(), existing.id).await?;
        if objects.iter().any(|o| o.backend == backend_name) {
            // This bucket already has a copy — no-op.
            return Ok((existing, false));
        }
        // Same content exists in another bucket — give this bucket its own copy.
        let storage_key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);
        {
            let storage = state.storage.read().await;
            let backend = storage.get(backend_name).ok_or_else(|| {
                AppError::Internal(format!("storage backend '{backend_name}' not found"))
            })?;
            backend
                .put(&storage_key, data.clone())
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        let storage_obj = CreateStorageObjectData {
            file_id: existing.id,
            backend: backend_name.to_string(),
            storage_path: storage_key,
        };
        StorageObjectRepository::create(state.db.pool(), storage_obj).await?;
        Ok((existing, false))
    } else {
        // Brand-new content — create the files row and this bucket's copy.
        let storage_key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);
        {
            let storage = state.storage.read().await;
            let backend = storage.get(backend_name).ok_or_else(|| {
                AppError::Internal(format!("storage backend '{backend_name}' not found"))
            })?;
            backend
                .put(&storage_key, data.clone())
                .await
                .map_err(|e| AppError::Storage(e.to_string()))?;
        }
        let file_record = FileRecord::new(
            hash.to_string(),
            storage_key.clone(),
            mime_type.map(|s| s.to_string()),
            data.len() as i64,
        );
        let file = FileRepository::create(state.db.pool(), file_record).await?;
        let storage_obj = CreateStorageObjectData {
            file_id: file.id,
            backend: backend_name.to_string(),
            storage_path: storage_key,
        };
        StorageObjectRepository::create(state.db.pool(), storage_obj).await?;
        Ok((file, true))
    }
}

/// Store a blob by its blake3 hash into this bucket's own backend. Used for
/// raw storage-tree entries that carry no owner metadata (they are never
/// linked to a user here, so no ref_count adjustment is made).
async fn store_blob_by_hash(
    state: &AppState,
    backend_name: &str,
    data: Bytes,
    entry_path: &str,
    result: &mut ImportResult,
) {
    let hash = hash_bytes(&data).await;
    match ensure_blob_in_backend(state, backend_name, &hash, &data, None).await {
        Ok(_) => result.files_imported += 1,
        Err(e) => result
            .errors
            .push(format!("failed to store blob for '{entry_path}': {e}")),
    }
}

/// Import both a ZIP (file data) and JSON (index structure) into a bucket.
/// The ZIP may be the raw storage-tree backup (entries are blobs keyed by
/// blake3 shard path, stored content-addressed) or the logical layout
/// `username/path/to/file.ext`. An optional `bucket_name/` prefix from legacy
/// exports is tolerated and stripped, so backups are portable across buckets
/// with different names/paths. The JSON should be the export-index format.
/// Files are stored as content-addressed blobs in this bucket's own backend
/// (each bucket keeps its own physical copy — a restored backup never depends
/// on another bucket), then the JSON structure is used to create folder
/// hierarchies and user_file links in this bucket.
pub async fn import_bucket_combined(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
    mut multipart: Multipart,
) -> AppResult<Json<ImportResult>> {
    auth.require_admin()?;

    // Verify bucket exists
    BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Zip-bomb / DoS limits derived from the configured per-upload limit.
    // Computed before the multipart read so both streams are capped as they arrive.
    let per_entry_limit = (state.config.storage.max_upload_size_mb * 1024 * 1024) as u64;
    let total_limit = MAX_TOTAL_UNCOMPRESSED_MULT * per_entry_limit;
    let upload_limit = MAX_ZIP_UPLOAD_MULT * per_entry_limit;

    // Extract ZIP and JSON from multipart
    let mut zip_data: Option<Bytes> = None;
    let mut json_data: Option<String> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        match field.name() {
            Some("zip") => {
                zip_data = Some(
                    read_field_bounded(&mut field, upload_limit as usize)
                        .await
                        .map_err(|e| AppError::BadRequest(format!("failed to read zip file: {e}")))?,
                );
            }
            Some("index") => {
                let bytes = read_field_bounded(&mut field, upload_limit as usize)
                    .await
                    .map_err(|e| AppError::BadRequest(format!("failed to read index file: {e}")))?;
                json_data = Some(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|_| AppError::BadRequest("index is not valid UTF-8".into()))?,
                );
            }
            _ => {}
        }
    }

    let zip_bytes = zip_data.ok_or_else(|| AppError::BadRequest("no zip field in upload".into()))?;
    let json_text = json_data.ok_or_else(|| AppError::BadRequest("no index field in upload".into()))?;

    if zip_bytes.len() as u64 > upload_limit {
        return Err(AppError::BadRequest(format!(
            "ZIP too large: {} bytes (max: {} MB)",
            zip_bytes.len(),
            upload_limit / (1024 * 1024)
        )));
    }
    if json_text.len() as u64 > upload_limit {
        return Err(AppError::BadRequest(format!(
            "index JSON too large: {} bytes (max: {} MB)",
            json_text.len(),
            upload_limit / (1024 * 1024)
        )));
    }

    // Parse JSON
    let payload: BucketIndexExportDto = serde_json::from_str(&json_text)
        .map_err(|e| AppError::BadRequest(format!("invalid index JSON: {e}")))?;

    // The JSON bucket name is informational; the URL parameter determines
    // the target bucket so backups can be restored across buckets with
    // different names/paths.

    if payload.users.len() > MAX_INDEX_USERS {
        return Err(AppError::BadRequest(format!(
            "index contains too many users: {} (max: {})",
            payload.users.len(),
            MAX_INDEX_USERS
        )));
    }

    // The target bucket is also the storage backend for this import: buckets
    // keep their own physical copies, so every blob lands in this bucket's own
    // storage directory rather than a shared "first available" backend.
    let backend_name = bucket_name.clone();

    let mut result = ImportResult {
        files_imported: 0,
        folders_created: 0,
        errors: Vec::new(),
    };

    // Pre-fetch known bucket names so we can strip any leading bucket prefix
    // from ZIP paths. This lets export ZIPs from *any* bucket be imported
    // into this bucket without manual path rewriting.
    let known_buckets: Vec<String> = match BucketRepository::list(state.db.pool()).await {
        Ok(buckets) => buckets.into_iter().map(|b| b.name).collect(),
        Err(_) => Vec::new(),
    };

    // ── Step 1: Extract all ZIP entries SYNCHRONOUSLY ──
    // ZipFile is not Send, so we must drop it before any .await.
    let mut zip_entries: Vec<ZipEntryData> = Vec::new();
    let mut total_uncompressed: u64 = 0;

    {
        let reader = std::io::Cursor::new(&zip_bytes);
        let mut archive = zip::ZipArchive::new(reader)
            .map_err(|e| AppError::BadRequest(format!("invalid zip file: {e}")))?;

        if archive.len() > MAX_ZIP_ENTRIES {
            return Err(AppError::BadRequest(format!(
                "ZIP contains too many entries: {} (max: {})",
                archive.len(),
                MAX_ZIP_ENTRIES
            )));
        }

        for i in 0..archive.len() {
            // ── Synchronous block: ZipFile must be dropped before .await ──
            let extracted = {
                let mut entry = match archive.by_index(i) {
                    Ok(e) => e,
                    Err(e) => {
                        result.errors.push(format!("zip entry #{i}: {e}"));
                        continue;
                    }
                };

                let entry_path = entry.name().to_string();
                if entry_path.ends_with('/') || entry.is_dir() {
                    continue;
                }

                // Skip symlink entries — never materialize links from an archive.
                if entry_is_symlink(&entry) {
                    result
                        .errors
                        .push(format!("skipped '{entry_path}': symlink entries are not imported"));
                    continue;
                }

                // Normalise path: strip optional leading bucket name prefix so
                // export ZIPs from *any* bucket can be imported here.
                let normalized = entry_path.replace('\\', "/");
                let trimmed = normalized.trim_start_matches('/');

                // If the first segment matches any known bucket, strip it
                let trimmed = if let Some(first_slash) = trimmed.find('/') {
                    let first_segment = &trimmed[..first_slash];
                    if known_buckets.iter().any(|b| b == first_segment) {
                        &trimmed[first_slash + 1..]
                    } else {
                        trimmed
                    }
                } else {
                    trimmed
                };

                let parts: Vec<&str> = trimmed.split('/').collect();
                if parts.len() < 2 {
                    result.errors.push(format!(
                        "skipped '{}': path must be username/file or bucket/username/file", entry_path
                    ));
                    continue;
                }

                let username = parts[0].to_string();
                let file_name = (*parts.last().unwrap_or(&"")).to_string();
                let _folder_segments: Vec<String> = if parts.len() > 2 {
                    parts[1..parts.len() - 1].iter().map(|s| s.to_string()).collect()
                } else {
                    Vec::new()
                };

                if username.is_empty() || file_name.is_empty() {
                    result.errors.push(format!("skipped '{}': invalid path segments", entry_path));
                    continue;
                }

                let file_data = match read_entry_bounded(&mut entry, per_entry_limit) {
                    Ok(d) => d,
                    Err(e) => {
                        result.errors.push(format!("skipped '{entry_path}': {e}"));
                        continue;
                    }
                };

                Some(ZipEntryData {
                    entry_path,
                    data: Bytes::from(file_data),
                })
            }; // ZipFile dropped here — safe to .await now

            if let Some(entry) = extracted {
                total_uncompressed += entry.data.len() as u64;
                if total_uncompressed > total_limit {
                    result.errors.push(format!(
                        "aborted: total decompressed size exceeds limit of {} bytes",
                        total_limit
                    ));
                    break;
                }
                zip_entries.push(entry);
            }
        }
    }
    // archive dropped here

    // ── Step 2: Store each ZIP entry as a content-addressed blob in THIS
    // bucket's own backend. Buckets keep independent physical copies, so a blob
    // is stored here even when the same content exists elsewhere; within this
    // bucket, content is deduplicated. `created_this_import` tracks brand-new
    // blobs — they already carry ref_count = 1 for the link Step 3 will create,
    // so Step 3 only bumps ref_count for links to pre-existing content.
    let mut created_this_import: HashSet<Uuid> = HashSet::new();
    for entry in &zip_entries {
        let hash = hash_bytes(&entry.data).await;
        match ensure_blob_in_backend(&state, &backend_name, &hash, &entry.data, None).await {
            Ok((file, created)) => {
                if created {
                    created_this_import.insert(file.id);
                }
            }
            Err(e) => {
                result.errors.push(format!(
                    "failed to store blob for '{}': {e}", entry.entry_path
                ));
            }
        }
    }

    // ── Step 3: Process JSON — create folders + user_file links ──
    for user_dto in &payload.users {
        let user = match UserRepository::find_by_username(state.db.pool(), &user_dto.username).await {
            Ok(Some(u)) => u,
            Ok(None) => {
                result.errors.push(format!("skipped user '{}': not found", user_dto.username));
                continue;
            }
            Err(e) => {
                result.errors.push(format!("skipped user '{}': db error: {e}", user_dto.username));
                continue;
            }
        };

        // Create folders from JSON
        for folder_dto in &user_dto.folders {
            let segments: Vec<&str> = folder_dto.full_path.split('/').filter(|s| !s.is_empty()).collect();
            if segments.is_empty() { continue; }
            match resolve_or_create_folders(state.db.pool(), user.id, &bucket_name, &segments).await {
                Ok((_, created)) => { result.folders_created += created; }
                Err(e) => {
                    result.errors.push(format!("user '{}': folder '{}': {e}", user_dto.username, folder_dto.full_path));
                }
            }
        }

        // Create user_file entries from JSON
        for file_dto in &user_dto.files {
            // Resolve folder path
            let folder_id = if let Some(ref fp) = file_dto.folder {
                let segments: Vec<&str> = fp.split('/').filter(|s| !s.is_empty()).collect();
                if segments.is_empty() {
                    None
                } else {
                    match resolve_or_create_folders(state.db.pool(), user.id, &bucket_name, &segments).await {
                        Ok((fid, created)) => {
                            result.folders_created += created;
                            Some(fid)
                        }
                        Err(e) => {
                            result.errors.push(format!(
                                "user '{}': file '{}': folder error: {e}",
                                user_dto.username, file_dto.name
                            ));
                            continue;
                        }
                    }
                }
            } else {
                None
            };

            // Look up physical blob by hash (should exist now from Step 2)
            let existing_file = match FileRepository::find_by_hash(state.db.pool(), &file_dto.hash).await {
                Ok(Some(f)) => f,
                Ok(None) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': blob with hash '{}' not found in ZIP or storage.",
                        user_dto.username, file_dto.name, file_dto.hash
                    ));
                    continue;
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': db error: {e}",
                        user_dto.username, file_dto.name
                    ));
                    continue;
                }
            };

            // Check if this user already has this file linked by the same name in
            // THIS bucket (a link in another bucket does not block a restore).
            let already_linked = match UserFileRepository::find_active_in_bucket_by_user_file_and_name(
                state.db.pool(), user.id, existing_file.id, &file_dto.name, &bucket_name,
            ).await {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': check error: {e}",
                        user_dto.username, file_dto.name
                    ));
                    continue;
                }
            };

            if already_linked {
                // Already linked with same name — skip (idempotent)
                continue;
            }

            // Check for soft-deleted entry with same triple — restore
            // `created_row_id` is set only when we insert a brand-new row, so a
            // later quota failure can roll that row back without a ghost.
            let mut created_row_id = None;
            let restored_id = match UserFileRepository::find_deleted_by_user_file_and_name(
                state.db.pool(), user.id, existing_file.id, &file_dto.name,
            ).await {
                Ok(Some(deleted_uf)) => {
                    let _ = UserFileRepository::restore(state.db.pool(), deleted_uf.id).await;
                    // Update bucket/folder if changed
                    if deleted_uf.bucket_name.as_deref() != Some(&bucket_name)
                        || deleted_uf.folder_id != folder_id
                    {
                        let _ = UserFileRepository::update_bucket_and_folder(
                            state.db.pool(), deleted_uf.id,
                            Some(bucket_name.clone()), folder_id,
                        ).await;
                    }
                    Some(deleted_uf.id)
                }
                Ok(None) => {
                    // The name comes from client JSON — apply the same component
                    // rules used everywhere else so hostile names can never reach
                    // the database.
                    if let Err(e) = validate_component_name(&file_dto.name) {
                        result.errors.push(format!(
                            "user '{}': file '{}': invalid name: {e}",
                            user_dto.username, file_dto.name
                        ));
                        continue;
                    }
                    let user_file_record = UserFileRecord {
                        id: Uuid::new_v4(),
                        user_id: user.id,
                        file_id: existing_file.id,
                        original_name: file_dto.name.clone(),
                        mime_type: file_dto.mime_type.clone(),
                        bucket_name: Some(bucket_name.clone()),
                        folder_id,
                    };
                    let new_row_id = user_file_record.id;
                    match UserFileRepository::create(state.db.pool(), user_file_record).await {
                        Ok(_) => {
                            created_row_id = Some(new_row_id);
                            None
                        }
                        Err(e) => {
                            result.errors.push(format!(
                                "user '{}': file '{}': insert error: {e}",
                                user_dto.username, file_dto.name
                            ));
                            None
                        }
                    }
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': db error: {e}",
                        user_dto.username, file_dto.name
                    ));
                    None
                }
            };

            // A row was either restored or freshly created — account for it.
            if restored_id.is_some() || created_row_id.is_some() {
                // A link to pre-existing content bumps the physical file's
                // ref_count; brand-new blobs (created in Step 2 of this import)
                // already start at ref_count = 1, so they must not bump again.
                let mut bumped_ref = false;
                if !created_this_import.contains(&existing_file.id) {
                    if let Err(e) = FileRepository::update_ref_count(
                        state.db.pool(), existing_file.id, 1,
                    ).await {
                        result.errors.push(format!(
                            "user '{}': file '{}': refcount error: {e}",
                            user_dto.username, file_dto.name
                        ));
                        if let Some(row_id) = created_row_id {
                            let _ = UserFileRepository::hard_delete_by_id(
                                state.db.pool(), row_id,
                            )
                            .await;
                        }
                        continue;
                    }
                    bumped_ref = true;
                }
                // Charge the user's storage with the REAL blob size — the
                // size in the JSON is client-controlled and must not drive
                // storage accounting. Atomic so concurrent imports cannot
                // lose updates. Enforce the quota: on a rejected reservation,
                // roll back a freshly created row (and the ref_count bump
                // taken for it) so no ghost reference is left.
                match UserRepository::charge_storage(
                    state.db.pool(), user.id, existing_file.size,
                ).await {
                    Ok(true) => {
                        result.files_imported += 1;
                    }
                    Ok(false) => {
                        if let Some(row_id) = created_row_id {
                            let _ = UserFileRepository::hard_delete_by_id(
                                state.db.pool(), row_id,
                            )
                            .await;
                            if bumped_ref {
                                let _ = FileRepository::update_ref_count(
                                    state.db.pool(), existing_file.id, -1,
                                )
                                .await;
                            }
                        }
                        result.errors.push(format!(
                            "user '{}': file '{}': storage quota exceeded",
                            user_dto.username, file_dto.name
                        ));
                    }
                    Err(e) => {
                        if let Some(row_id) = created_row_id {
                            let _ = UserFileRepository::hard_delete_by_id(
                                state.db.pool(), row_id,
                            )
                            .await;
                            if bumped_ref {
                                let _ = FileRepository::update_ref_count(
                                    state.db.pool(), existing_file.id, -1,
                                )
                                .await;
                            }
                        }
                        result.errors.push(format!(
                            "user '{}': file '{}': quota charge error: {e}",
                            user_dto.username, file_dto.name
                        ));
                    }
                }
            }
        }
    }

    info!(
        "admin {} combined-import into bucket '{}': {} files, {} folders, {} errors",
        auth.username, bucket_name, result.files_imported, result.folders_created, result.errors.len()
    );

    Ok(Json(result))
}

// ─── Helpers ──────────────────────────────────────────────────

/// Resolve or create a chain of folder segments for a user in a bucket.
/// Returns (final_folder_id, number_of_folders_created).
async fn resolve_or_create_folders(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    bucket_name: &str,
    segments: &[&str],
) -> AppResult<(Uuid, usize)> {
    let mut created = 0;
    let mut parent_id: Option<Uuid> = None;

    for segment in segments {
        // Strict name rules so imported folders behave like created ones.
        if let Err(e) = validate_component_name(segment) {
            return Err(AppError::BadRequest(format!(
                "invalid folder name '{segment}': {e}"
            )));
        }

        // Check if folder already exists at this level
        let existing = FolderRepository::list_children(pool, user_id, bucket_name, parent_id)
            .await?
            .into_iter()
            .find(|f| f.name == *segment);

        match existing {
            Some(f) => {
                parent_id = Some(f.id);
            }
            None => {
                let record = FolderRecord::new(user_id, bucket_name.to_string(), segment.to_string(), parent_id);
                let folder = FolderRepository::create(pool, record).await?;
                parent_id = Some(folder.id);
                created += 1;
            }
        }
    }

    let final_id = parent_id.ok_or_else(|| AppError::Internal("no folders resolved".to_string()))?;
    Ok((final_id, created))
}

/// Hash, ensure this bucket's own physical copy, and create the user_file
/// entry for a single file.
async fn import_file_data(
    state: &AppState,
    backend_name: &str,
    bucket_name: &str,
    user_id: Uuid,
    file_name: &str,
    mime_type: Option<&str>,
    folder_id: Option<Uuid>,
    data: Bytes,
) -> AppResult<String> {
    validate_component_name(file_name)
        .map_err(|e| AppError::BadRequest(format!("invalid file name '{file_name}': {e}")))?;

    let hash = hash_bytes(&data).await;

    // Reserve the storage quota up front, atomically, BEFORE any blob, user_file
    // row, or ref_count bump is created. A quota failure therefore aborts with
    // no committed state and no accounting drift. If a later step fails we
    // release the reservation (and undo any ref_count bump) below.
    if !UserRepository::charge_storage(state.db.pool(), user_id, data.len() as i64).await? {
        return Err(AppError::BadRequest(format!(
            "storage quota exceeded for user {user_id}"
        )));
    }

    // Track side effects so a downstream failure can roll them back.
    // `bumped_file_id` is set when we incremented a PRE-EXISTING blob's
    // ref_count; a fresh `files` row starts at ref_count = 1 and must not bump.
    let mut bumped_file_id: Option<Uuid> = None;

    let outcome = (async {
        // Ensure this bucket holds its own physical copy of the blob — an
        // import into a bucket must not depend on another bucket's copy.
        let (file, created_file_row) =
            ensure_blob_in_backend(state, backend_name, &hash, &data, mime_type).await?;
        let file_id = file.id;

        // A link to pre-existing content bumps the physical file's ref_count;
        // a brand-new files row already carries ref_count = 1 for this link.
        if !created_file_row {
            FileRepository::update_ref_count(state.db.pool(), file_id, 1).await?;
            bumped_file_id = Some(file_id);
        }

        // Create user_file entry — handle duplicates.
        // Uniqueness is now per (user_id, file_id, original_name, bucket_name):
        // a restored backup may link the same file+name into a different
        // bucket, so only a matching row IN THIS BUCKET is an idempotent no-op.
        if let Some(active_uf) = UserFileRepository::find_by_user_and_file(
            state.db.pool(), user_id, file_id,
        ).await? {
            if active_uf.original_name == file_name
                && active_uf.bucket_name.as_deref() == Some(bucket_name)
            {
                // Already linked with the same name in this bucket — the
                // import is a no-op. Roll back the quota reservation and
                // ref-count bump taken above so they do not leak without a new
                // user_file row.
                let _ = UserRepository::release_storage(
                    state.db.pool(), user_id, data.len() as i64,
                )
                .await;
                let _ = FileRepository::update_ref_count(
                    state.db.pool(), file_id, -1,
                )
                .await;
                return Ok(file_id);
            }
        }

        // Check for a soft-deleted entry with the same triple — restore it
        if let Some(deleted_uf) = UserFileRepository::find_deleted_by_user_file_and_name(
            state.db.pool(), user_id, file_id, file_name,
        ).await? {
            UserFileRepository::restore(state.db.pool(), deleted_uf.id).await?;
            // Update bucket/folder if changed
            if deleted_uf.bucket_name.as_deref() != Some(bucket_name)
                || deleted_uf.folder_id != folder_id
            {
                UserFileRepository::update_bucket_and_folder(
                    state.db.pool(), deleted_uf.id,
                    Some(bucket_name.to_string()), folder_id,
                ).await?;
            }
        } else {
            // No existing entry — create a new one
            let user_file_record = UserFileRecord {
                id: Uuid::new_v4(),
                user_id,
                file_id,
                original_name: file_name.to_string(),
                mime_type: mime_type.map(|s| s.to_string()),
                bucket_name: Some(bucket_name.to_string()),
                folder_id,
            };
            UserFileRepository::create(state.db.pool(), user_file_record).await?;
        }

        Ok(file_id)
    })
    .await;

    match outcome {
        Ok(_file_id) => Ok(hash),
        Err(e) => {
            // Roll back the quota reservation taken above.
            let _ = UserRepository::release_storage(
                state.db.pool(), user_id, data.len() as i64,
            )
            .await;
            // Undo a ref_count bump on pre-existing content so a failed import
            // does not leave the physical blob with an extra reference but no
            // new user_files row. A freshly created `files` row is left as an
            // unreferenced blob, which the orphan-cleanup path already handles.
            if let Some(fid) = bumped_file_id {
                let _ = FileRepository::update_ref_count(state.db.pool(), fid, -1).await;
            }
            Err(e)
        }
    }
}
