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

    // Pick a storage backend (first available)
    let backend_name = {
        let storage = state.storage.read().await;
        let backends = storage.list_backends();
        if backends.is_empty() {
            return Err(AppError::Internal("no storage backends configured".into()));
        }
        backends[0].clone()
    };

    let mut result = ImportResult {
        files_imported: 0,
        folders_created: 0,
        errors: Vec::new(),
    };

    let mut total_uncompressed: u64 = 0;

    // Process each ZIP entry
    for i in 0..archive.len() {
        // ── Extract metadata + data from the entry synchronously ──
        // (ZipFile is not Send, so we must drop it before any .await)
        let (entry_path, username, file_name, folder_segments, data_or_err) = {
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

            // Parse path: username / folder_path... / file_name
            let normalized = entry_path.replace('\\', "/");
            let trimmed = normalized.trim_start_matches('/');
            let parts: Vec<&str> = trimmed.split('/').collect();

            if parts.len() < 2 {
                result
                    .errors
                    .push(format!("skipped '{}': path must be username/file", entry_path));
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

            (entry_path, username, file_name, folder_segments, read_result)
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

    // Pick a storage backend
    let backend_name = {
        let storage = state.storage.read().await;
        let backends = storage.list_backends();
        if backends.is_empty() {
            return Err(AppError::Internal("no storage backends configured".into()));
        }
        backends[0].clone()
    };

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

    // Verify bucket name in JSON matches URL
    if payload.bucket != bucket_name {
        return Err(AppError::BadRequest(format!(
            "JSON bucket name '{}' does not match URL bucket name '{}'",
            payload.bucket, bucket_name
        )));
    }

    // Verify bucket exists
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

            // Skip if this user already has this file linked
            match UserFileRepository::find_by_user_and_file(
                state.db.pool(),
                user.id,
                existing_file.id,
            )
            .await
            {
                Ok(Some(_)) => {
                    // Already exists — skip silently (idempotent)
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

            match UserFileRepository::create(state.db.pool(), user_file_record).await {
                Ok(_) => {
                    // Charge the user's storage with the REAL blob size — the
                    // size in the JSON is client-controlled and must not drive
                    // storage accounting. Atomic so concurrent imports cannot
                    // lose updates.
                    let _ = UserRepository::charge_storage(
                        state.db.pool(),
                        user.id,
                        existing_file.size,
                    )
                    .await;
                    result.files_imported += 1;
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

/// Import both a ZIP (file data) and JSON (index structure) into a bucket.
/// The ZIP should contain files organised as `bucket_name/username/path/to/file.ext`
/// (same as export-zip format). The JSON should be the export-index format.
/// Files are stored as content-addressed blobs first, then the JSON structure
/// is used to create folder hierarchies and user_file links.
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

    // Validate bucket name
    if payload.bucket != bucket_name {
        return Err(AppError::BadRequest(format!(
            "JSON bucket name '{}' does not match URL bucket name '{}'",
            payload.bucket, bucket_name
        )));
    }

    if payload.users.len() > MAX_INDEX_USERS {
        return Err(AppError::BadRequest(format!(
            "index contains too many users: {} (max: {})",
            payload.users.len(),
            MAX_INDEX_USERS
        )));
    }

    // Pick a storage backend
    let backend_name = {
        let storage = state.storage.read().await;
        let backends = storage.list_backends();
        if backends.is_empty() {
            return Err(AppError::Internal("no storage backends configured".into()));
        }
        backends[0].clone()
    };

    let mut result = ImportResult {
        files_imported: 0,
        folders_created: 0,
        errors: Vec::new(),
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

                // Normalise path: strip optional bucket_name prefix
                let normalized = entry_path.replace('\\', "/");
                let trimmed = normalized.trim_start_matches('/');

                // If the first segment matches the bucket name, strip it
                let trimmed = if let Some(rest) = trimmed.strip_prefix(&format!("{}/", bucket_name)) {
                    rest
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

    // ── Step 2: Store each ZIP entry as a content-addressed blob ──
    for entry in &zip_entries {
        let hash = hash_bytes(&entry.data).await;

        let existing_file = FileRepository::find_by_hash(state.db.pool(), &hash).await;
        match existing_file {
            Ok(Some(f)) => {
                // Blob already exists — increment ref count
                let _ = FileRepository::update_ref_count(state.db.pool(), f.id, 1).await;
            }
            Ok(None) => {
                // New blob — store it
                let storage_key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);
                {
                    let storage = state.storage.read().await;
                    if let Some(backend) = storage.get(&backend_name) {
                        if let Err(e) = backend.put(&storage_key, entry.data.clone()).await {
                            result.errors.push(format!(
                                "failed to store blob for '{}': {e}", entry.entry_path
                            ));
                            continue;
                        }
                    } else {
                        result.errors.push(format!(
                            "storage backend '{}' not found", backend_name
                        ));
                        continue;
                    }
                }
                let file_record = FileRecord::new(hash.clone(), storage_key.clone(), None, entry.data.len() as i64);
                match FileRepository::create(state.db.pool(), file_record).await {
                    Ok(file) => {
                        let storage_obj = CreateStorageObjectData {
                            file_id: file.id,
                            backend: backend_name.clone(),
                            storage_path: storage_key,
                        };
                        let _ = StorageObjectRepository::create(state.db.pool(), storage_obj).await;
                    }
                    Err(e) => {
                        result.errors.push(format!(
                            "failed to create file record for '{}': {e}", entry.entry_path
                        ));
                        continue;
                    }
                }
            }
            Err(e) => {
                result.errors.push(format!(
                    "db error storing '{}': {e}", entry.entry_path
                ));
                continue;
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

            // Check if this user already has this file linked (by user_id + file_id)
            let already_linked = match UserFileRepository::find_by_user_and_file(
                state.db.pool(), user.id, existing_file.id,
            ).await {
                Ok(Some(uf)) => uf.original_name == file_dto.name,
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
            let created_new = match UserFileRepository::find_deleted_by_user_file_and_name(
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
                    false
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
                    match UserFileRepository::create(state.db.pool(), user_file_record).await {
                        Ok(_) => true,
                        Err(e) => {
                            result.errors.push(format!(
                                "user '{}': file '{}': insert error: {e}",
                                user_dto.username, file_dto.name
                            ));
                            false
                        }
                    }
                }
                Err(e) => {
                    result.errors.push(format!(
                        "user '{}': file '{}': db error: {e}",
                        user_dto.username, file_dto.name
                    ));
                    false
                }
            };

            if created_new {
                // Charge the user's storage with the REAL blob size — the
                // size in the JSON is client-controlled and must not drive
                // storage accounting. Atomic so concurrent imports cannot
                // lose updates.
                let _ = UserRepository::charge_storage(
                    state.db.pool(), user.id, existing_file.size,
                ).await;
                result.files_imported += 1;
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

/// Hash, deduplicate, store, and create user_file entry for a single file.
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

    // Check for deduplication
    let existing_file = FileRepository::find_by_hash(state.db.pool(), &hash).await?;

    let file_id = if let Some(ref existing) = existing_file {
        // Blob already exists — increment ref count
        FileRepository::update_ref_count(state.db.pool(), existing.id, 1).await?;
        existing.id
    } else {
        // New blob — store it
        let storage_key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);

        {
            let storage = state.storage.read().await;
            let backend = storage
                .get(backend_name)
                .ok_or_else(|| AppError::Internal(format!("storage backend '{}' not found", backend_name)))?;
            backend
                .put(&storage_key, data.clone())
                .await
                .map_err(|e| AppError::Internal(format!("failed to store blob: {e}")))?;
        }

        let file_record = FileRecord::new(hash.clone(), storage_key.clone(), mime_type.map(|s| s.to_string()), data.len() as i64);
        let file = FileRepository::create(state.db.pool(), file_record).await?;

        // Create storage object mapping
        let storage_obj = CreateStorageObjectData {
            file_id: file.id,
            backend: backend_name.to_string(),
            storage_path: storage_key,
        };
        StorageObjectRepository::create(state.db.pool(), storage_obj).await?;

        file.id
    };

    // Create user_file entry — handle duplicates
    // Check for an existing active user_file with the same (user_id, file_id, original_name)
    if let Some(active_uf) = UserFileRepository::find_by_user_and_file(
        state.db.pool(), user_id, file_id,
    ).await? {
        // A user_file for this (user_id, file_id) already exists.
        // The UNIQUE constraint is (user_id, file_id, original_name), so if original_name
        // also matches, skip entirely (idempotent). Otherwise try to insert.
        if active_uf.original_name == file_name {
            // Already linked with same original name — skip
            return Ok(hash);
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

    // Charge user storage with the REAL blob size — the size in the JSON is
    // client-controlled and must not drive storage accounting. Atomic so
    // concurrent imports cannot lose updates.
    if !UserRepository::charge_storage(state.db.pool(), user_id, data.len() as i64).await? {
        return Err(AppError::BadRequest(format!(
            "storage quota exceeded for user {user_id}"
        )));
    }

    Ok(hash)
}
