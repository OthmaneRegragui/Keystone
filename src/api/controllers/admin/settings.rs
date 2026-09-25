use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::AdminSettingRepository;
use tracing::info;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

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
#[derive(serde::Deserialize, Default)]
struct GithubRelease {
    tag_name: Option<String>,
    name: Option<String>,
    html_url: Option<String>,
    published_at: Option<String>,
    prerelease: Option<bool>,
}

/// Pick the highest dotted version from a list of tag names (ignores names
/// without any numeric component, e.g. "main", "nightly").
fn newest_version_tag<'a>(tags: impl Iterator<Item = &'a str>) -> Option<String> {
    let mut newest: Option<String> = None;
    for tag in tags {
        let candidate = tag.trim_start_matches('v').to_string();
        let has_number = candidate
            .split('.')
            .filter_map(|p| p.parse::<i64>().ok())
            .next()
            .is_some();
        if !has_number {
            continue;
        }
        match &newest {
            Some(cur) => {
                if compare_versions(&candidate, cur) > 0 {
                    newest = Some(candidate);
                }
            }
            None => newest = Some(candidate),
        }
    }
    newest
}

/// Many projects only push version tags (e.g. `v0.6.0`) without creating
/// formal GitHub "releases". Fall back to the newest tag in that case so the
/// update check still works.
async fn fetch_latest_tag(
    client: &reqwest::Client,
    repo: &str,
) -> Result<Option<GithubRelease>, String> {
    #[derive(serde::Deserialize)]
    struct Tag {
        name: String,
    }

    let url = format!("https://api.github.com/repos/{repo}/tags");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("update check (tags) failed: {e}"))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let tags: Vec<Tag> = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse tags: {e}"))?;

    let newest = newest_version_tag(tags.iter().map(|t| t.name.as_str()));
    Ok(newest.map(|tag| GithubRelease {
        tag_name: Some(tag),
        html_url: Some(format!("https://github.com/{repo}/releases")),
        ..GithubRelease::default()
    }))
}

fn hook_is_usable(hook: &str) -> bool {
    std::path::Path::new(hook).is_file()
}

/// True when a restart/update can be triggered from the admin panel: either a
/// host watcher is watching a request folder, or a direct hook is configured.
fn can_apply(config: &crate::config::UpdateConfig) -> bool {
    let request_dir = config.apply_request_path.trim();
    let hook = config.apply_hook.trim();
    (!request_dir.is_empty() && std::path::Path::new(request_dir).is_dir())
        || (!hook.is_empty() && hook_is_usable(hook))
}

/// Response shown when the repository has no published releases or version
/// tags to compare against. This is a normal state for a young project, not an
/// error the admin needs to act on.
fn no_update_available_response() -> Json<UpdateCheckDto> {
    Json(UpdateCheckDto {
        current_version: current_version().to_string(),
        latest: None,
        update_available: false,
        can_apply: false,
        error: None,
    })
}

fn update_response(release: GithubRelease, repo: &str, can_apply: bool) -> Json<UpdateCheckDto> {
    let tag = release.tag_name.unwrap_or_default();
    let latest = UpdateReleaseDto {
        tag: tag.clone(),
        name: release.name.unwrap_or_default(),
        url: release.html_url.unwrap_or_else(|| format!("https://github.com/{repo}/releases")),
        published_at: release.published_at,
        is_prerelease: release.prerelease.unwrap_or(false),
    };

    let update_available = !tag.is_empty() && compare_versions(&tag, current_version()) > 0;

    Json(UpdateCheckDto {
        current_version: current_version().to_string(),
        latest: Some(latest),
        update_available,
        can_apply,
        error: None,
    })
}

/// Check GitHub for a newer release than the running version.
///
/// Fails soft: network and rate-limit problems are reported as an error message
/// instead of a 500. A repository with no releases or version tags is treated
/// as simply having no update available.
pub async fn check_update(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<UpdateCheckDto>> {
    auth.require_admin()?;

    // `update.repo` is `owner/repo`; fall back to the upstream project when an
    // operator leaves it blank.
    let repo = state.config.update.repo.trim();
    let repo = if repo.is_empty() {
        "OthmaneRegragui/Keystone"
    } else {
        repo
    };
    let can_apply = can_apply(&state.config.update);
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
                can_apply: false,
                error: Some(format!("update check failed: {e}")),
            }));
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        // No formal GitHub releases — fall back to version tags.
        return match fetch_latest_tag(&client, &repo).await {
            Ok(Some(release)) => Ok(update_response(release, &repo, can_apply)),
            Ok(None) => Ok(no_update_available_response()),
            Err(e) => Ok(Json(UpdateCheckDto {
                current_version: current_version().to_string(),
                latest: None,
                update_available: false,
                can_apply: false,
                error: Some(e),
            })),
        };
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Ok(Json(UpdateCheckDto {
            current_version: current_version().to_string(),
            latest: None,
            update_available: false,
            can_apply: false,
            error: Some("update check rate limited by GitHub — try again later".to_string()),
        }));
    }
    if !status.is_success() {
        return Ok(Json(UpdateCheckDto {
            current_version: current_version().to_string(),
            latest: None,
            update_available: false,
            can_apply: false,
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
                can_apply: false,
                error: Some(format!("failed to parse release info: {e}")),
            }));
        }
    };

    Ok(update_response(release, &repo, can_apply))
}

