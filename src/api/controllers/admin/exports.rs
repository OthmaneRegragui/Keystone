use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use chrono::Utc;
use futures::TryStreamExt;
use tracing::{info, warn};
use uuid::Uuid;

use crate::api::extractors::AuthUser;
use crate::db::repos::buckets::BucketRepository;
use crate::db::repos::folders::{FolderExportRow, FolderRepository};
use crate::db::repos::user_files::{UserFileExportRow, UserFileRepository};
use crate::db::repos::users::UserRepository;
use crate::dto::*;
use crate::error::{AppError, AppResult};
use crate::utils::names::validate_component_name;
use crate::AppState;

/// Zip entry names are built from DB-sourced values (usernames, folder names,
/// original names) that are validated on write, but legacy rows could still
/// contain separators or `..`. Those must never appear inside an archive that
/// admins will extract on their own machines, so every segment is re-checked
/// before the entry is written.
fn zip_entry_name_ok(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    path.split('/')
        .all(|seg| !seg.is_empty() && validate_component_name(seg).is_ok())
}

/// Build a map of folder_id -> full path string for a given set of folders.
/// Each folder is prefixed by the username to keep per-user trees separate.
#[allow(dead_code)]
fn build_folder_paths(
    folders: &[FolderExportRow],
) -> HashMap<String, String> {
    // Build lookup: id -> (name, parent_id)
    let mut map: HashMap<String, (&str, Option<&str>)> = HashMap::new();
    for f in folders {
        map.insert(f.id.clone(), (f.name.as_str(), f.parent_id.as_deref()));
    }

    // Build full path for each folder
    let mut paths: HashMap<String, String> = HashMap::new();
    for f in folders {
        let mut segments = Vec::new();
        let mut current_id = Some(f.id.as_str());
        // Guard against parent_id cycles, which would otherwise loop forever
        // (a folder that is its own ancestor via a corrupted tree). We only
        // permit each folder along an ancestral chain once.
        let mut visited: HashSet<String> = HashSet::new();
        while let Some(cid) = current_id {
            if !visited.insert(cid.to_string()) {
                break;
            }
            if let Some((name, parent)) = map.get(cid) {
                segments.push(name.to_string());
                current_id = *parent;
            } else {
                break;
            }
        }
        segments.reverse();
        paths.insert(f.id.clone(), segments.join("/"));
    }
    paths
}

/// Recursively build the full path for a single folder_id.
fn resolve_folder_path(
    folder_id: &str,
    folder_map: &HashMap<String, (String, Option<String>)>,
) -> String {
    let mut segments = Vec::new();
    let mut current = Some(folder_id.to_string());
    // Guard against parent_id cycles so a corrupted tree cannot loop forever.
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(ref cid) = current {
        if !visited.insert(cid.clone()) {
            break;
        }
        if let Some((name, parent)) = folder_map.get(cid) {
            segments.push(name.clone());
            current = parent.clone();
        } else {
            break;
        }
    }
    segments.reverse();
    segments.join("/")
}

// ─── Export Index (JSON) ──────────────────────────────────────

pub async fn export_bucket_index(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
) -> AppResult<Json<BucketIndexExportDto>> {
    auth.require_admin()?;

    // Verify bucket exists
    BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Fetch all files in the bucket (all users, non-deleted)
    let file_rows: Vec<UserFileExportRow> = UserFileRepository::list_by_bucket_for_export(state.db.pool(), &bucket_name).await?;

    // Fetch all folders in the bucket (all users)
    let folder_rows: Vec<FolderExportRow> = FolderRepository::list_all_for_bucket_admin(state.db.pool(), &bucket_name).await?;

    // Build folder path lookup: folder_id -> full path
    let folder_map: HashMap<String, (String, Option<String>)> = folder_rows
        .iter()
        .map(|f| {
            (
                f.id.clone(),
                (f.name.clone(), f.parent_id.clone()),
            )
        })
        .collect();

    // Group files by user
    let mut user_files_map: HashMap<String, Vec<BucketExportFileDto>> = HashMap::new();
    // Also store user info by id
    let mut user_info_map: HashMap<String, (String, String)> = HashMap::new(); // user_id -> (username, email)

    // Populate user info from file rows (users that have files)
    for row in &file_rows {
        user_info_map
            .entry(row.user_id.clone())
            .or_insert_with(|| (row.username.clone(), String::new()));

        let folder_path = row
            .folder_id
            .as_ref()
            .map(|fid| resolve_folder_path(fid, &folder_map));

        user_files_map
            .entry(row.user_id.clone())
            .or_default()
            .push(BucketExportFileDto {
                name: row.original_name.clone(),
                folder: folder_path,
                size: row.size,
                hash: row.blake3_hash.clone(),
                mime_type: row.mime_type.clone(),
                created_at: row.created_at.clone(),
            });
    }

    // Also populate user info from folder rows so users with folders (but no files) are included
    for f in &folder_rows {
        user_info_map
            .entry(f.user_id.clone())
            .or_insert_with(|| (f.username.clone(), String::new()));
    }

    // Fetch email for each user (we need to fetch from DB since our query above only has username)
    // Actually, let's update our query or fetch emails separately.
    // For the file query rows, we had username but not email. Let's fetch emails for all distinct user_ids.
    let user_ids: Vec<String> = user_info_map.keys().cloned().collect();
    for uid in &user_ids {
        if let Ok(Some(user)) = UserRepository::find_by_id(state.db.pool(), Uuid::parse_str(uid).unwrap_or_default()).await {
            user_info_map.insert(uid.clone(), (user.username, user.email));
        }
    }

    // Group folders by user
    let mut user_folders_map: HashMap<String, Vec<BucketExportFolderDto>> = HashMap::new();
    for f in &folder_rows {
        let full_path = resolve_folder_path(&f.id, &folder_map);
        let parent_name = f.parent_id.as_ref().and_then(|pid| {
            folder_map.get(pid).map(|(name, _): &(String, Option<String>)| name.clone())
        });

        user_folders_map
            .entry(f.user_id.clone())
            .or_default()
            .push(BucketExportFolderDto {
                name: f.name.clone(),
                parent: parent_name,
                full_path,
                created_at: f.created_at.clone(),
            });
    }

    // Build the user DTOs
    let mut users_dto: Vec<BucketExportUserDto> = user_info_map
        .into_iter()
        .map(|(uid, (uname, email))| BucketExportUserDto {
            user_id: uid.clone(),
            username: uname.clone(),
            email,
            files: user_files_map.remove(&uid).unwrap_or_default(),
            folders: user_folders_map.remove(&uid).unwrap_or_default(),
        })
        .collect();
    users_dto.sort_by(|a, b| a.username.cmp(&b.username));

    Ok(Json(BucketIndexExportDto {
        bucket: bucket_name,
        exported_at: Utc::now().to_rfc3339(),
        users: users_dto,
    }))
}

