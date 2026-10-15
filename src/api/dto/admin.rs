use chrono::{DateTime, Utc};
use crate::error::{AppError, AppResult};
use crate::models::{BotPathRule, Bucket, Connection, PlatformSettings, StoragePath, UserGroup};
use serde::{Deserialize, Serialize};

fn default_true() -> bool { true }

// ─── Requests (input) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdateSettingRequest {
    pub key: String,
    pub value: String,
}

/// Store the Cloudflare Turnstile keys used by the login captcha.
///
/// The secret is **write-only**: it is accepted here but never returned by any
/// endpoint, and omitting it keeps the stored value unchanged so the admin form
/// can be re-saved without holding the secret in the browser.
#[derive(Debug, Deserialize)]
pub struct UpdateCaptchaSettingRequest {
    pub enabled: bool,
    #[serde(default)]
    pub site_key: String,
    /// `None` leaves the stored secret untouched, `Some("")` clears it.
    #[serde(default)]
    pub secret_key: Option<String>,
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
    pub allow_bots: bool,
    #[serde(default)]
    pub allow_sharing: bool,
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

/// A single orphaned physical file in the admin detail view.
#[derive(Debug, Serialize)]
pub struct OrphanedFileDto {
    pub id: String,
    pub hash: String,
    pub name: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub bucket: Option<String>,
    pub deleted_at: Option<String>,
    pub owner: Option<String>,
}

/// Page of orphaned physical files for the admin UI.
#[derive(Debug, Serialize)]
pub struct OrphanedFilesDto {
    pub total: i64,
    pub total_size_bytes: i64,
    pub files: Vec<OrphanedFileDto>,
}

/// A distinct owner (user) among orphaned files, for filter dropdowns.
#[derive(Debug, Serialize)]
pub struct OrphanedOwnerDto {
    pub id: String,
    pub username: String,
}

/// Distinct buckets and owners among orphaned files, for filter dropdowns.
#[derive(Debug, Serialize)]
pub struct OrphanedFacetsDto {
    pub buckets: Vec<String>,
    pub users: Vec<OrphanedOwnerDto>,
}

/// Result of purging orphaned physical files.
#[derive(Debug, Serialize)]
pub struct OrphanedDeleteResultDto {
    pub deleted: usize,
    pub failed: usize,
    pub errors: Vec<String>,
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
    pub allow_bots: bool,
    pub allow_sharing: bool,
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
            allow_bots: group.allow_bots,
            allow_sharing: group.allow_sharing,
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
    pub allow_bots: bool,
    pub allow_sharing: bool,
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

// ─── Software Update ─────────────────────────────────────────

/// Details about the newest published release (from GitHub).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateReleaseDto {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub published_at: Option<String>,
    pub is_prerelease: bool,
}

/// Result of checking for a newer Keystone release.
#[derive(Debug, Serialize)]
pub struct UpdateCheckDto {
    /// Version of the currently running server (e.g. "0.4.0").
    pub current_version: String,
    /// The newest release found, if the update source could be reached.
    pub latest: Option<UpdateReleaseDto>,
    /// True when `latest` is set and newer than `current_version`.
    pub update_available: bool,
    /// True when the server has a working way to pull and restart itself, so
    /// the panel can offer a one-click "Update & Restart" button instead of
    /// only linking to the release page. False when the operator has not set up
    /// a host watcher (request folder) or an apply hook.
    pub can_apply: bool,
    /// Human-readable error when the check failed (e.g. GitHub unreachable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of requesting an update/restart through the configured mechanism.
#[derive(Debug, Serialize)]
pub struct UpdateApplyDto {
    pub started: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct AdminBotDto {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub key_id: String,
    pub prefix: String,
    pub name: String,
    pub can_upload: bool,
    pub can_download: bool,
    pub can_copy: bool,
    pub can_edit: bool,
    pub can_delete: bool,
    pub can_list: bool,
    pub path_rules: Option<Vec<BotPathRule>>,
    pub upload_limit_bytes: i64,
    pub uploaded_bytes: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAdminBotRequest {
    pub user_id: String,
    pub name: String,
    #[serde(default)]
    pub can_upload: bool,
    #[serde(default)]
    pub can_download: bool,
    #[serde(default)]
    pub can_copy: bool,
    #[serde(default)]
    pub can_edit: bool,
    #[serde(default)]
    pub can_delete: bool,
    #[serde(default = "default_true")]
    pub can_list: bool,
    #[serde(default)]
    pub path_rules: Option<Vec<BotPathRule>>,
    #[serde(default)]
    pub upload_limit_bytes: i64,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct AdminUpdateBotRequest {
    pub name: Option<String>,
    pub can_upload: Option<bool>,
    pub can_download: Option<bool>,
    pub can_copy: Option<bool>,
    pub can_edit: Option<bool>,
    pub can_delete: Option<bool>,
    pub can_list: Option<bool>,
    pub path_rules: Option<Option<Vec<BotPathRule>>>,
    pub upload_limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminBotPathRequest {
    pub id: String,
}

// ─── Connections ─────────────────────────────────────────────

/// A saved remote-server connection, with its type-specific settings flattened
/// for the admin UI. Secrets (the FTP password) are write-only: only whether one
/// is set is ever returned.
#[derive(Debug, Serialize)]
pub struct ConnectionDto {
    pub id: String,
    pub name: String,
    pub connection_type: String,
    pub created_at: DateTime<Utc>,
    pub ftp_host: Option<String>,
    pub ftp_port: i32,
    pub ftp_username: Option<String>,
    pub ftp_base_path: String,
    pub ftp_secure: bool,
    pub ftp_passive: bool,
    pub ftp_verify_cert: bool,
    /// Whether an FTP password is stored. The password itself is write-only and
    /// is never returned by any endpoint.
    pub ftp_password_set: bool,
}

impl ConnectionDto {
    pub fn from_connection(c: Connection) -> Self {
        let ftp = if c.is_ftp() {
            serde_json::from_str::<crate::storage::ftp::FtpConfig>(&c.config).ok()
        } else {
            None
        };
        match ftp {
            Some(cfg) => Self {
                id: c.id,
                name: c.name,
                connection_type: c.connection_type,
                created_at: c.created_at,
                ftp_host: Some(cfg.host),
                ftp_port: cfg.port as i32,
                ftp_username: Some(cfg.username),
                ftp_base_path: cfg.base_path,
                ftp_secure: cfg.secure,
                ftp_passive: cfg.passive,
                ftp_verify_cert: cfg.verify_cert,
                ftp_password_set: !cfg.password.is_empty(),
            },
            None => Self {
                id: c.id,
                name: c.name,
                connection_type: c.connection_type,
                created_at: c.created_at,
                ftp_host: None,
                ftp_port: 21,
                ftp_username: None,
                ftp_base_path: String::new(),
                ftp_secure: false,
                ftp_passive: true,
                ftp_verify_cert: true,
                ftp_password_set: false,
            },
        }
    }
}

/// Create a saved connection. Every field is optional; only the fields relevant
/// to `connection_type` are read. The FTP password is write-only.
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CreateConnectionRequest {
    pub name: String,
    /// Backend kind, e.g. `"ftp"`.
    pub connection_type: String,
    pub ftp_host: Option<String>,
    pub ftp_port: Option<i32>,
    pub ftp_username: Option<String>,
    pub ftp_password: Option<String>,
    pub ftp_base_path: Option<String>,
    pub ftp_secure: Option<bool>,
    pub ftp_passive: Option<bool>,
    pub ftp_verify_cert: Option<bool>,
}

impl CreateConnectionRequest {
    /// Validate this request and resolve it into `(connection_type, config_json)`.
    pub fn to_config(&self) -> AppResult<(String, String)> {
        let connection_type = self.connection_type.trim().to_ascii_lowercase();
        match connection_type.as_str() {
            "ftp" => {
                let host = self.ftp_host.as_deref().unwrap_or("").trim().to_string();
                if host.is_empty() {
                    return Err(AppError::Validation("FTP host is required".into()));
                }
                if host.contains('\0') {
                    return Err(AppError::Validation(
                        "FTP host contains invalid characters".into(),
                    ));
                }
                let port = self.ftp_port.unwrap_or(21);
                if !(1..=65535).contains(&port) {
                    return Err(AppError::Validation(
                        "FTP port must be between 1 and 65535".into(),
                    ));
                }
                let username = self.ftp_username.as_deref().unwrap_or("").trim().to_string();
                if username.is_empty() {
                    return Err(AppError::Validation("FTP username is required".into()));
                }
                let base_path = self.ftp_base_path.clone().unwrap_or_default();
                if base_path.contains('\0') {
                    return Err(AppError::Validation(
                        "FTP base path contains invalid characters".into(),
                    ));
                }
                let config = crate::storage::ftp::FtpConfig {
                    host,
                    port: port as u16,
                    username,
                    password: self.ftp_password.clone().unwrap_or_default(),
                    base_path,
                    secure: self.ftp_secure.unwrap_or(false),
                    passive: self.ftp_passive.unwrap_or(true),
                    verify_cert: self.ftp_verify_cert.unwrap_or(true),
                };
                let json = serde_json::to_string(&config).map_err(|e| {
                    AppError::Internal(format!("failed to serialise FTP connection: {e}"))
                })?;
                Ok((connection_type, json))
            }
            other => Err(AppError::Validation(format!(
                "unknown connection type '{other}'"
            ))),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DeleteConnectionRequest {
    pub id: String,
}

/// Probe a connection: either a saved one (`id`) or the inline settings of one
/// that has not been saved yet. A saved connection is probed server-side with
/// its stored password (which is never sent to the client).
#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CheckConnectionRequest {
    /// Saved connection to probe. When absent, `connection` is used instead.
    pub id: Option<String>,
    #[serde(flatten)]
    pub connection: CreateConnectionRequest,
}

#[derive(Debug, Serialize)]
pub struct ConnectionCheckDto {
    pub ok: bool,
    /// Human-readable failure reason when `ok` is false.
    pub error: Option<String>,
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
    /// Saved connection this path mirrors to, if any. `None` = plain local disk.
    pub connection_id: Option<String>,
    /// Name of the assigned connection, resolved at read time.
    pub connection_name: Option<String>,
    /// Backend kind of the assigned connection, e.g. `"ftp"`.
    pub connection_type: Option<String>,
    pub delete_local_after_upload: bool,
}

impl StoragePathDto {
    pub fn from_path(
        sp: StoragePath,
        connection: Option<&Connection>,
        file_count: i64,
        total_size: i64,
        bucket_count: i64,
    ) -> Self {
        Self {
            id: sp.id,
            name: sp.name,
            path: sp.path,
            file_count,
            total_size,
            bucket_count,
            created_at: sp.created_at,
            connection_id: sp.connection_id,
            connection_name: connection.map(|c| c.name.clone()),
            connection_type: connection.map(|c| c.connection_type.clone()),
            delete_local_after_upload: sp.delete_local_after_upload,
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
    /// Saved connection to assign, or `None` for a plain local path.
    #[serde(default)]
    pub connection_id: Option<String>,
    /// Remove the staged local copy once a remote upload succeeds.
    #[serde(default)]
    pub delete_local_after_upload: bool,
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
