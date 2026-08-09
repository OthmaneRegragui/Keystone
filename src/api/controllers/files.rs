use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::models::File;
use crate::db::rows::{CreateStorageObjectData, FileRecord, FolderRecord, UserFileRecord};
use crate::db::repos::{
    buckets::AccessibleBucket, BucketRepository, FileRepository, FolderRepository,
    StorageObjectRepository, UserFileRepository, UserRepository,
};
use crate::utils::hashing::blake3::hash_bytes;
use crate::utils::names::validate_component_name;
use serde::Deserialize;
use uuid::Uuid;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListFilesParams {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub search: Option<String>,
    pub bucket: Option<String>,
    pub folder_id: Option<String>,
}

// ── Request-hardening limits ──

/// Hard cap on the number of multipart fields accepted by `upload`.
const MAX_MULTIPART_FIELDS: usize = 16;
/// Hard cap for small text fields (bucket name, folder_id, overwrite flag).
const MAX_TEXT_FIELD_BYTES: usize = 4096;
/// Cap on the number of ids accepted by the batch endpoints.
const MAX_BATCH_IDS: usize = 500;
/// Cap on the free-text `search` parameter of list endpoints.
const MAX_SEARCH_LEN: usize = 256;

/// Read a multipart field in bounded chunks, rejecting anything larger than
/// `max_bytes` without ever buffering more than that much. This prevents a
/// client from exhausting server memory with a giant field *before* the
/// configured upload-size limit is applied.
async fn read_field_bytes(
    field: &mut axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> AppResult<bytes::Bytes> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                if buf.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(AppError::BadRequest(format!(
                        "field exceeds maximum allowed size ({} bytes)",
                        max_bytes
                    )));
                }
                buf.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(AppError::BadRequest(format!("failed to read field: {e}"))),
        }
    }
    Ok(bytes::Bytes::from(buf))
}

/// Read a small text field with a hard byte cap and UTF-8 validation.
async fn read_field_text(field: &mut axum::extract::multipart::Field<'_>) -> AppResult<String> {
    let data = read_field_bytes(field, MAX_TEXT_FIELD_BYTES).await?;
    String::from_utf8(data.to_vec())
        .map_err(|_| AppError::BadRequest("field is not valid UTF-8".into()))
}

/// Keep only header-safe, non-empty MIME strings (no CR/LF or other control
/// characters, capped length) so a stored value can never poison a response
/// header when the file is downloaded.
fn sanitize_mime_type(mime: String) -> Option<String> {
    let trimmed = mime.trim().to_string();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(trimmed)
}

/// MIME types that are safe to render inline. Stored files are attacker-
/// controlled, so anything that can execute active content (`text/html`,
/// `image/svg+xml`, `text/javascript`, XML, …) is excluded: serving those
/// inline in the application's origin would let a crafted file run scripts
/// there (stored XSS, amplified by the UI's `unsafe-inline` CSP). Everything
/// else is forced to `Content-Disposition: attachment` so the browser
/// downloads it instead of rendering it.
fn is_inline_safe(mime: &str) -> bool {
    let mime = mime.to_ascii_lowercase();
    matches!(
        mime.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/avif"
            | "image/x-icon"
            | "video/mp4"
            | "video/webm"
            | "video/quicktime"
            | "video/x-matroska"
            | "video/x-msvideo"
            | "video/mpeg"
            | "video/x-ms-wmv"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/ogg"
            | "audio/mp4"
            | "application/pdf"
            | "text/plain"
    )
}

/// Guess a MIME type from a filename extension. Used as a fallback when the
/// client does not provide a useful Content-Type on the multipart file part
/// (curl sends `application/octet-stream` for `-F`, and some tools send none).
/// Only covers common types so the frontend can classify/preview them.
fn guess_mime_type(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "mpg" | "mpeg" => "video/mpeg",
        "wmv" => "video/x-ms-wmv",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "m4a" => "audio/mp4",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppt" => "application/vnd.ms-powerpoint",
        _ => return None,
    };
    Some(mime.to_string())
}

/// Resolve the MIME type to store for an uploaded file: use the client-provided
/// Content-Type unless it is missing or generic, then fall back to a filename
/// extension guess.
fn resolve_mime_type(client_mime: Option<String>, filename: Option<&str>) -> Option<String> {
    let generic = client_mime
        .as_deref()
        .map(|m| {
            m.eq_ignore_ascii_case("application/octet-stream")
                || m.eq_ignore_ascii_case("application/binary")
        })
        .unwrap_or(false);
    if !generic {
        return client_mime;
    }
    filename.and_then(guess_mime_type).or(client_mime)
}

/// Fetch the buckets accessible to a user, failing closed on DB errors.
async fn accessible_buckets(state: &AppState, user_id: Uuid) -> AppResult<Vec<AccessibleBucket>> {
    Ok(BucketRepository::list_accessible_to_user(
        state.db.pool(),
        &user_id.to_string(),
    )
    .await?)
}

fn can_upload_on(buckets: &[AccessibleBucket], name: &str) -> bool {
    buckets.iter().any(|b| b.name == name && b.can_upload)
}

fn can_download_on(buckets: &[AccessibleBucket], name: &str) -> bool {
    buckets.iter().any(|b| b.name == name && b.can_download)
}

/// Check a bucket's per-user storage limit (0 = unlimited). `bucket_used` is
/// the sum of active file sizes the user currently has in that bucket.
fn bucket_limit_allows(bucket: &AccessibleBucket, bucket_used: i64, additional: i64) -> bool {
    bucket.user_storage_limit <= 0
        || bucket_used.saturating_add(additional) <= bucket.user_storage_limit
}

