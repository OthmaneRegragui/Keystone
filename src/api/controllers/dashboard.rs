use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::api::extractors::AuthUser;
use crate::db::repos::{ApiKeyRepository, UserFileRepository, UserRepository};
use crate::dto::*;
use crate::error::AppResult;
use crate::AppState;

/// User-facing dashboard: aggregate stats and the most recently added files.
/// UI sessions only — an API key has no dashboard to render.
pub async fn stats(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<DashboardStatsDto>> {
    auth.require_ui_session()?;

    let (total_files, storage_used, duplicates_saved) =
        UserFileRepository::summarize_by_user(state.db.pool(), auth.user_id).await?;

    let api_key_count = ApiKeyRepository::count_active_by_user(state.db.pool(), auth.user_id).await?;

    let quota_bytes = UserRepository::find_by_id(state.db.pool(), auth.user_id)
        .await?
        .map(|u| u.storage_quota)
        .unwrap_or(0);

    let recent_files = UserFileRepository::recent_by_user(state.db.pool(), auth.user_id, 8)
        .await?
        .into_iter()
        .map(|(uf, hash, size, ref_count)| FileDto {
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
        })
        .collect();

    Ok(Json(DashboardStatsDto {
        total_files,
        storage_used,
        duplicates_saved,
        api_key_count,
        quota_bytes,
        recent_files,
    }))
}
