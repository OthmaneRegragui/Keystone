use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use uuid::Uuid;

use crate::api::extractors::{AuthUser, BotCapability};
use crate::dto::*;
use crate::error::{AppError, AppResult};
use crate::db::repos::{ShareRepository, FolderRepository, UserFileRepository, FileRepository, GroupRepository, AdminSettingRepository, StorageObjectRepository};
use crate::db::rows::share_row::CreateSharedItemData;
use crate::models::share::SharedItemType;
use crate::models::StorageObject;
use crate::AppState;

/// Hard cap on recipients per share request. Without it, one request could
/// trigger an unbounded number of email lookups and share inserts (minor DoS).
const MAX_SHARE_RECIPIENTS: usize = 100;

/// Share files or folders with other users by email.
/// POST /api/share
pub async fn share_item(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Json(body): Json<ShareItemRequest>,
) -> AppResult<Json<ShareResultDto>> {
    auth_user.require_scope("files:write")?;

    if body.emails.len() > MAX_SHARE_RECIPIENTS {
        return Err(AppError::BadRequest(format!(
            "too many recipients: at most {MAX_SHARE_RECIPIENTS} emails per share request"
        )));
    }

    let item_type = SharedItemType::parse(&body.item_type)
        .ok_or_else(|| AppError::BadRequest("item_type must be 'file' or 'folder'".into()))?;

    // Check sharing permission
    if !auth_user.is_admin() {
        let global_allowed = AdminSettingRepository::get_bool(state.db.pool(), "allow_user_sharing")
            .await
            .unwrap_or(true);
        if !global_allowed {
            return Err(AppError::Forbidden("sharing is disabled by administrator".into()));
        }

        let user_groups = GroupRepository::list_user_groups(state.db.pool(), &auth_user.user_id.to_string()).await.unwrap_or_default();
        if user_groups.is_empty() {
            return Err(AppError::Forbidden("you must be in a group to share files".into()));
        }

        let mut has_sharing_perm = false;
        for gid in &user_groups {
            if let Ok(Some(group)) = GroupRepository::get_by_id(state.db.pool(), gid).await {
                if group.allow_sharing {
                    has_sharing_perm = true;
                    break;
                }
            }
        }
        if !has_sharing_perm {
            return Err(AppError::Forbidden("your group does not have sharing permission".into()));
        }
    }

    // Verify the item exists and belongs to the sharer
    match item_type {
        SharedItemType::File => {
            let uf = UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, body.item_id).await?;
            if uf.is_none() {
                return Err(AppError::NotFound("file not found".into()));
            }
        }
        SharedItemType::Folder => {
            let folder = FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, body.item_id).await?;
            if folder.is_none() {
                return Err(AppError::NotFound("folder not found".into()));
            }
        }
    }

    let mut shared_count = 0usize;
    let mut failed_emails = Vec::new();

    for email in &body.emails {
        let email = email.trim().to_lowercase();
        if email.is_empty() || !email.contains('@') {
            failed_emails.push(email);
            continue;
        }

        // Do not reveal whether an email is registered. Every syntactically
        // valid, non-self, non-duplicate share attempt looks identical to the
        // caller: a registered recipient silently gets the share and a missing
        // recipient is treated the same as a duplicate or self-share. Only
        // structurally invalid emails are echoed back. This prevents the
        // endpoint from being used as an email-enumeration oracle.
        let recipient_id = match ShareRepository::find_user_id_by_email(state.db.pool(), &email).await? {
            Some(id) => id,
            None => continue,
        };

        // Cannot share with yourself.
        if recipient_id == auth_user.user_id {
            continue;
        }

        match item_type {
            SharedItemType::File => {
                let data = CreateSharedItemData::new(
                    auth_user.user_id,
                    recipient_id,
                    SharedItemType::File,
                    body.item_id,
                );
                // Only count a share as successful when a new row was actually
                // inserted (an existing/duplicate share is not a new one).
                if ShareRepository::create(state.db.pool(), data).await? {
                    shared_count += 1;
                }
            }
            SharedItemType::Folder => {
                let count = ShareRepository::create_folder_shares(
                    state.db.pool(),
                    auth_user.user_id,
                    recipient_id,
                    body.item_id,
                ).await?;
                shared_count += count;
            }
        }
    }

    let message = if shared_count > 0 {
        format!("shared {} item(s) successfully", shared_count)
    } else if !failed_emails.is_empty() {
        "no valid recipients found".to_string()
    } else {
        "nothing to share".to_string()
    };

    Ok(Json(ShareResultDto {
        message,
        shared_count,
        failed_emails,
    }))
}