// ─── Export ZIP ───────────────────────────────────────────────

pub async fn export_bucket_zip(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(bucket_name): Path<String>,
) -> AppResult<Response<Body>> {
    auth.require_admin()?;

    // The name flows into ZIP entry paths; reject anything that could contain
    // path separators or traversal components before it is echoed verbatim.
    validate_component_name(&bucket_name)
        .map_err(|e| AppError::BadRequest(format!("invalid bucket name: {e}")))?;

    // Verify bucket exists. The archive mirrors the bucket's physical
    // storage folder, so we need its path (not the logical user layout).
    let bucket = BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Root the archive at the bucket's path relative to a configured storage
    // root (e.g. `p1/b1`), so unzipping it at a storage root recreates the
    // folder verbatim.
    let bucket_path = bucket.path.clone();
    let zip_root = bucket_zip_root(&state.config.storage.local_paths, &bucket.path, &bucket_name);

    // Stream the archive to the client. The ZIP is written to a temporary file
    // on disk (the writer needs a seekable sink) rather than being buffered
    // wholesale in RAM, so an admin exporting a very large bucket cannot
    // exhaust server memory. A background task emits the file's bytes through a
    // bounded channel, and the response body streams them to the client.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);

    let task_bucket = bucket_name.clone();
    let task_user = auth.username.clone();

    tokio::spawn(async move {
        let mut tmpfile = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => {
                warn!("export: cannot create temp file for bucket '{task_bucket}': {e}");
                return;
            }
        };

        let (exported, _total_bytes) = match write_bucket_zip(
            &bucket_path,
            &zip_root,
            tmpfile.as_file_mut(),
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!("export: failed to build zip for bucket '{task_bucket}': {e}");
                return;
            }
        };

        // Rewind and stream the completed archive to the client in bounded chunks.
        use std::io::{Read, Seek};
        if let Err(e) = tmpfile.as_file_mut().seek(std::io::SeekFrom::Start(0)) {
            warn!("export: cannot rewind temp file for '{task_bucket}': {e}");
            return;
        }
        let file = tmpfile.as_file_mut();
        let mut buf = vec![0u8; 64 * 1024];
        let mut sent = 0u64;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = Bytes::copy_from_slice(&buf[..n]);
            // `send().await` (not `blocking_send`) — this task runs on a tokio
            // worker thread, where blocking_send panics with "Cannot block the
            // current thread from within a runtime", killing the stream and
            // leaving the client with an empty (0-byte) archive.
            if tx.send(Ok(chunk)).await.is_err() {
                break; // client disconnected
            }
            sent += n as u64;
        }
        info!(
            "admin {} exported bucket '{}' as ZIP ({} files, {} bytes)",
            task_user, task_bucket, exported, sent
        );
    });

    // The response body streams the channel's contents. Content-Length is
    // intentionally omitted so the connection uses chunked transfer encoding.
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let body = Body::from_stream(stream);

    let zip_filename = format!("{}.zip", bucket_name);
    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        "application/zip"
            .parse()
            .map_err(|_| AppError::Internal("invalid content-type header".into()))?,
    );
    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"{}\"", zip_filename)
            .parse()
            .map_err(|_| AppError::Internal("invalid content-disposition header".into()))?,
    );

    Ok((StatusCode::OK, headers, body).into_response())
}