/// Upload a file. Content-addressed deduplication is applied:
/// - If the same content (blake3 hash) already exists on the backend, only a new
///   user_files entry is created (no duplicate blob stored).
/// - Each user gets their own original_name and mime_type for the same content.
pub async fn upload(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    mut multipart: Multipart,
) -> AppResult<Json<UploadResponse>> {
    auth_user.require_scope("files:write")?;

    // Enforce the configured upload size limit while streaming the body — the
    // file payload is never buffered beyond this bound, so a client cannot
    // exhaust server memory before the limit is applied.
    let max_bytes = (state.config.storage.max_upload_size_mb * 1024 * 1024) as usize;

    let mut original_name: Option<String> = None;
    let mut mime_type: Option<String> = None;
    let mut data: Option<bytes::Bytes> = None;
    let mut bucket_name: Option<String> = None;
    let mut folder_id: Option<String> = None;
    let mut overwrite = false;
    let mut field_count = 0usize;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("failed to read multipart: {e}")))?
    {
        field_count += 1;
        if field_count > MAX_MULTIPART_FIELDS {
            return Err(AppError::BadRequest(format!(
                "too many multipart fields (maximum {MAX_MULTIPART_FIELDS})"
            )));
        }

        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                if data.is_some() {
                    return Err(AppError::BadRequest(
                        "duplicate 'file' field in multipart upload".into(),
                    ));
                }
                original_name = field.file_name().map(|s| s.to_string());
                mime_type = resolve_mime_type(
                    field.content_type().and_then(|s| sanitize_mime_type(s.to_string())),
                    field.file_name(),
                );
                data = Some(read_field_bytes(&mut field, max_bytes).await?);
            }
            "bucket" => {
                let val = read_field_text(&mut field).await?;
                if !val.is_empty() {
                    bucket_name = Some(val);
                }
            }
            "folder_id" => {
                let val = read_field_text(&mut field).await?;
                if !val.is_empty() {
                    folder_id = Some(val);
                }
            }
            "overwrite" => {
                let val = read_field_text(&mut field).await?;
                if val == "true" {
                    overwrite = true;
                }
            }
            _ => {} // unknown fields are ignored for forward compatibility
        }
    }

    let original_name = original_name
        .ok_or_else(|| AppError::BadRequest("missing filename".into()))?;
    let original_name = original_name.trim().to_string();
    validate_component_name(&original_name)
        .map_err(|e| AppError::BadRequest(format!("invalid file name: {e}")))?;
    let data = data
        .ok_or_else(|| AppError::BadRequest("no file data provided".into()))?;

    if data.is_empty() {
        return Err(AppError::BadRequest("empty file".into()));
    }

    let user = UserRepository::find_by_id(state.db.pool(), auth_user.user_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    if !user.has_storage_available(data.len() as i64) {
        return Err(AppError::BadRequest("storage quota exceeded".into()));
    }

    // Resolve the target bucket. Fails closed (Forbidden) when the user has no
    // access or upload permission for it.
    let accessible = accessible_buckets(&state, auth_user.user_id).await?;
    let backend_name = if let Some(ref bucket) = bucket_name {
        match accessible.iter().find(|b| &b.name == bucket) {
            Some(b) if b.can_upload => bucket.clone(),
            Some(_) => return Err(AppError::Forbidden(format!(
                "upload not permitted for bucket '{}'",
                bucket
            ))),
            None => return Err(AppError::Forbidden(format!(
                "bucket '{}' not accessible",
                bucket
            ))),
        }
    } else {
        // No bucket specified — use the user's first accessible bucket
        match accessible.iter().find(|b| b.can_upload) {
            Some(b) => b.name.clone(),
            None => return Err(AppError::Forbidden(
                "no accessible bucket with upload permission".into(),
            )),
        }
    };

    let backend = {
        let storage = state.storage.read().await;
        storage
            .get(&backend_name)
            .ok_or_else(|| AppError::Internal(format!("storage backend '{}' not found", backend_name)))?
    };

    let hash = hash_bytes(&data).await;

    // Check if a physical file with this hash already exists (deduplication).
    // NOTE: we do NOT increment ref_count here — that only happens after the
    // duplicate check below, when a new user_files entry will actually be
    // created (an overwrite must not bump the physical file's ref_count).
    let existing_physical = FileRepository::find_by_hash(state.db.pool(), &hash).await?;

    let file = if let Some(ref existing) = existing_physical {
        existing.clone()
    } else {
        // New unique content - store blob and create file record
        let storage_key = format!("{}/{}/{}", &hash[..2], &hash[2..4], hash);
        backend
            .put(&storage_key, data.clone())
            .await
            .map_err(|e| AppError::Storage(e.to_string()))?;

        let record = FileRecord::new(hash, storage_key.clone(), mime_type.clone(), data.len() as i64);
        let file = FileRepository::create(state.db.pool(), record).await?;

        let storage_data = CreateStorageObjectData {
            file_id: file.id,
            backend: backend_name.clone(),
            storage_path: storage_key,
        };
        StorageObjectRepository::create(state.db.pool(), storage_data).await?;
        file
    };

    // Validate folder_id if provided: it must belong to the user and to the
    // bucket the file is actually being stored in (backend_name), so a file can
    // never end up "inside" a folder that belongs to a different bucket.
    let resolved_folder_id = if let Some(ref fid) = folder_id {
        let uuid = Uuid::parse_str(fid)
            .map_err(|_| AppError::BadRequest("invalid folder_id".into()))?;
        let folder = FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, uuid)
            .await?
            .ok_or_else(|| AppError::NotFound("folder not found".into()))?;
        if folder.bucket_name != backend_name {
            return Err(AppError::BadRequest(format!(
                "folder '{}' does not belong to bucket '{}'",
                folder.name, backend_name
            )));
        }
        Some(uuid)
    } else {
        None
    };

    // Pre-insert duplicate check: if the user already has an ACTIVE user_files
    // row for (user_id, file_id, original_name), the unique index
    // `idx_user_files_user_file` would reject a second insert. Surface a clean
    // 409 instead of a raw 500 — unless the client explicitly requests an
    // overwrite, in which case we reuse the existing row.
    if let Some(existing_uf) = UserFileRepository::find_active_by_user_file_and_name(
        state.db.pool(),
        auth_user.user_id,
        file.id,
        &original_name,
    )
    .await?
    {
        if !overwrite {
            return Err(AppError::FileAlreadyExists(format!(
                "a file named '{original_name}' already exists in this location"
            )));
        }

        // Overwrite: keep the same row, just move it to the new bucket/folder.
        // No new user_files row, no storage_used bump, no ref_count bump.
        // If no bucket was specified, the row keeps its current bucket rather
        // than being cleared to NULL.
        let target_bucket = bucket_name.clone().or(existing_uf.bucket_name.clone());
        UserFileRepository::update_bucket_and_folder(
            state.db.pool(),
            existing_uf.id,
            target_bucket,
            resolved_folder_id,
        )
        .await?;
        if let Some(ref mime) = mime_type {
            if existing_uf.mime_type.as_deref() != Some(mime.as_str()) {
                UserFileRepository::update_mime_type(state.db.pool(), existing_uf.id, mime).await?;
            }
        }
        let user_file = UserFileRepository::find_by_id(state.db.pool(), existing_uf.id)
            .await?
            .ok_or_else(|| AppError::Internal("user_file not found after update".to_string()))?;

        return Ok(Json(UploadResponse {
            file: file_dto_from_user_file(&user_file, &file),
            duplicate: true,
        }));
    }

    // A new user_files reference will be created below (fresh row or restore of
    // a soft-deleted one), so enforce the bucket's per-user storage limit now.
    // `user_storage_limit` 0 means unlimited.
    if let Some(acl) = accessible.iter().find(|b| b.name == backend_name) {
        let bucket_used = UserFileRepository::sum_active_size_by_user_and_bucket(
            state.db.pool(),
            auth_user.user_id,
            &backend_name,
        )
        .await?;
        if !bucket_limit_allows(acl, bucket_used, data.len() as i64) {
            return Err(AppError::BadRequest(format!(
                "bucket '{}' storage limit exceeded (limit: {} bytes)",
                backend_name, acl.user_storage_limit
            )));
        }
    }

    // A new user_files entry will be created below, so now the physical file's
    // ref_count may be incremented (dedup path).
    if let Some(ref existing) = existing_physical {
        FileRepository::update_ref_count(state.db.pool(), existing.id, 1).await?;
    }

    // Create a user_files entry so this user has their own reference to the file.
    // Check for soft-deleted row with the same key first — restore it instead of inserting.
    let user_file = if let Some(deleted_uf) = UserFileRepository::find_deleted_by_user_file_and_name(
        state.db.pool(),
        auth_user.user_id,
        file.id,
        &original_name,
    ).await? {
        // Restore the soft-deleted entry. If no bucket was specified, keep the
        // row's previous bucket rather than clearing it to NULL.
        let restored_bucket = bucket_name.clone().or(deleted_uf.bucket_name.clone());
        UserFileRepository::restore(state.db.pool(), deleted_uf.id).await?;
        // Update bucket/folder if they changed
        if deleted_uf.bucket_name != restored_bucket || deleted_uf.folder_id != resolved_folder_id {
            UserFileRepository::update_bucket_and_folder(
                state.db.pool(),
                deleted_uf.id,
                restored_bucket,
                resolved_folder_id,
            ).await?;
        }
        // Refresh the MIME type if the client sent a more specific one.
        if let Some(ref mime) = mime_type {
            if deleted_uf.mime_type.as_deref() != Some(mime.as_str()) {
                UserFileRepository::update_mime_type(state.db.pool(), deleted_uf.id, mime).await?;
            }
        }
        // Re-fetch to get updated data
        UserFileRepository::find_by_id(state.db.pool(), deleted_uf.id)
            .await?
            .unwrap_or(deleted_uf)
    } else {
        // New row. Record the effective bucket (explicit or defaulted) so the
        // file is correctly attributed for access checks and bucket limits.
        let mut user_file_record =
            UserFileRecord::new(auth_user.user_id, file.id, original_name, mime_type.clone(), Some(backend_name));
        user_file_record.folder_id = resolved_folder_id;
        UserFileRepository::create(state.db.pool(), user_file_record).await?
    };

    // Charge quota atomically. The pre-check above is only a fast-fail for UX;
    // this conditional UPDATE is the authoritative gate, so concurrent uploads
    // cannot together exceed the quota (lost-update).
    if !UserRepository::charge_storage(
        state.db.pool(),
        auth_user.user_id,
        data.len() as i64,
    )
    .await?
    {
        return Err(AppError::BadRequest("storage quota exceeded".into()));
    }

    Ok(Json(UploadResponse {
        file: file_dto_from_user_file(&user_file, &file),
        duplicate: existing_physical.is_some(),
    }))
}