/// List items shared with the current user.
/// GET /api/shared
pub async fn list_shared(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
) -> AppResult<Json<SharedListDto>> {
    auth_user.require_scope("files:read")?;

    let shared_items = ShareRepository::list_shared_with_user(state.db.pool(), auth_user.user_id).await?;

    let mut folders = Vec::new();
    let mut files = Vec::new();

    for (item, sharer_username, _sharer_email) in shared_items {
        match item.item_type {
            SharedItemType::Folder => {
                // Look up the folder details
                if let Ok(Some(folder)) = FolderRepository::find_by_id(state.db.pool(), item.item_id).await {
                    folders.push(SharedFolderDto {
                        share_id: item.id,
                        folder_id: folder.id,
                        name: folder.name,
                        bucket_name: folder.bucket_name,
                        shared_by_user_id: item.shared_by_user_id,
                        shared_by_username: sharer_username,
                        shared_at: item.created_at,
                    });
                }
            }
            SharedItemType::File => {
                // Look up the file details (the file might be owned by another user).
                // `deleted_at IS NULL` so a previously-shared file that the owner has
                // trashed no longer exposes its metadata here.
                let row = sqlx::query_as::<_, crate::db::rows::UserFileRow>(
                    "SELECT * FROM user_files WHERE id = $1 AND deleted_at IS NULL",
                )
                .bind(item.item_id.to_string())
                .fetch_optional(state.db.pool())
                .await;

                if let Ok(Some(uf_row)) = row {
                    let uf = crate::models::UserFile::from(uf_row);
                    // Get the physical file info
                    if let Ok(Some(physical)) = FileRepository::find_by_id(state.db.pool(), uf.file_id).await {
                        files.push(SharedFileDto {
                            share_id: item.id,
                            user_file_id: uf.id,
                            name: uf.original_name.clone(),
                            hash: physical.blake3_hash.clone(),
                            size: physical.size,
                            mime_type: uf.mime_type.clone(),
                            bucket_name: uf.bucket_name.clone(),
                            folder_id: uf.folder_id,
                            shared_by_user_id: item.shared_by_user_id,
                            shared_by_username: sharer_username,
                            shared_at: item.created_at,
                        });
                    }
                }
            }
        }
    }

    Ok(Json(SharedListDto { folders, files }))
}

/// List who an item is shared with.
/// GET /api/share/:item_type/:item_id
pub async fn list_shares_of_item(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path((item_type_str, item_id)): Path<(String, Uuid)>,
) -> AppResult<Json<ShareInfoListDto>> {
    auth_user.require_scope("files:read")?;

    let item_type = SharedItemType::parse(&item_type_str)
        .ok_or_else(|| AppError::BadRequest("item_type must be 'file' or 'folder'".into()))?;

    // Verify the item belongs to the current user
    match item_type {
        SharedItemType::File => {
            UserFileRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, item_id)
                .await?
                .ok_or_else(|| AppError::NotFound("file not found".into()))?;
        }
        SharedItemType::Folder => {
            FolderRepository::find_by_user_and_id(state.db.pool(), auth_user.user_id, item_id)
                .await?
                .ok_or_else(|| AppError::NotFound("folder not found".into()))?;
        }
    }

    let shares = ShareRepository::list_shares_of_item(state.db.pool(), item_type, item_id).await?;

    let share_infos: Vec<ShareInfoDto> = shares
        .into_iter()
        .map(|(item, username, email)| ShareInfoDto {
            share_id: item.id,
            user_id: item.shared_with_user_id,
            username,
            email,
            shared_at: item.created_at,
        })
        .collect();

    Ok(Json(ShareInfoListDto { shares: share_infos }))
}

