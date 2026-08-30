use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::AdminSettingRepository;
use tracing::info;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

/// GitHub owner/repo the update check queries for the latest release. Override
/// with the `KEYSTONE_UPDATE_REPO` env var (format `owner/repo`).
fn update_repo() -> String {
    std::env::var("KEYSTONE_UPDATE_REPO").unwrap_or_else(|_| "OthmaneRegragui/Keystone".to_string())
}

/// The version of the currently running server binary.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Compare two dotted version strings (e.g. "0.4.0", "1.2.3"). Returns:
///   - 1 if `a` is newer than `b`
///   - -1 if `a` is older than `b`
///   - 0 if equal
fn compare_versions(a: &str, b: &str) -> i32 {
    let pa: Vec<i64> = a
        .trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    let pb: Vec<i64> = b
        .trim_start_matches('v')
        .split('.')
        .filter_map(|p| p.parse().ok())
        .collect();
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x > y {
            return 1;
        }
        if x < y {
            return -1;
        }
    }
    0
}

/// GitHub release payload — only the fields we need.
#[derive(serde::Deserialize)]
struct GithubRelease {
    tag_name: Option<String>,
    name: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
    prerelease: Option<bool>,
}

/// Check GitHub for a newer release than the running version.
///
/// Fails soft: if GitHub is unreachable, rate-limited, or the repo has no
/// release, we report an error message instead of returning a 500 — the admin
/// still sees the current version.
pub async fn check_update(
    State(_state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<UpdateCheckDto>> {
    auth.require_admin()?;

    let repo = update_repo();
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("keystone-update-check")
        .build()
        .map_err(|e| AppError::Internal(format!("failed to build http client: {e}")))?;

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return Ok(Json(UpdateCheckDto {
                current_version: current_version().to_string(),
                latest: None,
                update_available: false,
                error: Some(format!("update check failed: {e}")),
            }));
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(Json(UpdateCheckDto {
            current_version: current_version().to_string(),
            latest: None,
            update_available: false,
            error: Some(format!("no releases found for {repo}")),
        }));
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Ok(Json(UpdateCheckDto {
            current_version: current_version().to_string(),
            latest: None,
            update_available: false,
            error: Some("update check rate limited by GitHub — try again later".to_string()),
        }));
    }
    if !status.is_success() {
        return Ok(Json(UpdateCheckDto {
            current_version: current_version().to_string(),
            latest: None,
            update_available: false,
            error: Some(format!("update check failed with status {status}")),
        }));
    }

    let release: GithubRelease = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            return Ok(Json(UpdateCheckDto {
                current_version: current_version().to_string(),
                latest: None,
                update_available: false,
                error: Some(format!("failed to parse release info: {e}")),
            }));
        }
    };

    let tag = release.tag_name.unwrap_or_default();
    let latest = UpdateReleaseDto {
        tag: tag.clone(),
        name: release.name.unwrap_or_default(),
        url: release.html_url.unwrap_or_else(|| format!("https://github.com/{repo}/releases")),
        published_at: release.published_at,
        is_prerelease: release.prerelease.unwrap_or(false),
    };

    let update_available = !tag.is_empty() && compare_versions(&tag, current_version()) > 0;

    Ok(Json(UpdateCheckDto {
        current_version: current_version().to_string(),
        latest: Some(latest),
        update_available,
        error: None,
    }))
}

pub async fn get_settings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<PlatformSettingsDto>> {
    auth.require_admin()?;
    let settings = AdminSettingRepository::get_platform_settings(state.db.pool()).await?;
    Ok(Json(settings))
}

pub async fn update_setting(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateSettingRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    match body.key.as_str() {
        "block_registrations" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "block_registrations", val).await?;
            info!("admin {} set block_registrations={}", auth.username, val);
        }
        "allow_user_api_keys" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_api_keys", val).await?;
            info!("admin {} set allow_user_api_keys={}", auth.username, val);
        }
        "allow_user_password_change" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_password_change", val).await?;
            info!("admin {} set allow_user_password_change={}", auth.username, val);
        }
        "allow_user_bots" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_bots", val).await?;
            info!("admin {} set allow_user_bots={}", auth.username, val);
        }
        "allow_user_sharing" => {
            let val = body.value == "true";
            AdminSettingRepository::set_bool(state.db.pool(), "allow_user_sharing", val).await?;
            info!("admin {} set allow_user_sharing={}", auth.username, val);
        }
        _ => {
            return Err(AppError::BadRequest(format!("unknown setting key: {}", body.key)));
        }
    }
    Ok(Json(MessageResponse { message: "setting updated".to_string() }))
}