/// List files owned by the current user (via user_files).
pub async fn list_files(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Query(params): Query<ListFilesParams>,
) -> AppResult<Json<FileListDto>> {
    auth_user.require_scope("files:read")?;
    // Clamp pagination so `(page - 1) * per_page` can never overflow and the
    // query stays bounded.
    let page = params.page.unwrap_or(1).clamp(1, 1_000_000);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) as i64 * per_page as i64;
    let limit = per_page as i64;

    // Cap free-text search to a sane length.
    let search_owned = params
        .search
        .as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_SEARCH_LEN)
        .collect::<String>();
    let search = if search_owned.is_empty() {
        None
    } else {
        Some(search_owned.as_str())
    };
    let bucket = params.bucket.as_deref();
    let folder_id = params.folder_id.as_deref().and_then(|s| Uuid::parse_str(s).ok());
    let user_files_with_meta =
        UserFileRepository::list_by_user(state.db.pool(), auth_user.user_id, offset, limit, search, bucket, folder_id).await?;
    let total =
        UserFileRepository::count_by_user(state.db.pool(), auth_user.user_id, search, bucket, folder_id).await?;

    let files: Vec<FileDto> = user_files_with_meta
        .into_iter()
        .map(|(uf, blake3_hash, size, ref_count)| {
            // Synthesize a File-like metadata for the DTO
            let file = File {
                id: uf.file_id,
                blake3_hash,
                original_name: String::new(), // not used; we use uf.original_name
                mime_type: None,
                size,
                ref_count,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            file_dto_from_user_file(&uf, &file)
        })
        .collect();

    Ok(Json(FileListDto {
        files,
        total,
        page,
        per_page,
    }))
}