/// Remove a share (only the sharer can remove their own share).
/// DELETE /api/share/:id
pub async fn delete_share(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(share_id): Path<Uuid>,
) -> AppResult<Json<MessageResponse>> {
    auth_user.require_scope("files:write")?;

    let deleted = ShareRepository::delete_share(state.db.pool(), share_id, auth_user.user_id).await?;
    if !deleted {
        return Err(AppError::NotFound("share not found or not owned by you".into()));
    }

    Ok(Json(MessageResponse {
        message: "share removed".to_string(),
    }))
}

/// Download a file shared with the current user.
/// GET /api/shared/file/:id/download
pub async fn download_shared_file(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(user_file_id): Path<Uuid>,
) -> AppResult<Response> {
    auth_user.require_scope("files:read")?;
    auth_user.require_bot_capability(BotCapability::Download)?;

    // Verify the caller can access this file: either the file is directly
    // shared with them, or it lives inside a folder that is shared with them
    // (matching how `shared_folder_contents` authorises listing, so a file the
    // recipient can see in a shared folder is also downloadable).
    let is_shared = ShareRepository::is_shared_with(
        state.db.pool(),
        SharedItemType::File,
        user_file_id,
        auth_user.user_id,
    )
    .await?;

    if !is_shared {
        // The file is not directly shared — fall back to checking whether any
        // ancestor folder of this file is shared with the recipient.
        let user_file = UserFileRepository::find_by_id(state.db.pool(), user_file_id).await?;
        let via_folder = match user_file.as_ref().and_then(|uf| uf.folder_id) {
            Some(folder_id) => is_shared_or_ancestor_shared(
                state.db.pool(),
                auth_user.user_id,
                folder_id,
            )
            .await?,
            None => false,
        };
        if !via_folder {
            return Err(AppError::NotFound("file not found or not shared with you".into()));
        }
    }

    // Look up the file
    let user_file = UserFileRepository::find_by_id(state.db.pool(), user_file_id).await?
        .ok_or_else(|| AppError::NotFound("file not found".into()))?;

    let file = FileRepository::find_by_id(state.db.pool(), user_file.file_id).await?
        .ok_or_else(|| AppError::Internal("physical file not found".into()))?;

    let storage_objects = StorageObjectRepository::find_by_file_id(state.db.pool(), file.id).await?;
    // Prefer the copy in this file's own bucket, then any registered backend —
    // buckets keep independent physical copies, so never serve from a backend
    // that no longer exists.
    let storage_obj = {
        let storage = state.storage.read().await;
        let registered = |so: &StorageObject| storage.get(&so.backend).is_some();
        user_file
            .bucket_name
            .as_deref()
            .and_then(|bn| storage_objects.iter().find(|so| so.backend == bn && registered(so)))
            .or_else(|| storage_objects.iter().find(|so| registered(so)))
            .or_else(|| storage_objects.first())
            .cloned()
    }
    .ok_or_else(|| AppError::NotFound("file not found in storage".into()))?;

    let backend = {
        let storage = state.storage.read().await;
        storage.get(&storage_obj.backend)
            .ok_or_else(|| AppError::Internal("storage backend not found".into()))?
    };

    let data = backend.get(&storage_obj.storage_path).await
        .map_err(|e| AppError::Storage(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("file data not found in storage".into()))?;

    let content_type = user_file.mime_type.unwrap_or_else(|| "application/octet-stream".to_string());

    let mut headers = HeaderMap::new();
    headers.insert(
        "content-type",
        content_type.parse().unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );

    let safe_name: String = user_file.original_name.chars()
        .filter(|c| c.is_ascii() && !c.is_control() && *c != '"' && *c != '\\')
        .collect();
    let safe_name = if safe_name.is_empty() { "download".to_string() } else { safe_name };

    headers.insert(
        "content-disposition",
        format!("attachment; filename=\"{safe_name}\"")
            .parse()
            .map_err(|_| AppError::Internal("invalid content-disposition header".into()))?,
    );
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    headers.insert("content-length", data.len().into());

    Ok((StatusCode::OK, headers, data).into_response())
}

/// Browse contents of a shared folder.
/// GET /api/shared/folder/:id/contents
pub async fn shared_folder_contents(
    State(state): State<Arc<AppState>>,
    auth_user: AuthUser,
    Path(folder_id): Path<Uuid>,
) -> AppResult<Json<FolderContentDto>> {
    auth_user.require_scope("files:read")?;

    // Verify the folder exists
    FolderRepository::find_by_id(state.db.pool(), folder_id).await?
        .ok_or_else(|| AppError::NotFound("folder not found".into()))?;

    // Verify the folder is shared with the user (directly or via ancestor)
    let is_shared = is_shared_or_ancestor_shared(
        state.db.pool(),
        auth_user.user_id,
        folder_id,
    ).await?;

    if !is_shared {
        return Err(AppError::NotFound("folder not found or not shared with you".into()));
    }

    // Find the top-level shared ancestor to get the sharer's info
    let sharer_info = find_top_level_share_sharer(state.db.pool(), auth_user.user_id, folder_id).await?;

    // List child folders (unscoped)
    let folders = FolderRepository::list_children_unscoped(
        state.db.pool(),
        Some(folder_id),
    ).await?;

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
                is_shared: false,
                shared_by_username: sharer_info.as_ref().map(|s| s.1.clone()),
                shared_at: sharer_info.as_ref().map(|s| s.0),
            });
        }
        dtos
    };

    // List files in this folder (unscoped)
    let raw_files = UserFileRepository::list_in_folder_unscoped(
        state.db.pool(),
        folder_id,
    ).await?;

    let file_dtos: Vec<FileDto> = {
        let mut dtos = Vec::new();
        for (uf, hash, size, ref_count) in raw_files {
            dtos.push(FileDto {
                id: uf.file_id,
                user_file_id: uf.id,
                name: uf.original_name,
                hash,
                size,
                mime_type: uf.mime_type,
                created_at: uf.created_at,
                ref_count,
                bucket_name: uf.bucket_name,
                folder_id: uf.folder_id,
                is_shared: false,
                shared_by_username: sharer_info.as_ref().map(|s| s.1.clone()),
                shared_at: sharer_info.as_ref().map(|s| s.0),
            });
        }
        dtos
    };

    // Build breadcrumb path
    let chain = FolderRepository::get_path(state.db.pool(), folder_id).await?;
    let mut breadcrumbs = vec![FolderBreadcrumb { id: None, name: "Shared".into() }];
    for (id, name) in chain {
        breadcrumbs.push(FolderBreadcrumb { id: Some(id), name });
    }

    Ok(Json(FolderContentDto {
        folders: folder_dtos,
        files: file_dtos,
        path: breadcrumbs,
    }))
}

