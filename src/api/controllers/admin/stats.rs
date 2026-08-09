use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::AppResult;
use crate::db::repos::{
    AdminSettingRepository, BucketRepository, FileRepository, GroupRepository, UserRepository,
    UserFileRepository,
};

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

pub async fn get_stats(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<AdminStatsDto>> {
    auth.require_admin()?;
    let total_users = UserRepository::count(state.db.pool()).await.unwrap_or(0);
    let total_files = FileRepository::count(state.db.pool(), None).await.unwrap_or(0);
    let total_buckets = BucketRepository::list(state.db.pool()).await.map(|b| b.len() as i64).unwrap_or(0);
    let total_groups = GroupRepository::list(state.db.pool()).await.map(|g| g.len() as i64).unwrap_or(0);
    let active_user_files = UserFileRepository::count_active(state.db.pool()).await.unwrap_or(0);
    let active_user_files_size = UserFileRepository::size_active(state.db.pool()).await.unwrap_or(0);
    let deleted_user_files = UserFileRepository::count_deleted(state.db.pool()).await.unwrap_or(0);
    let deleted_user_files_size = UserFileRepository::size_deleted(state.db.pool()).await.unwrap_or(0);
    let (orphaned_physical_files, orphaned_physical_files_size) = UserFileRepository::orphaned_physical_files_global(state.db.pool()).await.unwrap_or((0, 0));
    let settings = AdminSettingRepository::get_platform_settings(state.db.pool()).await
        .unwrap_or_else(|_| crate::models::PlatformSettings {
            block_registrations: true,
            allow_user_api_keys: false,
            allow_user_password_change: false,
        });

    Ok(Json(AdminStatsDto {
        total_users,
        total_files,
        total_buckets,
        total_groups,
        block_registrations: settings.block_registrations,
        active_user_files,
        active_user_files_size,
        deleted_user_files,
        deleted_user_files_size,
        orphaned_physical_files,
        orphaned_physical_files_size,
    }))
}