/// Get a single file's metadata. Ensures the file belongs to the current user.
pub async fn get_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<FileDto>> {
    auth_user.require_scope("files:read")?;
    // id here is the user_file id
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let file = FileRepository::find_by_id(state.db.pool(), user_file.file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file data not found".into()))?;

    Ok(Json(file_dto_from_user_file(&user_file, &file)))
}

/// Download a file. Ensures the file belongs to the current user.
/// Load a user's file and serve its bytes with the requested Content-Disposition.
/// `attachment` forces the browser to download; `inline` lets it render the raw
/// bytes in place (images, videos, PDFs, …). Both require `files:read` and the
/// bucket's `can_download` permission.
async fn serve_file(
    state: &AppState,
    auth_user: &AuthUser,
    id: Uuid,
    disposition: &'static str,
) -> Result<Response<Body>, AppError> {
    auth_user.require_scope("files:read")?;
    // id is the user_file id
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Check can_download permission for the bucket this file belongs to.
    // Files without a bucket are the user's private files and always allowed.
    if let Some(ref bucket_name) = user_file.bucket_name {
        let accessible = accessible_buckets(state, auth_user.user_id).await?;
        if !can_download_on(&accessible, bucket_name) {
            return Err(AppError::Forbidden(format!(
                "download not permitted for bucket '{}'",
                bucket_name
            )));
        }
    }

    let file = FileRepository::find_by_id(state.db.pool(), user_file.file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file data not found".into()))?;

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

    let content_type = user_file
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());

    // Stored XSS defense: only serve content inline when its MIME type cannot
    // execute active content. Anything else (HTML, SVG, JS, XML, unknown) is
    // silently downgraded to `attachment` so the browser downloads it instead
    // of rendering it inside the application's origin.
    let disposition = if disposition == "inline" && !is_inline_safe(&content_type) {
        "attachment"
    } else {
        disposition
    };

    let mut headers = HeaderMap::new();
    // Stored MIME values are sanitized at upload time, but legacy rows may hold
    // anything — fall back to octet-stream instead of failing the download.
    headers.insert(
        "content-type",
        content_type
            .parse()
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // ASCII-only, quote/backslash-free filename so the header can never be
    // poisoned with header injection and never fails to serialize.
    let safe_name: String = user_file
        .original_name
        .chars()
        .filter(|c| c.is_ascii() && !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let safe_name = if safe_name.is_empty() {
        "download".to_string()
    } else {
        safe_name
    };

    headers.insert(
        "content-disposition",
        format!("{disposition}; filename=\"{safe_name}\"")
            .parse()
            .map_err(|_| AppError::Internal("invalid content-disposition header".into()))?,
    );
    // Prevent MIME-sniffing attacks when serving a user-controlled content-type.
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("content-length", data.len().into());

    // Belt-and-braces for the raw endpoint: sandbox anything rendered inline so
    // even a browser MIME-confusion or a future inline-safe type regression
    // cannot execute scripts or open popups.
    if disposition == "inline" {
        headers.insert(
            "content-security-policy",
            HeaderValue::from_static("sandbox; default-src 'none'"),
        );
    }

    Ok((StatusCode::OK, headers, data).into_response())
}

/// Download a file as an attachment (the browser saves it instead of rendering).
pub async fn download_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, AppError> {
    serve_file(&state, &auth_user, id, "attachment").await
}

/// Serve a file's raw bytes inline so the browser renders them (images, videos,
/// PDFs, …). This is the "raw content" endpoint: same bytes as download, but
/// with `Content-Disposition: inline` instead of `attachment`.
pub async fn raw_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, AppError> {
    serve_file(&state, &auth_user, id, "inline").await
}

/// Delete a file reference for the current user.
/// Removes the user_files entry and decrements the physical file's ref_count.
/// The physical blob is only cleaned up when no users reference it (ref_count <= 0).
pub async fn delete_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:delete")?;
    // id is the user_file id
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Check can_upload permission for the bucket (delete requires write access)
    if let Some(ref bucket_name) = user_file.bucket_name {
        let accessible = accessible_buckets(&state, auth_user.user_id).await?;
        if !can_upload_on(&accessible, bucket_name) {
            return Err(AppError::Forbidden(format!(
                "delete not permitted for bucket '{}'",
                bucket_name
            )));
        }
    }

    // Soft-delete: mark the file as deleted but keep it in the database.
    // Physical file and storage quota are NOT affected — only permanent delete does that.
    UserFileRepository::delete(state.db.pool(), user_file.id).await?;

    Ok(Json(MessageResponse {
        message: format!("file '{}' deleted", user_file.original_name),
    }))
}

/// Verify a file's integrity. Ensures the file belongs to the current user.
pub async fn verify_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    auth_user.require_scope("files:read")?;
    // id is the user_file id
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Verifying reads the stored content, so it requires download permission
    // for the file's bucket (same gate as download_file).
    if let Some(ref bucket_name) = user_file.bucket_name {
        let accessible = accessible_buckets(&state, auth_user.user_id).await?;
        if !can_download_on(&accessible, bucket_name) {
            return Err(AppError::Forbidden(format!(
                "download not permitted for bucket '{}'",
                bucket_name
            )));
        }
    }

    let file = FileRepository::find_by_id(state.db.pool(), user_file.file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file data not found".into()))?;

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

    let computed_hash = hash_bytes(&data).await;
    let valid = computed_hash == file.blake3_hash;

    Ok(Json(serde_json::json!({
        "file_id": file.id,
        "user_file_id": user_file.id,
        "expected_hash": file.blake3_hash,
        "computed_hash": computed_hash,
        "valid": valid,
    })))
}

/// List buckets visible to the current user, with merged permissions across groups.
pub async fn list_user_buckets(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> AppResult<Json<Vec<UserBucketDto>>> {
    auth_user.require_scope("files:read")?;
    let buckets = BucketRepository::list_accessible_to_user(
        state.db.pool(),
        &auth_user.user_id.to_string(),
    )
    .await?;

    let dtos: Vec<UserBucketDto> = buckets
        .into_iter()
        .map(|b| UserBucketDto {
            id: b.id,
            name: b.name,
            can_upload: b.can_upload,
            can_download: b.can_download,
            user_storage_limit: b.user_storage_limit,
        })
        .collect();

    Ok(Json(dtos))
}

// ── File rename / move ──

/// Rename a file (update its original_name).
pub async fn rename_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameFileRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;
    let name = body.name.trim().to_string();
    validate_component_name(&name)
        .map_err(|e| AppError::BadRequest(format!("invalid file name: {e}")))?;

    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Renaming is a write operation on the file's bucket.
    if let Some(ref bucket_name) = user_file.bucket_name {
        let accessible = accessible_buckets(&state, auth_user.user_id).await?;
        if !can_upload_on(&accessible, bucket_name) {
            return Err(AppError::Forbidden(format!(
                "rename not permitted for bucket '{}'",
                bucket_name
            )));
        }
    }

    // Pre-check the unique index `idx_user_files_user_file
    // (user_id, file_id, original_name)`: renaming to a name that already
    // exists for the same content (possible after upload dedup) would otherwise
    // surface as a raw 500 from the constraint violation.
    if UserFileRepository::find_active_by_user_file_and_name(
        state.db.pool(),
        auth_user.user_id,
        user_file.file_id,
        &name,
    )
    .await?
    .is_some()
    {
        return Err(AppError::FileAlreadyExists(format!(
            "a file named '{name}' already exists in this location"
        )));
    }

    UserFileRepository::update_name(state.db.pool(), user_file.id, &name).await?;

    Ok(Json(MessageResponse {
        message: format!("file renamed to '{name}'"),
    }))
}

/// Move a file to a different folder/bucket (or root if folder_id is null).
/// Supports cross-bucket moves. When no target bucket is given, a target folder
/// determines the bucket (folders always belong to exactly one bucket), so a
/// file can never end up "inside" a folder of a different bucket.
pub async fn move_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveFileRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let mut target_bucket = body.bucket_name.clone().or(user_file.bucket_name.clone());

    // Validate the target folder belongs to the user and resolve the effective
    // target bucket: an explicit bucket must match the folder's bucket, and a
    // missing bucket adopts the folder's bucket.
    if let Some(target_folder_id) = body.folder_id {
        let target_folder = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            target_folder_id,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("target folder not found".into()))?;

        match &target_bucket {
            Some(b) if b != &target_folder.bucket_name => {
                return Err(AppError::BadRequest(
                    "target folder is in a different bucket than specified".into(),
                ));
            }
            Some(_) => {}
            None => target_bucket = Some(target_folder.bucket_name.clone()),
        }
    }

    // Cross-bucket moves require upload permission on the target bucket.
    if target_bucket != user_file.bucket_name {
        if let Some(ref tb) = target_bucket {
            let accessible = accessible_buckets(&state, auth_user.user_id).await?;
            if !can_upload_on(&accessible, tb) {
                return Err(AppError::Forbidden(format!(
                    "upload not permitted for bucket '{}'",
                    tb
                )));
            }
        }
    }

    UserFileRepository::update_bucket_and_folder(
        state.db.pool(),
        user_file.id,
        target_bucket,
        body.folder_id,
    )
    .await?;

    Ok(Json(MessageResponse {
        message: "file moved".into(),
    }))
}