/// Trigger the update: either drop a request file for the host-side watcher
/// (the Docker-safe path) or run a directly-configured hook. This is the button
/// the admin panel shows when an update is available and one of these is set.
///
/// Safety: requires a logged-in admin, and the server never touches Docker
/// itself — the request file / hook is executed by the operator's host.
pub async fn apply_update(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<UpdateApplyDto>> {
    auth.require_admin()?;

    // Preferred (Docker): hand off to the host-side watcher via a marker file.
    let request_dir = state.config.update.apply_request_path.trim().to_string();
    if !request_dir.is_empty() {
        let dir_path = std::path::Path::new(&request_dir);
        if !dir_path.is_dir() {
            return Err(AppError::Internal(format!(
                "update request folder is not a directory: {request_dir}"
            )));
        }
        let request = dir_path.join(format!(
            "restart-{}.request",
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::write(&request, current_version().as_bytes())
            .map_err(|e| AppError::Internal(format!("failed to write update request: {e}")))?;
        return Ok(Json(UpdateApplyDto {
            started: true,
            message: "Update requested. Keystone will rebuild and restart in a moment.".into(),
        }));
    }

    // Fallback (server can exec the host directly, e.g. systemd/VM deploys).
    let hook = state.config.update.apply_hook.trim().to_string();
    if !hook.is_empty() {
        if !hook_is_usable(&hook) {
            return Err(AppError::Internal(format!(
                "update hook is not an executable file: {hook}"
            )));
        }
        tokio::process::Command::new(&hook)
            .spawn()
            .map_err(|e| AppError::Internal(format!("failed to start update hook: {e}")))?;
        return Ok(Json(UpdateApplyDto {
            started: true,
            message: "Update started. Keystone will restart in a moment.".into(),
        }));
    }

    Err(AppError::BadRequest(
        "no update mechanism is configured (set UPDATE_APPLY_REQUEST_PATH or UPDATE_APPLY_HOOK)"
            .into(),
    ))
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

#[cfg(test)]
mod tests {
    use super::{can_apply, compare_versions, newest_version_tag, no_update_available_response};
    use crate::config::UpdateConfig;

    #[test]
    fn no_release_or_tag_is_not_reported_as_an_error() {
        let response = no_update_available_response().0;
        assert_eq!(response.current_version, super::current_version());
        assert!(response.latest.is_none());
        assert!(!response.update_available);
        assert!(response.error.is_none());
    }

    #[test]
    fn can_apply_is_false_without_a_configured_mechanism() {
        let config = UpdateConfig::default();
        assert!(!can_apply(&config));
    }

    #[test]
    fn can_apply_is_true_when_the_request_folder_exists() {
        let config = UpdateConfig {
            apply_request_path: std::env::temp_dir().to_string_lossy().to_string(),
            ..UpdateConfig::default()
        };
        assert!(can_apply(&config));
    }

    #[test]
    fn compare_versions_accepts_v_prefix() {
        assert_eq!(compare_versions("v0.6.0", "0.5.0"), 1);
        assert_eq!(compare_versions("0.6.0", "0.6.0"), 0);
        assert_eq!(compare_versions("0.5.1", "0.6.0"), -1);
        assert_eq!(compare_versions("v0.6.10", "0.6.9"), 1);
    }

    #[test]
    fn newest_version_tag_picks_the_max() {
        let tags = ["v0.5.0", "0.6.0", "release-notes", "v0.6.1"];
        assert_eq!(newest_version_tag(tags.iter().copied()).unwrap(), "0.6.1");
    }

    #[test]
    fn newest_version_tag_skips_non_version_tags() {
        assert_eq!(newest_version_tag(["main", "nightly"].into_iter()), None);
        assert_eq!(newest_version_tag(std::iter::empty::<&str>()), None);
    }

    #[test]
    fn newest_version_tag_detects_newer_than_running() {
        // The scenario from the bug report: server is 0.7.0, only a tag exists.
        let tags = ["v0.6.0", "v0.7.0"];
        assert_eq!(newest_version_tag(tags.iter().copied()).unwrap(), "0.7.0");
        assert_eq!(compare_versions("0.7.0", super::current_version()), 0);
    }
}
