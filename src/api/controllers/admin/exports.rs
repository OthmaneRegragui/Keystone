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
use crate::db::repos::storage::StorageObjectRepository;
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

    // Verify bucket exists
    let _bucket = BucketRepository::find_by_name(state.db.pool(), &bucket_name)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("bucket '{}' not found", bucket_name)))?;

    // Fetch all files in the bucket (all users, non-deleted)
    let file_rows: Vec<UserFileExportRow> = UserFileRepository::list_by_bucket_for_export(state.db.pool(), &bucket_name).await?;

    // Fetch all folders to build path map
    let folder_rows: Vec<FolderExportRow> = FolderRepository::list_all_for_bucket_admin(state.db.pool(), &bucket_name).await?;
    let folder_map: HashMap<String, (String, Option<String>)> = folder_rows
        .iter()
        .map(|f| (f.id.clone(), (f.name.clone(), f.parent_id.clone())))
        .collect();

    // Also track which users have which folders for empty folder inclusion
    let mut user_folders_set: HashMap<String, Vec<String>> = HashMap::new(); // user_id -> list of folder paths
    for f in &folder_rows {
        let fp = resolve_folder_path(&f.id, &folder_map);
        user_folders_set
            .entry(f.user_id.clone())
            .or_default()
            .push(fp);
    }

    // Stream the archive to the client. The ZIP is written to a temporary file
    // on disk (the writer needs a seekable sink) rather than being buffered
    // wholesale in RAM, so an admin exporting a very large bucket cannot
    // exhaust server memory. A background task emits the file's bytes through a
    // bounded channel, and the response body streams them to the client.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(16);

    let task_state = state.clone();
    let task_bucket = bucket_name.clone();
    let task_user = auth.username.clone();
    let task_file_rows = file_rows;
    let task_user_folders_set = user_folders_set;
    let task_folder_map = folder_map;

    tokio::spawn(async move {
        let mut tmpfile = match tempfile::NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => {
                warn!("export: cannot create temp file for bucket '{task_bucket}': {e}");
                return;
            }
        };

        let (exported, _total_bytes) = match write_bucket_zip(
            &task_state,
            &task_bucket,
            &task_file_rows,
            &task_user_folders_set,
            &task_folder_map,
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
            if tx.blocking_send(Ok(chunk)).is_err() {
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

/// Build a ZIP archive of a bucket into `file`. Returns the number of files
/// written and their total byte size. `file` must start empty and is left
/// positioned at the end of the archive.
async fn write_bucket_zip(
    state: &Arc<AppState>,
    bucket_name: &str,
    file_rows: &[UserFileExportRow],
    user_folders_set: &HashMap<String, Vec<String>>,
    folder_map: &HashMap<String, (String, Option<String>)>,
    file: &mut std::fs::File,
) -> AppResult<(usize, u64)> {
    let mut zip_writer = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    // Track which directories have been created to avoid duplicates
    let mut created_dirs: HashMap<String, bool> = HashMap::new();

    // Macro-like helper to ensure a directory entry exists in the zip
    // We use a local fn since closures can't borrow zip_writer while it's also used directly
    fn ensure_dir_in_zip(
        zip: &mut zip::ZipWriter<std::io::BufWriter<&mut std::fs::File>>,
        dirs: &mut HashMap<String, bool>,
        path: &str,
        opts: zip::write::SimpleFileOptions,
    ) -> AppResult<()> {
        if !dirs.contains_key(path) {
            let dir_path = if path.ends_with('/') {
                path.to_string()
            } else {
                format!("{}/", path)
            };
            zip.add_directory(&dir_path, opts)
                .map_err(|e| AppError::Internal(format!("failed to add directory to zip: {e}")))?;
            dirs.insert(path.to_string(), true);
        }
        Ok(())
    }

    // First, create empty folder entries for each user
    for (user_id, folder_paths) in user_folders_set {
        let user = match UserRepository::find_by_id(state.db.pool(), Uuid::parse_str(user_id).unwrap_or_default()).await {
            Ok(Some(u)) => u,
            _ => continue,
        };
        for fp in folder_paths {
            let dir_path = format!("{}/{}/{}", bucket_name, user.username, fp);
            if !zip_entry_name_ok(&dir_path) {
                warn!("export: skipping folder entry '{dir_path}' (invalid path components)");
                continue;
            }
            ensure_dir_in_zip(&mut zip_writer, &mut created_dirs, &dir_path, options)?;
        }
    }

    // Now process each file
    let mut exported = 0usize;
    let mut total_bytes = 0u64;
    for row in file_rows {
        let user = match UserRepository::find_by_id(
            state.db.pool(),
            Uuid::parse_str(&row.user_id).unwrap_or_default(),
        )
        .await
        {
            Ok(Some(u)) => u,
            _ => continue,
        };

        // Resolve folder path
        let folder_path = row
            .folder_id
            .as_ref()
            .map(|fid| resolve_folder_path(fid, folder_map));

        // Build zip entry path: bucket_name / username / [folder_path] / file_name
        let entry_path = match &folder_path {
            Some(fp) => format!("{}/{}/{}/{}", bucket_name, user.username, fp, row.original_name),
            None => format!("{}/{}/{}", bucket_name, user.username, row.original_name),
        };

        // Defense-in-depth: legacy DB rows may hold names that are not
        // valid path components; those must never reach the archive.
        if !zip_entry_name_ok(&entry_path) {
            warn!(
                "export: skipping '{}' (invalid path components)",
                entry_path
            );
            continue;
        }

        // Ensure directory exists
        if let Some(parent) = std::path::Path::new(&entry_path).parent() {
            let parent_str = parent.to_string_lossy().replace('\\', "/");
            ensure_dir_in_zip(&mut zip_writer, &mut created_dirs, &parent_str, options)?;
        }

        // Read physical file data from storage backend
        let file_id = Uuid::parse_str(&row.file_id).unwrap_or_default();
        let storage_objects =
            StorageObjectRepository::find_by_file_id(state.db.pool(), file_id).await?;
        let storage_obj = match storage_objects.first() {
            Some(obj) => obj,
            None => continue,
        };

        let backend = {
            let storage = state.storage.read().await;
            storage
                .get(&storage_obj.backend)
                .ok_or_else(|| AppError::Internal(format!("storage backend '{}' not found", storage_obj.backend)))?
        };

        let data = match backend.get(&storage_obj.storage_path).await {
            Ok(Some(d)) => d,
            _ => continue,
        };

        // Add file to zip
        zip_writer
            .start_file(&entry_path, options)
            .map_err(|e| AppError::Internal(format!("failed to start zip entry: {e}")))?;
        zip_writer
            .write_all(&data)
            .map_err(|e| AppError::Internal(format!("failed to write zip entry: {e}")))?;
        exported += 1;
        total_bytes += data.len() as u64;
    }

    // Finalize zip and flush the buffered writer to disk.
    let _ = zip_writer
        .finish()
        .map_err(|e| AppError::Internal(format!("failed to finalize zip: {e}")))?
        .flush()
        .map_err(|e| AppError::Internal(format!("failed to flush zip: {e}")))?;

    Ok((exported, total_bytes))
}