/// Pick a copy name ("<base> - Copy", "<base> - Copy (2)", ...) that does not
/// collide with an existing ACTIVE or soft-deleted user_files row for the same
/// user + file. Soft-deleted rows still occupy the unique index
/// `idx_user_files_user_file (user_id, file_id, original_name)`, so a naive
/// "X - Copy" would collide after a delete-then-recopy.
async fn available_copy_name(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    file_id: Uuid,
    base: &str,
) -> AppResult<String> {
    let mut name = format!("{base} - Copy");
    let mut n = 2u32;
    loop {
        let active = UserFileRepository::find_active_by_user_file_and_name(
            pool, user_id, file_id, &name,
        )
        .await?;
        let deleted = UserFileRepository::find_deleted_by_user_file_and_name(
            pool, user_id, file_id, &name,
        )
        .await?;
        if active.is_none() && deleted.is_none() {
            return Ok(name);
        }
        name = format!("{base} - Copy ({n})");
        n += 1;
        if n > 1000 {
            return Err(AppError::Internal(
                "could not generate a unique copy name".into(),
            ));
        }
    }
}

/// Copy a file to a different folder/bucket. Creates a new user_files entry
/// pointing to the same physical file (content-addressed dedup).
pub async fn copy_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<CopyFileRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;
    let user_file = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    // Resolve the effective target bucket: an explicit bucket must match the
    // folder's bucket, and a missing bucket adopts the folder's bucket.
    let mut target_bucket = body.bucket_name.clone();
    if let Some(target_folder_id) = body.folder_id {
        let target_folder = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            target_folder_id,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("target folder not found".into()))?;

        match &target_bucket {
            Some(b) if b != &target_folder.bucket_name => {
                return Err(AppError::BadRequest(
                    "target folder is in a different bucket than specified".into(),
                ));
            }
            Some(_) => {}
            None => target_bucket = Some(target_folder.bucket_name.clone()),
        }
    }
    let target_bucket =
        target_bucket.unwrap_or_else(|| user_file.bucket_name.clone().unwrap_or_default());

    let accessible = accessible_buckets(&state, auth_user.user_id).await?;

    // Copying out of a bucket requires download permission on the SOURCE bucket.
    // Otherwise a user who can only upload to (but not download from) a bucket
    // could copy its files into a writable bucket and read them there.
    if let Some(ref source_bucket) = user_file.bucket_name {
        if !can_download_on(&accessible, source_bucket) {
            return Err(AppError::Forbidden(format!(
                "download not permitted for bucket '{}'",
                source_bucket
            )));
        }
    }

    // Copying requires upload permission on the target bucket.
    if !can_upload_on(&accessible, &target_bucket) {
        return Err(AppError::Forbidden(format!(
            "upload not permitted for bucket '{}'",
            target_bucket
        )));
    }

    let file = FileRepository::find_by_id(state.db.pool(), user_file.file_id)
        .await?
        .ok_or_else(|| AppError::NotFound("file data not found".into()))?;

    // Enforce the bucket's per-user storage limit before creating the copy
    // (each copy is a new logical reference charged against the user's quota).
    if let Some(acl) = accessible.iter().find(|b| b.name == target_bucket) {
        let bucket_used = UserFileRepository::sum_active_size_by_user_and_bucket(
            state.db.pool(),
            auth_user.user_id,
            &target_bucket,
        )
        .await?;
        if !bucket_limit_allows(acl, bucket_used, file.size) {
            return Err(AppError::BadRequest(format!(
                "bucket '{}' storage limit exceeded (limit: {} bytes)",
                target_bucket, acl.user_storage_limit
            )));
        }
    }

    let copy_name = available_copy_name(
        state.db.pool(),
        auth_user.user_id,
        user_file.file_id,
        &user_file.original_name,
    )
    .await?;
    validate_component_name(&copy_name).map_err(|e| {
        AppError::BadRequest(format!("cannot copy: the copy name would be invalid: {e}"))
    })?;
    let mut record = UserFileRecord::new(
        auth_user.user_id,
        user_file.file_id,
        copy_name,
        user_file.mime_type.clone(),
        Some(target_bucket.clone()),
    );
    record.folder_id = body.folder_id;

    // Charge quota atomically BEFORE creating the reference, compensating on
    // failure, so a concurrent copy can never push the user over the limit.
    if !UserRepository::charge_storage(state.db.pool(), auth_user.user_id, file.size).await? {
        return Err(AppError::BadRequest("storage quota exceeded".into()));
    }
    match UserFileRepository::create(state.db.pool(), record).await {
        Ok(_) => {
            // A new logical reference exists: keep ref_count consistent with the
            // upload dedup path, where every new user_files row counts toward the
            // physical file.
            FileRepository::update_ref_count(state.db.pool(), user_file.file_id, 1).await?;
        }
        Err(e) => {
            let _ = UserRepository::release_storage(state.db.pool(), auth_user.user_id, file.size)
                .await;
            return Err(e);
        }
    }

    Ok(Json(MessageResponse {
        message: "file copied".into(),
    }))
}

