use chrono::{DateTime, Utc};
use crate::models::{File, UserFile};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct FileDto {
    pub id: Uuid,
    /// The user-specific file ID (user_files.id) used for API operations.
    pub user_file_id: Uuid,
    /// The user's original filename.
    pub name: String,
    /// The content hash (blake3).
    pub hash: String,
    pub size: i64,
    /// The user's mime type.
    pub mime_type: Option<String>,
    /// When the user uploaded this file.
    pub created_at: DateTime<Utc>,
    /// How many users reference the underlying physical file.
    pub ref_count: i32,
    /// The bucket this file belongs to.
    pub bucket_name: Option<String>,
    /// The virtual folder this file is in (None = root).
    pub folder_id: Option<Uuid>,
    /// Whether this file has been shared with other users.
    pub is_shared: bool,
    /// Username of the sharer (populated when browsing shared folder contents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_by_username: Option<String>,
    /// When the item was shared (populated when browsing shared folder contents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_at: Option<DateTime<Utc>>,
}

/// Build a FileDto from a UserFile + physical File metadata.
pub fn file_dto_from_user_file(user_file: &UserFile, file: &File) -> FileDto {
    FileDto {
        id: file.id,
        user_file_id: user_file.id,
        name: user_file.original_name.clone(),
        hash: file.blake3_hash.clone(),
        size: file.size,
        mime_type: user_file.mime_type.clone(),
        created_at: user_file.created_at,
        ref_count: file.ref_count,
        bucket_name: user_file.bucket_name.clone(),
        folder_id: user_file.folder_id,
        is_shared: false,
        shared_by_username: None,
        shared_at: None,
    }
}

#[derive(Debug, Serialize)]
pub struct FileListDto {
    pub files: Vec<FileDto>,
    pub total: i64,
    pub page: u32,
    pub per_page: u32,
}

#[derive(Debug, Serialize)]
pub struct UploadResponse {
    pub file: FileDto,
    pub duplicate: bool,
}

// ── Dashboard DTOs ──

/// Aggregate stats and recent activity for the signed-in user's dashboard.
#[derive(Debug, Serialize)]
pub struct DashboardStatsDto {
    pub total_files: i64,
    pub storage_used: i64,
    pub duplicates_saved: i64,
    pub quota_bytes: i64,
    pub recent_files: Vec<FileDto>,
}

#[derive(Debug, Serialize)]
pub struct UserBucketDto {
    pub id: String,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub user_storage_limit: i64,
}

// ── Folder DTOs ──

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub bucket_name: String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct RenameFolderRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameFileRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct MoveFileRequest {
    pub folder_id: Option<Uuid>,
    pub bucket_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CopyFileRequest {
    pub folder_id: Option<Uuid>,
    pub bucket_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchMoveRequest {
    pub file_ids: Vec<Uuid>,
    pub folder_id: Option<Uuid>,
    pub bucket_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchCopyRequest {
    pub file_ids: Vec<Uuid>,
    pub folder_id: Option<Uuid>,
    pub bucket_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchDeleteRequest {
    pub file_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct BatchResultResponse {
    pub success: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FolderDto {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub bucket_name: String,
    pub created_at: DateTime<Utc>,
    pub file_count: i64,
    pub folder_count: i64,
    /// Whether this folder has been shared with other users.
    pub is_shared: bool,
    /// Username of the sharer (populated when browsing shared folder contents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_by_username: Option<String>,
    /// When the item was shared (populated when browsing shared folder contents).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct FolderContentDto {
    pub folders: Vec<FolderDto>,
    pub files: Vec<FileDto>,
    pub path: Vec<FolderBreadcrumb>,
}

#[derive(Debug, Serialize)]
pub struct FolderBreadcrumb {
    pub id: Option<Uuid>,
    pub name: String,
}

/// Flat folder list for building a tree on the client side.
#[derive(Debug, Serialize)]
pub struct FolderTreeItem {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct FolderTreeDto {
    pub folders: Vec<FolderTreeItem>,
}

/// Response for `GET /api/folders/resolve`.
#[derive(Debug, Serialize)]
pub struct FolderResolveDto {
    pub folder_id: Uuid,
    pub path: Vec<FolderBreadcrumb>,
}

#[derive(Debug, Deserialize)]
pub struct MoveFolderRequest {
    pub folder_id: Option<Uuid>,
}

// ── Trash DTOs ──

/// A trashed folder entry.
#[derive(Debug, Serialize)]
pub struct TrashFolderDto {
    pub folder_id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub bucket_name: String,
    pub deleted_at: DateTime<Utc>,
}

/// A trashed file entry.
#[derive(Debug, Serialize)]
pub struct TrashFileDto {
    pub user_file_id: Uuid,
    pub name: String,
    pub hash: String,
    pub size: i64,
    pub mime_type: Option<String>,
    pub deleted_at: DateTime<Utc>,
    pub bucket_name: Option<String>,
    pub folder_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct TrashListDto {
    pub folders: Vec<TrashFolderDto>,
    pub files: Vec<TrashFileDto>,
}

#[derive(Debug, Deserialize)]
pub struct RestoreTrashRequest {
    /// Optional destination folder_id within the same bucket. None = restore to original location.
    pub folder_id: Option<Uuid>,
}

// ── Sharing DTOs ──

#[derive(Debug, Deserialize)]
pub struct ShareItemRequest {
    /// Email addresses of users to share with.
    pub emails: Vec<String>,
    /// "file" or "folder".
    pub item_type: String,
    /// The user_file_id or folder_id to share.
    pub item_id: Uuid,
}

/// A folder shared with the current user.
#[derive(Debug, Serialize)]
pub struct SharedFolderDto {
    pub share_id: Uuid,
    pub folder_id: Uuid,
    pub name: String,
    pub bucket_name: String,
    pub shared_by_user_id: Uuid,
    pub shared_by_username: String,
    pub shared_at: chrono::DateTime<chrono::Utc>,
}

/// A file shared with the current user.
#[derive(Debug, Serialize)]
pub struct SharedFileDto {
    pub share_id: Uuid,
    pub user_file_id: Uuid,
    pub name: String,
    pub hash: String,
    pub size: i64,
    pub mime_type: Option<String>,
    pub bucket_name: Option<String>,
    pub folder_id: Option<Uuid>,
    pub shared_by_user_id: Uuid,
    pub shared_by_username: String,
    pub shared_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct SharedListDto {
    pub folders: Vec<SharedFolderDto>,
    pub files: Vec<SharedFileDto>,
}

/// Who an item is shared with (for the share management UI on /files).
#[derive(Debug, Serialize)]
pub struct ShareInfoDto {
    pub share_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub email: String,
    pub shared_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
pub struct ShareInfoListDto {
    pub shares: Vec<ShareInfoDto>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteShareRequest {
    pub share_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct ShareResultDto {
    pub message: String,
    pub shared_count: usize,
    pub failed_emails: Vec<String>,
}
