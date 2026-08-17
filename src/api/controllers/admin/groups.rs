use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::{GroupRepository, UserRepository};
use tracing::info;
use uuid::Uuid;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

/// Parse a UUID coming from a request body. Without this, invalid strings
/// flow into sqlx `TEXT` binds and fail the FK/join later as a 500 instead
/// of a clean 400.
fn require_uuid(v: &str, what: &str) -> Result<(), AppError> {
    if Uuid::parse_str(v).is_err() {
        return Err(AppError::BadRequest(format!("invalid {what} id '{v}'")));
    }
    Ok(())
}

/// Reject negative per-user storage limits before they reach the DB.
fn validate_user_storage_limit(limit: i64) -> Result<(), AppError> {
    if limit < 0 {
        return Err(AppError::BadRequest(
            "user storage limit cannot be negative".into(),
        ));
    }
    Ok(())
}

pub async fn list_groups(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<GroupDto>>> {
    auth.require_admin()?;
    let groups = GroupRepository::list(state.db.pool()).await?;
    let mut result = Vec::new();
    for g in groups {
        let members = GroupRepository::list_members(state.db.pool(), &g.id).await.unwrap_or_default();
        let buckets = GroupRepository::list_buckets(state.db.pool(), &g.id).await.unwrap_or_default();
        result.push(GroupDto::from_group(g, members.len() as i64, buckets.len() as i64));
    }
    Ok(Json(result))
}

pub async fn create_group(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateGroupRequest>,
) -> AppResult<Json<GroupDto>> {
    auth.require_admin()?;
    if body.name.is_empty() {
        return Err(AppError::Validation("group name is required".into()));
    }
    if body.name.len() > 255 {
        return Err(AppError::Validation(
            "group name is too long (max 255 characters)".into(),
        ));
    }
    if body.name.chars().any(|c| c.is_control()) {
        return Err(AppError::Validation(
            "group name cannot contain control characters".into(),
        ));
    }
    let group = GroupRepository::create(state.db.pool(), &body.name).await?;

    // Link buckets if provided
    let bucket_count = if let Some(buckets) = &body.buckets {
        let mut count = 0i64;
        for assignment in buckets {
            require_uuid(&assignment.bucket_id, "bucket")?;
            let limit = assignment.user_storage_limit.unwrap_or(0);
            validate_user_storage_limit(limit)?;
            GroupRepository::add_bucket(state.db.pool(), &group.id, &assignment.bucket_id, limit).await?;
            count += 1;
        }
        count
    } else {
        0
    };

    info!("admin {} created group '{}' with {} buckets", auth.username, body.name, bucket_count);
    Ok(Json(GroupDto::from_group(group, 0, bucket_count)))
}

pub async fn delete_group(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<DeleteGroupRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.id, "group")?;
    GroupRepository::delete(state.db.pool(), &body.id).await?;
    info!("admin {} deleted group {}", auth.username, body.id);
    Ok(Json(MessageResponse { message: "group deleted".to_string() }))
}

pub async fn get_group_detail(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> AppResult<Json<GroupDetailDto>> {
    auth.require_admin()?;
    let id = params.get("id").ok_or_else(|| AppError::BadRequest("missing id param".into()))?;
    let group = GroupRepository::get_by_id(state.db.pool(), id).await?
        .ok_or_else(|| AppError::NotFound("group not found".into()))?;

    let member_ids = GroupRepository::list_members(state.db.pool(), id).await.unwrap_or_default();
    let mut members = Vec::new();
    for mid in &member_ids {
        if let Ok(uid) = Uuid::parse_str(mid) {
            if let Ok(Some(user)) = UserRepository::find_by_id(state.db.pool(), uid).await {
                members.push(AdminUserDto {
                    id: user.id.to_string(),
                    username: user.username,
                    email: user.email,
                    role: user.role.to_string(),
                    storage_quota: user.storage_quota,
                    storage_used: user.storage_used,
                    created_at: user.created_at,
                    group_ids: vec![],
                });
            }
        }
    }

    let bucket_details = GroupRepository::list_group_bucket_details(state.db.pool(), id).await.unwrap_or_default();
    let buckets = bucket_details
        .into_iter()
        .map(|(id, name, path, storage_used, storage_limit, user_storage_limit, user_count, can_upload, can_download)| GroupBucketDto {
            id,
            name,
            path,
            storage_used,
            storage_limit,
            user_storage_limit,
            user_count,
            can_upload,
            can_download,
        })
        .collect();

    Ok(Json(GroupDetailDto {
        id: group.id.clone(),
        name: group.name.clone(),
        members,
        buckets,
        allow_api_keys: group.allow_api_keys,
        allow_password_change: group.allow_password_change,
        allow_bots: group.allow_bots,
    }))
}

pub async fn add_group_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<GroupMemberRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.user_id, "user")?;
    GroupRepository::add_member(state.db.pool(), &body.group_id, &body.user_id).await?;
    Ok(Json(MessageResponse { message: "member added".to_string() }))
}