/// Batch move multiple files to a different folder/bucket.
pub async fn batch_move(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<BatchMoveRequest>,
) -> AppResult<Json<BatchResultResponse>> {
    auth_user.require_scope("files:write")?;
    if body.file_ids.is_empty() {
        return Err(AppError::BadRequest("file_ids cannot be empty".into()));
    }
    if body.file_ids.len() > MAX_BATCH_IDS {
        return Err(AppError::BadRequest(format!(
            "too many file ids (maximum {MAX_BATCH_IDS})"
        )));
    }

    let accessible = accessible_buckets(&state, auth_user.user_id).await?;

    // Validate the target folder up-front and derive its bucket (folders always
    // belong to exactly one bucket).
    let target_folder_bucket: Option<String> = if let Some(target_folder_id) = body.folder_id {
        let target_folder = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            target_folder_id,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("target folder not found".into()))?;

        if let Some(ref b) = body.bucket_name {
            if b != &target_folder.bucket_name {
                return Err(AppError::BadRequest(
                    "target folder is in a different bucket than specified".into(),
                ));
            }
        }
        Some(target_folder.bucket_name)
    } else {
        None
    };

    let mut success = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    for file_id in &body.file_ids {
        match UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, *file_id).await {
            Ok(Some(uf)) => {
                // Effective target bucket: explicit bucket, else the folder's
                // bucket, else the file's current bucket.
                let bucket = body
                    .bucket_name
                    .clone()
                    .or_else(|| target_folder_bucket.clone())
                    .or(uf.bucket_name.clone());

                // A cross-bucket move needs upload permission on the target.
                if bucket != uf.bucket_name {
                    match bucket.as_deref() {
                        Some(b) if !b.is_empty() && !can_upload_on(&accessible, b) => {
                            failed += 1;
                            errors.push(format!(
                                "{}: no upload permission for bucket '{}'",
                                uf.original_name, b
                            ));
                            continue;
                        }
                        _ => {}
                    }
                }

                match UserFileRepository::update_bucket_and_folder(
                    state.db.pool(), uf.id, bucket, body.folder_id
                ).await {
                    Ok(true) => success += 1,
                    Ok(false) => { failed += 1; errors.push(format!("{}: not found", uf.original_name)); }
                    Err(e) => { failed += 1; errors.push(format!("{}: {}", uf.original_name, e)); }
                }
            }
            _ => { failed += 1; errors.push(format!("file {} not found", file_id)); }
        }
    }

    Ok(Json(BatchResultResponse { success, failed, errors }))
}

/// Batch copy multiple files to a different folder/bucket.
pub async fn batch_copy(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<BatchCopyRequest>,
) -> AppResult<Json<BatchResultResponse>> {
    auth_user.require_scope("files:write")?;
    if body.file_ids.is_empty() {
        return Err(AppError::BadRequest("file_ids cannot be empty".into()));
    }
    if body.file_ids.len() > MAX_BATCH_IDS {
        return Err(AppError::BadRequest(format!(
            "too many file ids (maximum {MAX_BATCH_IDS})"
        )));
    }

    let accessible = accessible_buckets(&state, auth_user.user_id).await?;

    // Validate the target folder up-front and derive its bucket.
    let target_folder_bucket: Option<String> = if let Some(target_folder_id) = body.folder_id {
        let target_folder = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            target_folder_id,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("target folder not found".into()))?;

        if let Some(ref b) = body.bucket_name {
            if b != &target_folder.bucket_name {
                return Err(AppError::BadRequest(
                    "target folder is in a different bucket than specified".into(),
                ));
            }
        }
        Some(target_folder.bucket_name)
    } else {
        None
    };

    let mut success = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    for file_id in &body.file_ids {
        match UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, *file_id).await {
            Ok(Some(uf)) => {
                // Effective target bucket: explicit bucket, else the folder's
                // bucket, else the file's current bucket.
                let bucket = body
                    .bucket_name
                    .clone()
                    .or_else(|| target_folder_bucket.clone())
                    .or(uf.bucket_name.clone());

                // Copying out of a bucket requires download permission on the
                // SOURCE bucket, otherwise download restrictions could be
                // bypassed by copying a file into a writable bucket.
                if let Some(ref source_bucket) = uf.bucket_name {
                    if !can_download_on(&accessible, source_bucket) {
                        failed += 1;
                        errors.push(format!(
                            "{}: no download permission for bucket '{}'",
                            uf.original_name, source_bucket
                        ));
                        continue;
                    }
                }

                // Copying requires upload permission on the target bucket.
                match bucket.as_deref() {
                    Some(b) if !b.is_empty() && !can_upload_on(&accessible, b) => {
                        failed += 1;
                        errors.push(format!(
                            "{}: no upload permission for bucket '{}'",
                            uf.original_name, b
                        ));
                        continue;
                    }
                    _ => {}
                }

                let copy_name = match available_copy_name(
                    state.db.pool(),
                    auth_user.user_id,
                    uf.file_id,
                    &uf.original_name,
                )
                .await
                {
                    Ok(name) => name,
                    Err(e) => {
                        failed += 1;
                        errors.push(format!("{}: {}", uf.original_name, e));
                        continue;
                    }
                };
                if let Err(e) = validate_component_name(&copy_name) {
                    failed += 1;
                    errors.push(format!("{}: copy name would be invalid ({e})", uf.original_name));
                    continue;
                }

                let file = match FileRepository::find_by_id(state.db.pool(), uf.file_id).await {
                    Ok(Some(f)) => f,
                    _ => {
                        failed += 1;
                        errors.push(format!("{}: file data not found", uf.original_name));
                        continue;
                    }
                };

                // Per-bucket storage limit check (the global quota is enforced
                // atomically by charge_storage just before the insert).
                if let Some(b) = bucket.as_deref() {
                    if !b.is_empty() {
                        if let Some(acl) = accessible.iter().find(|a| a.name.as_str() == b) {
                            let bucket_used = UserFileRepository::sum_active_size_by_user_and_bucket(
                                state.db.pool(),
                                auth_user.user_id,
                                b,
                            )
                            .await?;
                            if !bucket_limit_allows(acl, bucket_used, file.size) {
                                failed += 1;
                                errors.push(format!(
                                    "{}: bucket '{}' storage limit exceeded",
                                    uf.original_name, b
                                ));
                                continue;
                            }
                        }
                    }
                }

                let mut record = UserFileRecord::new(
                    auth_user.user_id, uf.file_id, copy_name,
                    uf.mime_type.clone(), bucket,
                );
                record.folder_id = body.folder_id;
                // Charge quota atomically per item BEFORE creating the reference
                // (compensating on failure). The previous approach checked each
                // item against a single stale snapshot and wrote the aggregate
                // afterwards, so a batch whose items individually fit but summed
                // over the quota all succeeded (quota bypass), and concurrent
                // requests lost updates.
                if !UserRepository::charge_storage(state.db.pool(), auth_user.user_id, file.size).await? {
                    failed += 1;
                    errors.push(format!("{}: storage quota exceeded", uf.original_name));
                    continue;
                }
                match UserFileRepository::create(state.db.pool(), record).await {
                    Ok(_) => {
                        FileRepository::update_ref_count(state.db.pool(), uf.file_id, 1).await?;
                        success += 1;
                    }
                    Err(e) => {
                        let _ = UserRepository::release_storage(
                            state.db.pool(),
                            auth_user.user_id,
                            file.size,
                        )
                        .await;
                        failed += 1;
                        errors.push(format!("{}: {}", uf.original_name, e));
                    }
                }
            }
            _ => { failed += 1; errors.push(format!("file {} not found", file_id)); }
        }
    }

    Ok(Json(BatchResultResponse { success, failed, errors }))
}

