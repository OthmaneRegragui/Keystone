use chrono::{DateTime, Utc};
use crate::models::{Bucket, PlatformSettings, StoragePath, UserGroup};
use serde::{Deserialize, Serialize};

fn default_true() -> bool { true }

// ─── Requests (input) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdateSettingRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateBucketRequest {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SetBucketVisibleRequest {
    pub name: String,
    pub visible: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateBucketRequest {
    pub original_name: String,
    pub name: String,
    pub path: String,
    #[serde(default = "default_true")]
    pub visible_to_users: bool,
    pub is_active: bool,
    pub storage_limit: i64,
}

#[derive(Debug, Deserialize)]
pub struct DeleteBucketRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserQuotaRequest {
    pub user_id: String,
    pub storage_quota: i64,
}

#[derive(Debug, Deserialize)]
pub struct CreateAdminUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub role: String,
    pub group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub buckets: Option<Vec<GroupBucketAssignment>>,
}

#[derive(Debug, Deserialize)]
pub struct GroupBucketAssignment {
    pub bucket_id: String,
    pub user_storage_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAdminApiKeyRequest {
    pub user_id: Option<String>,
    pub name: String,
    pub scopes: Vec<String>,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteGroupRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct GroupMemberRequest {
    pub group_id: String,
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveGroupMemberRequest {
    pub group_id: String,
    pub user_id: String,
}

#[derive(Debug, Deserialize)]
pub struct BulkGroupMembershipRequest {
    pub user_ids: Vec<String>,
    pub group_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GroupBucketRequest {
    pub group_id: String,
    pub bucket_id: String,
    pub user_storage_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct RemoveGroupBucketRequest {
    pub group_id: String,
    pub bucket_id: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupBucketPermissionsRequest {
    pub group_id: String,
    pub bucket_id: String,
    pub can_upload: bool,
    pub can_download: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetGroupBucketUserLimitRequest {
    pub group_id: String,
    pub bucket_id: String,
    pub user_storage_limit: i64,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGroupPermissionsRequest {
    pub group_id: String,
    pub allow_api_keys: bool,
    pub allow_password_change: bool,
}

#[derive(Debug, Deserialize)]
pub struct RevokeApiKeyRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangeBucketPathRequest {
    pub bucket_name: String,
    pub new_path: String,
}

// ─── Response DTOs (output) ──────────────────────────────────
// These are lean HTTP transfer objects derived from core models.

/// Re-export `PlatformSettings` directly — fields are identical.
pub type PlatformSettingsDto = PlatformSettings;

#[derive(Debug, Serialize)]
pub struct BucketDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub is_active: bool,
    pub visible_to_users: bool,
    pub storage_used: i64,
    pub storage_limit: i64,
    /// Number of soft-deleted files in this bucket (physically still on disk).
    pub deleted_files: i64,
    /// Total size of soft-deleted files in this bucket.
    pub deleted_files_size: i64,
    /// Number of fully orphaned physical files (no active user reference in this bucket).
    pub orphaned_files: i64,
    /// Total size of orphaned physical files in this bucket.
    pub orphaned_files_size: i64,
}

impl BucketDto {
    /// Create from a core `Bucket` with explicit storage usage, deleted stats, and orphaned stats.
    pub fn from_bucket(bucket: Bucket, storage_used: i64, deleted_files: i64, deleted_files_size: i64, orphaned_files: i64, orphaned_files_size: i64) -> Self {
        Self {
            id: bucket.id,
            name: bucket.name,
            path: bucket.path,
            is_active: bucket.is_active,
            visible_to_users: bucket.visible_to_users,
            storage_used,
            storage_limit: bucket.storage_limit,
            deleted_files,
            deleted_files_size,
            orphaned_files,
            orphaned_files_size,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AdminStatsDto {
    pub total_users: i64,
    pub total_files: i64,
    pub total_buckets: i64,
    pub total_groups: i64,
    pub block_registrations: bool,
    /// Total number of active (non-deleted) user files.
    pub active_user_files: i64,
    /// Total size in bytes of active user files.
    pub active_user_files_size: i64,
    /// Total number of soft-deleted user files.
    pub deleted_user_files: i64,
    /// Total size in bytes of soft-deleted user files.
    pub deleted_user_files_size: i64,
    /// Number of physical files that are completely orphaned (ALL user references soft-deleted).
    /// These waste disk space but no user can access them.
    pub orphaned_physical_files: i64,
    /// Total size in bytes of orphaned physical files.
    pub orphaned_physical_files_size: i64,
}

#[derive(Debug, Serialize)]
pub struct AdminUserDto {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub storage_quota: i64,
    pub storage_used: i64,
    pub created_at: DateTime<Utc>,
    pub group_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GroupDto {
    pub id: String,
    pub name: String,
    pub member_count: i64,
    pub bucket_count: i64,
    pub allow_api_keys: bool,
    pub allow_password_change: bool,
}

impl GroupDto {
    /// Build from a core `UserGroup` plus pre-fetched counts.
    pub fn from_group(group: UserGroup, member_count: i64, bucket_count: i64) -> Self {
        Self {
            id: group.id,
            name: group.name,
            member_count,
            bucket_count,
            allow_api_keys: group.allow_api_keys,
            allow_password_change: group.allow_password_change,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct GroupBucketDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub storage_used: i64,
    pub storage_limit: i64,
    pub user_storage_limit: i64,
    pub user_count: i64,
    pub can_upload: bool,
    pub can_download: bool,
}

#[derive(Debug, Serialize)]
pub struct GroupDetailDto {
    pub id: String,
    pub name: String,
    pub members: Vec<AdminUserDto>,
    pub buckets: Vec<GroupBucketDto>,
    pub allow_api_keys: bool,
    pub allow_password_change: bool,
}

#[derive(Debug, Serialize)]
pub struct AdminApiKeyDto {
    pub id: String,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub is_active: bool,
}

// ─── Storage Paths ───────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StoragePathDto {
    pub id: String,
    pub name: String,
    pub path: String,
    pub file_count: i64,
    pub total_size: i64,
    pub bucket_count: i64,
    pub created_at: DateTime<Utc>,
}

impl StoragePathDto {
    pub fn from_path(sp: StoragePath, file_count: i64, total_size: i64, bucket_count: i64) -> Self {
        Self {
            id: sp.id,
            name: sp.name,
            path: sp.path,
            file_count,
            total_size,
            bucket_count,
            created_at: sp.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StorageBaseDto {
    /// First directory of STORAGE_LOCAL_PATHS; new storage paths are created under it.
    pub env_base: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateStoragePathRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteStoragePathRequest {
    pub id: String,
}

// ─── Bucket Export ────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BucketExportFileDto {
    pub name: String,
    pub folder: Option<String>,       // full folder path within user's tree, None = root
    pub size: i64,
    pub hash: String,
    pub mime_type: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BucketExportFolderDto {
    pub name: String,
    pub parent: Option<String>,       // parent folder name or None for root
    pub full_path: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BucketExportUserDto {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub files: Vec<BucketExportFileDto>,
    pub folders: Vec<BucketExportFolderDto>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BucketIndexExportDto {
    pub bucket: String,
    pub exported_at: String,
    pub users: Vec<BucketExportUserDto>,
}