/// Check if a folder is shared with the user, either directly or via an ancestor folder.
async fn is_shared_or_ancestor_shared(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    folder_id: Uuid,
) -> AppResult<bool> {
    // Walk up the folder tree checking if any ancestor is shared
    let mut current = Some(folder_id);
    // Guard against parent_id cycles so a corrupted tree cannot loop forever.
    let mut visited = std::collections::HashSet::new();
    while let Some(fid) = current {
        if !visited.insert(fid) {
            break;
        }
        if ShareRepository::is_shared_with(pool, SharedItemType::Folder, fid, user_id).await? {
            return Ok(true);
        }
        // Walk to parent
        if let Some(folder) = FolderRepository::find_by_id(pool, fid).await? {
            current = folder.parent_id;
        } else {
            break;
        }
    }
    Ok(false)
}

/// Walk up the folder tree to find the top-level shared ancestor, then return
/// (shared_at, sharer_username) so child items can display sharing metadata.
async fn find_top_level_share_sharer(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    folder_id: Uuid,
) -> AppResult<Option<(chrono::DateTime<chrono::Utc>, String)>> {
    // Collect the chain of folder IDs from current up to root
    let mut chain = Vec::new();
    let mut current = Some(folder_id);
    // Guard against parent_id cycles so a corrupted tree cannot loop forever.
    let mut visited = std::collections::HashSet::new();
    while let Some(fid) = current {
        if !visited.insert(fid) {
            break;
        }
        chain.push(fid);
        if let Some(folder) = FolderRepository::find_by_id(pool, fid).await? {
            current = folder.parent_id;
        } else {
            break;
        }
    }

    // Walk from root (last in chain) down to find the first shared ancestor
    for fid in chain.iter().rev() {
        if let Some((item, sharer_username, _sharer_email)) =
            ShareRepository::find_folder_share_for_user(pool, *fid, user_id).await?
        {
            return Ok(Some((item.created_at, sharer_username)));
        }
    }
    Ok(None)
}