/// Batch delete multiple files.
pub async fn batch_delete(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<BatchDeleteRequest>,
) -> AppResult<Json<BatchResultResponse>> {
    auth_user.require_scope("files:delete")?;
    if body.file_ids.is_empty() {
        return Err(AppError::BadRequest("file_ids cannot be empty".into()));
    }
    if body.file_ids.len() > MAX_BATCH_IDS {
        return Err(AppError::BadRequest(format!(
            "too many file ids (maximum {MAX_BATCH_IDS})"
        )));
    }

    let accessible = accessible_buckets(&state, auth_user.user_id).await?;

    let mut success = 0usize;
    let mut failed = 0usize;
    let mut errors = Vec::new();

    for file_id in &body.file_ids {
        match UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, *file_id).await {
            Ok(Some(uf)) => {
                // Check can_upload permission for the bucket
                let can_delete = if let Some(ref bn) = uf.bucket_name {
                    can_upload_on(&accessible, bn)
                } else {
                    true
                };

                if !can_delete {
                    failed += 1;
                    errors.push(format!("{}: no permission", uf.original_name));
                    continue;
                }

                // Soft-delete only: mark as deleted, keep physical file and storage quota
                match UserFileRepository::delete(state.db.pool(), uf.id).await {
                    Ok(true) => success += 1,
                    _ => { failed += 1; errors.push(format!("{}: delete failed", uf.original_name)); }
                }
            }
            _ => { failed += 1; errors.push(format!("file {} not found", file_id)); }
        }
    }

    Ok(Json(BatchResultResponse { success, failed, errors }))
}

// ── Folder CRUD ──

/// Create a new virtual folder.
pub async fn create_folder(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<CreateFolderRequest>,
) -> AppResult<Json<FolderDto>> {
    auth_user.require_scope("files:write")?;
    let name = body.name.trim().to_string();
    validate_component_name(&name)
        .map_err(|e| AppError::BadRequest(format!("invalid folder name: {e}")))?;

    // Verify bucket access (folder creation is a write operation)
    let accessible = accessible_buckets(&state, auth_user.user_id).await?;
    if !can_upload_on(&accessible, &body.bucket_name) {
        return Err(AppError::Forbidden(format!(
            "cannot create folders in bucket '{}'",
            body.bucket_name
        )));
    }

    // Verify the parent chain (if any): every ancestor must belong to the same
    // user and bucket, and nesting depth is capped to match resolve_path's
    // 32-segment limit so breadcrumb walks stay bounded.
    if let Some(mut current_id) = body.parent_id {
        let mut depth = 1usize;
        loop {
            let parent = FolderRepository::find_by_user_and_id(
                state.db.pool(),
                auth_user.user_id,
                current_id,
            )
            .await?
            .ok_or_else(|| AppError::NotFound("parent folder not found".into()))?;

            if parent.bucket_name != body.bucket_name {
                return Err(AppError::BadRequest(
                    "parent folder is in a different bucket".into(),
                ));
            }

            match parent.parent_id {
                Some(pid) => {
                    if depth >= 32 {
                        return Err(AppError::BadRequest(
                            "folder nesting too deep (maximum 32 levels)".into(),
                        ));
                    }
                    current_id = pid;
                    depth += 1;
                }
                None => break,
            }
        }
    }

    let record = FolderRecord::new(
        auth_user.user_id,
        body.bucket_name,
        name,
        body.parent_id,
    );
    let folder = FolderRepository::create(state.db.pool(), record).await?;

    let file_count = FolderRepository::count_files(state.db.pool(), folder.id).await?;
    let folder_count = FolderRepository::count_subfolders(state.db.pool(), folder.id).await?;

    Ok(Json(FolderDto {
        id: folder.id,
        name: folder.name,
        parent_id: folder.parent_id,
        bucket_name: folder.bucket_name,
        created_at: folder.created_at,
        file_count,
        folder_count,
    }))
}

/// List folder contents (subfolders + files) with breadcrumb path.
pub async fn list_folder_contents(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Query(params): Query<ListFilesParams>,
) -> AppResult<Json<FolderContentDto>> {
    auth_user.require_scope("files:read")?;
    let bucket = params.bucket.as_ref()
        .ok_or_else(|| AppError::BadRequest("bucket parameter is required".into()))?;

    let folder_id = params.folder_id.as_deref().and_then(|s| Uuid::parse_str(s).ok());

    // The folder must belong to the user and to the requested bucket; otherwise
    // the breadcrumb walk would leak path information for folders the user can
    // see but that live in a different bucket.
    if let Some(fid) = folder_id {
        let folder = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            fid,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("folder not found".into()))?;
        if folder.bucket_name != *bucket {
            return Err(AppError::BadRequest(
                "folder is in a different bucket than requested".into(),
            ));
        }
    }

    // List subfolders
    let folders = FolderRepository::list_children(
        state.db.pool(),
        auth_user.user_id,
        bucket,
        folder_id,
    )
    .await?;

    let folder_dtos: Vec<FolderDto> = {
        let mut dtos = Vec::new();
        for f in folders {
            let file_count = FolderRepository::count_files(state.db.pool(), f.id).await?;
            let folder_count = FolderRepository::count_subfolders(state.db.pool(), f.id).await?;
            dtos.push(FolderDto {
                id: f.id,
                name: f.name,
                parent_id: f.parent_id,
                bucket_name: f.bucket_name,
                created_at: f.created_at,
                file_count,
                folder_count,
            });
        }
        dtos
    };

    // Build breadcrumb path
    let path = if let Some(fid) = folder_id {
        let chain = FolderRepository::get_path(state.db.pool(), fid).await?;
        let mut breadcrumbs = vec![FolderBreadcrumb { id: None, name: "Root".into() }];
        for (id, name) in chain {
            breadcrumbs.push(FolderBreadcrumb { id: Some(id), name });
        }
        breadcrumbs
    } else {
        vec![FolderBreadcrumb { id: None, name: "Root".into() }]
    };

    Ok(Json(FolderContentDto {
        folders: folder_dtos,
        files: vec![], // files are fetched separately via list_files
        path,
    }))
}