pub async fn add_bulk_group_members(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<BulkGroupMembershipRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    if body.user_ids.is_empty() || body.group_ids.is_empty() {
        return Err(AppError::BadRequest(
            "user_ids and group_ids must not be empty".to_string(),
        ));
    }
    // Cap the cross-product so a single request cannot fan out into an
    // unbounded number of INSERTs (the repo loops over both lists).
    const MAX_BULK_IDS: usize = 10_000;
    const MAX_BULK_MEMBERSHIPS: usize = 1_000_000;
    if body.user_ids.len() > MAX_BULK_IDS || body.group_ids.len() > MAX_BULK_IDS {
        return Err(AppError::BadRequest(format!(
            "too many ids: at most {MAX_BULK_IDS} users and {MAX_BULK_IDS} groups per request"
        )));
    }
    if body.user_ids.len().saturating_mul(body.group_ids.len()) > MAX_BULK_MEMBERSHIPS {
        return Err(AppError::BadRequest(format!(
            "too many user-group combinations (max {MAX_BULK_MEMBERSHIPS})"
        )));
    }
    for uid in &body.user_ids {
        require_uuid(uid, "user")?;
    }
    for gid in &body.group_ids {
        require_uuid(gid, "group")?;
    }
    let added = GroupRepository::add_members_to_groups(state.db.pool(), &body.user_ids, &body.group_ids).await?;
    info!(
        "admin {} granted {} user(s) access to {} group(s) ({} new memberships)",
        auth.username,
        body.user_ids.len(),
        body.group_ids.len(),
        added
    );
    Ok(Json(MessageResponse { message: format!("{added} membership(s) added") }))
}

pub async fn remove_group_member(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<RemoveGroupMemberRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.user_id, "user")?;
    GroupRepository::remove_member(state.db.pool(), &body.group_id, &body.user_id).await?;
    Ok(Json(MessageResponse { message: "member removed".to_string() }))
}

pub async fn add_group_bucket(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<GroupBucketRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.bucket_id, "bucket")?;
    let limit = body.user_storage_limit.unwrap_or(0);
    validate_user_storage_limit(limit)?;
    GroupRepository::add_bucket(state.db.pool(), &body.group_id, &body.bucket_id, limit).await?;
    Ok(Json(MessageResponse { message: "bucket linked to group".to_string() }))
}

pub async fn remove_group_bucket(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<RemoveGroupBucketRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.bucket_id, "bucket")?;
    GroupRepository::remove_bucket(state.db.pool(), &body.group_id, &body.bucket_id).await?;
    Ok(Json(MessageResponse { message: "bucket unlinked from group".to_string() }))
}

pub async fn update_group_bucket_permissions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateGroupBucketPermissionsRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.bucket_id, "bucket")?;
    GroupRepository::update_bucket_permissions(
        state.db.pool(),
        &body.group_id,
        &body.bucket_id,
        body.can_upload,
        body.can_download,
    ).await?;
    Ok(Json(MessageResponse { message: "permissions updated".to_string() }))
}

pub async fn set_group_bucket_user_limit(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<SetGroupBucketUserLimitRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    require_uuid(&body.bucket_id, "bucket")?;
    validate_user_storage_limit(body.user_storage_limit)?;
    GroupRepository::set_user_storage_limit(
        state.db.pool(),
        &body.group_id,
        &body.bucket_id,
        body.user_storage_limit,
    ).await?;
    info!(
        "admin {} set user storage limit for group {} bucket {} to {}",
        auth.username, body.group_id, body.bucket_id, body.user_storage_limit
    );
    Ok(Json(MessageResponse { message: "user storage limit updated".to_string() }))
}

pub async fn update_group_permissions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateGroupPermissionsRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    require_uuid(&body.group_id, "group")?;
    GroupRepository::update_permissions(
        state.db.pool(),
        &body.group_id,
        body.allow_api_keys,
        body.allow_password_change,
        body.allow_bots,
    ).await?;
    info!(
        "admin {} updated group {} permissions: allow_api_keys={}, allow_password_change={}, allow_bots={}",
        auth.username, body.group_id, body.allow_api_keys, body.allow_password_change, body.allow_bots
    );
    Ok(Json(MessageResponse { message: "group permissions updated".to_string() }))
}