/// Compute the archive root folder for a bucket's physical storage directory:
/// its path relative to a configured storage root (e.g. `p1/b1`), falling back
/// to the bucket name when the path lives outside every storage root.
fn bucket_zip_root(storage_roots: &[String], bucket_path: &str, bucket_name: &str) -> String {
    let bp = std::path::Path::new(bucket_path);
    for root in storage_roots {
        let rp = std::path::Path::new(root);
        if let Ok(rel) = bp.strip_prefix(rp) {
            if !rel.as_os_str().is_empty() {
                return rel.to_string_lossy().replace('\\', "/");
            }
        }
    }
    bucket_name.to_string()
}

/// Recursively add a directory (and everything below it) from the bucket's
/// physical storage folder into the archive. Returns the number of files
/// written and their total byte size.
fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<std::io::BufWriter<&mut std::fs::File>>,
    created_dirs: &mut HashSet<String>,
    options: zip::write::SimpleFileOptions,
    fs_dir: &std::path::Path,
    rel_prefix: &str,
) -> AppResult<(usize, u64)> {
    let mut exported = 0usize;
    let mut total_bytes = 0u64;

    // Emit a directory entry for the folder itself (including the archive root).
    if !rel_prefix.is_empty() && created_dirs.insert(rel_prefix.to_string()) {
        let dir_path = format!("{}/", rel_prefix);
        zip.add_directory(&dir_path, options)
            .map_err(|e| AppError::Internal(format!("failed to add directory to zip: {e}")))?;
    }

    let entries = std::fs::read_dir(fs_dir).map_err(|e| {
        AppError::Internal(format!(
            "failed to read storage directory '{}': {e}",
            fs_dir.display()
        ))
    })?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!("export: skipping unreadable entry in '{}': {e}", fs_dir.display());
                continue;
            }
        };

        // Use symlink_metadata so symlinks are never followed — a link inside
        // the bucket folder could point outside it and silently drag
        // unrelated data into the archive.
        let meta = match std::fs::symlink_metadata(entry.path()) {
            Ok(m) => m,
            Err(e) => {
                warn!("export: cannot stat '{}': {e}", entry.path().display());
                continue;
            }
        };

        let name = entry.file_name().to_string_lossy().replace('\\', "/");
        let rel = if rel_prefix.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_prefix, name)
        };

        if meta.is_dir() {
            if !zip_entry_name_ok(&rel) {
                warn!("export: skipping folder entry '{rel}' (invalid path components)");
                continue;
            }
            let (sub_files, sub_bytes) =
                add_dir_to_zip(zip, created_dirs, options, &entry.path(), &rel)?;
            exported += sub_files;
            total_bytes += sub_bytes;
        } else if meta.is_file() {
            if !zip_entry_name_ok(&rel) {
                warn!("export: skipping '{}' (invalid path components)", rel);
                continue;
            }
            let mut reader = std::io::BufReader::new(std::fs::File::open(entry.path()).map_err(|e| {
                AppError::Internal(format!(
                    "failed to open '{}' for export: {e}",
                    entry.path().display()
                ))
            })?);
            zip.start_file(&rel, options)
                .map_err(|e| AppError::Internal(format!("failed to start zip entry '{rel}': {e}")))?;
            let copied = std::io::copy(&mut reader, zip)
                .map_err(|e| AppError::Internal(format!("failed to write zip entry: {e}")))?;
            exported += 1;
            total_bytes += copied;
        } else {
            warn!(
                "export: skipping non-regular file '{}'",
                entry.path().display()
            );
        }
    }

    Ok((exported, total_bytes))
}

/// Build a ZIP archive of a bucket's physical storage folder into `file`.
/// Returns the number of files written and their total byte size. `file` must
/// start empty and is left positioned at the end of the archive.
async fn write_bucket_zip(
    bucket_path: &str,
    zip_root: &str,
    file: &mut std::fs::File,
) -> AppResult<(usize, u64)> {
    let mut zip_writer = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let fs_root = std::path::Path::new(bucket_path);
    if !fs_root.is_dir() {
        warn!("export: bucket storage folder '{}' does not exist", bucket_path);
        let _ = zip_writer
            .finish()
            .map_err(|e| AppError::Internal(format!("failed to finalize zip: {e}")))?
            .flush()
            .map_err(|e| AppError::Internal(format!("failed to flush zip: {e}")))?;
        return Ok((0, 0));
    }

    let mut created_dirs: HashSet<String> = HashSet::new();
    let (exported, total_bytes) =
        add_dir_to_zip(&mut zip_writer, &mut created_dirs, options, fs_root, zip_root)?;

    // Finalize zip and flush the buffered writer to disk.
    let _ = zip_writer
        .finish()
        .map_err(|e| AppError::Internal(format!("failed to finalize zip: {e}")))?
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush zip: {e}")))?;

    Ok((exported, total_bytes))
}