/// List ALL folders in a bucket (flat list for building a tree on the client).
pub async fn list_all_folders(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Query(params): Query<ListFilesParams>,
) -> AppResult<Json<FolderTreeDto>> {
    auth_user.require_scope("files:read")?;
    let bucket = params.bucket.as_ref()
        .ok_or_else(|| AppError::BadRequest("bucket parameter is required".into()))?;

    let folders = FolderRepository::list_all_for_bucket(
        state.db.pool(),
        auth_user.user_id,
        bucket,
    )
    .await?;

    let items: Vec<FolderTreeItem> = folders.into_iter().map(|f| FolderTreeItem {
        id: f.id,
        name: f.name,
        parent_id: f.parent_id,
    }).collect();

    Ok(Json(FolderTreeDto { folders: items }))
}

#[derive(Debug, Deserialize)]
pub struct ResolvePathParams {
    pub bucket_id: Option<String>,
    pub path: Option<String>,
}

/// Resolve a folder path like "/Documents/Work" to a folder ID.
/// Used by the frontend to support deep-linking via query params.
/// Accepts `bucket_id` (UUID) + `path` (slash-separated).
/// Security:
///   - Requires authentication (AuthUser extractor)
///   - Path segments are matched via parameterized SQL (no injection)
///   - Each segment is scoped to user_id + bucket_name (no cross-user access)
///   - Segment length and count are capped to prevent abuse
///   - Empty/whitespace-only segments are rejected
pub async fn resolve_folder_path(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Query(params): Query<ResolvePathParams>,
) -> AppResult<Json<FolderResolveDto>> {
    auth_user.require_scope("files:read")?;
    let bucket_id = params.bucket_id.as_ref()
        .ok_or_else(|| AppError::BadRequest("bucket_id parameter is required".into()))?;
    let path = params.path.as_ref()
        .ok_or_else(|| AppError::BadRequest("path parameter is required".into()))?;

    // Cap the raw path length so a huge input cannot force expensive splitting.
    if path.len() > 4096 {
        return Err(AppError::BadRequest("path is too long".into()));
    }

    // Validate bucket_id: must be a valid UUID
    let bucket_uuid = uuid::Uuid::parse_str(bucket_id)
        .map_err(|_| AppError::BadRequest("invalid bucket_id".into()))?;

    // The user must actually have access to the bucket: this avoids a bucket
    // existence oracle and prevents resolving paths in inaccessible buckets.
    let accessible = accessible_buckets(&state, auth_user.user_id).await?;
    let bucket_name = accessible
        .iter()
        .find(|b| b.id == bucket_uuid.to_string())
        .map(|b| b.name.clone())
        .ok_or_else(|| AppError::NotFound("bucket not found".into()))?;

    let result = FolderRepository::resolve_path(
        state.db.pool(),
        auth_user.user_id,
        &bucket_name,
        path,
    )
    .await?;

    match result {
        Some((folder_id, path_chain)) => {
            let breadcrumbs: Vec<FolderBreadcrumb> = path_chain
                .into_iter()
                .map(|(id, name)| FolderBreadcrumb {
                    id: if id.is_nil() { None } else { Some(id) },
                    name,
                })
                .collect();

            Ok(Json(FolderResolveDto {
                folder_id,
                path: breadcrumbs,
            }))
        }
        None => Err(AppError::NotFound("folder path not found".into())),
    }
}

/// Rename a folder.
pub async fn rename_folder(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<RenameFolderRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;
    let name = body.name.trim().to_string();
    validate_component_name(&name)
        .map_err(|e| AppError::BadRequest(format!("invalid folder name: {e}")))?;

    let folder = FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("folder not found".into()))?;

    FolderRepository::update_name(state.db.pool(), folder.id, &name).await?;

    Ok(Json(MessageResponse {
        message: format!("folder renamed to '{name}'"),
    }))
}

/// Delete a folder. Children are moved to the parent (or root).
pub async fn delete_folder(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:delete")?;
    let folder = FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("folder not found".into()))?;

    // Deleting a folder soft-deletes every file in it, so it requires write
    // access to the folder's bucket (matches delete_file semantics).
    let accessible = accessible_buckets(&state, auth_user.user_id).await?;
    if !can_upload_on(&accessible, &folder.bucket_name) {
        return Err(AppError::Forbidden(format!(
            "cannot delete folders in bucket '{}'",
            folder.bucket_name
        )));
    }

    FolderRepository::delete(state.db.pool(), folder.id).await?;

    Ok(Json(MessageResponse {
        message: format!("folder '{}' deleted", folder.name),
    }))
}

/// Move a folder to a new parent (or root if folder_id is null).
pub async fn move_folder(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<MoveFolderRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;
    let folder = FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, id)
        .await?
        .ok_or_else(|| AppError::NotFound("folder not found".into()))?;

    // The target folder must belong to the same user and the same bucket, so
    // folders can never end up nested across buckets.
    if let Some(mut target_id) = body.folder_id {
        let target = FolderRepository::find_by_user_and_id(
            state.db.pool(),
            auth_user.user_id,
            target_id,
        )
        .await?
        .ok_or_else(|| AppError::NotFound("target folder not found".into()))?;

        if target.bucket_name != folder.bucket_name {
            return Err(AppError::BadRequest(
                "cannot move folder into a folder of a different bucket".into(),
            ));
        }

        // Cap the resulting ancestor-chain depth (matches resolve_path's
        // 32-segment limit) so breadcrumb walks stay bounded.
        let mut depth = 1usize;
        loop {
            match FolderRepository::find_by_user_and_id(
                state.db.pool(),
                auth_user.user_id,
                target_id,
            )
            .await?
            {
                Some(f) => match f.parent_id {
                    Some(pid) => {
                        if depth >= 32 {
                            return Err(AppError::BadRequest(
                                "folder nesting too deep (maximum 32 levels)".into(),
                            ));
                        }
                        target_id = pid;
                        depth += 1;
                    }
                    None => break,
                },
                None => break,
            }
        }
    }

    FolderRepository::move_folder(state.db.pool(), folder.id, body.folder_id).await?;

    let target = match body.folder_id {
        Some(_) => "moved",
        None => "moved to root",
    };

    Ok(Json(MessageResponse {
        message: format!("folder '{}' {}", folder.name, target),
    }))
}
