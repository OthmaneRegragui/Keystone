use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::{AppError, AppResult};
use crate::db::repos::AdminSettingRepository;
use tracing::{info, warn};

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::AppState;

/// Defensive bound on a stored Turnstile key. Real keys are ~35 characters.
const MAX_CAPTCHA_KEY_LEN: usize = 256;

/// The version of the currently running server binary.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The git commit the running binary was built from, if it can be determined.
///
/// Docker runs get this from `KEYSTONE_GIT_COMMIT`, which `docker-run.sh`
/// exports from the checked-out repo before `compose up`. Local builds fall
/// back to reading `.git/HEAD` from the working tree. `None` means the check
/// cannot compare commits; it then reports the remote HEAD without claiming an
/// update is available.
pub fn current_commit() -> Option<String> {
    if let Ok(raw) = std::env::var("KEYSTONE_GIT_COMMIT") {
        let sha = raw.trim();
        if !sha.is_empty() && !sha.eq_ignore_ascii_case("unknown") {
            return Some(sha.to_ascii_lowercase());
        }
    }

    // Compile-time value, for builds where the variable was present while
    // compiling but is absent from the runtime environment.
    if let Some(raw) = option_env!("KEYSTONE_GIT_COMMIT") {
        let sha = raw.trim();
        if !sha.is_empty() && !sha.eq_ignore_ascii_case("unknown") {
            return Some(sha.to_ascii_lowercase());
        }
    }

    read_git_head().map(|sha| sha.to_ascii_lowercase())
}

/// Resolve `HEAD` by walking up from the current directory until a `.git`
/// entry is found. Supports a normal `.git` directory and a worktree's `.git`
/// file (which contains `gitdir: ...`).
fn read_git_head() -> Option<String> {
    use std::path::{Path, PathBuf};

    fn read_ref(git_dir: &Path, reference: &str) -> Option<String> {
        // Worktrees keep refs in the common dir, not in the per-worktree dir.
        let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
            .ok()
            .map(|raw| {
                let path = PathBuf::from(raw.trim());
                if path.is_absolute() {
                    path
                } else {
                    git_dir.join(path)
                }
            })
            .unwrap_or_else(|| git_dir.to_path_buf());

        for base in [common_dir, git_dir.to_path_buf()] {
            if let Ok(sha) = std::fs::read_to_string(base.join(reference)) {
                let sha = sha.trim();
                if !sha.is_empty() {
                    return Some(sha.to_string());
                }
            }
        }
        None
    }

    fn resolve(git_dir: &Path) -> Option<String> {
        let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
        let head = head.trim();
        if let Some(reference) = head.strip_prefix("ref: ") {
            read_ref(git_dir, reference.trim())
        } else if !head.is_empty() {
            Some(head.to_string())
        } else {
            None
        }
    }

    let mut dir = std::env::current_dir().ok()?;
    loop {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            if let Some(sha) = resolve(&dot_git) {
                return Some(sha);
            }
        } else if dot_git.is_file() {
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            let git_dir = contents
                .trim()
                .strip_prefix("gitdir:")
                .map(str::trim)
                .map(PathBuf::from)?;
            let git_dir = if git_dir.is_absolute() {
                git_dir
            } else {
                dir.join(git_dir)
            };
            if let Some(sha) = resolve(&git_dir) {
                return Some(sha);
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// True when two commit SHAs refer to the same commit. Short and full SHAs are
/// both accepted.
fn commits_match(a: &str, b: &str) -> bool {
    let a = a.trim().to_ascii_lowercase();
    let b = b.trim().to_ascii_lowercase();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a.starts_with(&b) || b.starts_with(&a)
}

/// GitHub commit payload — only the fields we need.
#[derive(serde::Deserialize, Default)]
struct GithubCommit {
    sha: Option<String>,
    html_url: Option<String>,
    commit: Option<GithubCommitDetails>,
}

#[derive(serde::Deserialize, Default)]
struct GithubCommitDetails {
    message: Option<String>,
    author: Option<GithubCommitAuthor>,
}

#[derive(serde::Deserialize, Default)]
struct GithubCommitAuthor {
    date: Option<String>,
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

fn update_check_response(
    latest: Option<UpdateReleaseDto>,
    update_available: bool,
    can_apply: bool,
    error: Option<String>,
) -> Json<UpdateCheckDto> {
    Json(UpdateCheckDto {
        current_version: current_version().to_string(),
        current_commit: current_commit(),
        latest,
        update_available,
        can_apply,
        error,
    })
}

/// First line of a commit message, trimmed to a comfortable display length.
fn commit_summary(message: &str) -> String {
    let first = message.lines().next().unwrap_or("").trim();
    if first.chars().count() <= 120 {
        first.to_string()
    } else {
        let mut summary: String = first.chars().take(117).collect();
        summary.push_str("...");
        summary
    }
}

/// Check the remote branch's HEAD commit against the commit the running binary
/// was built from.
///
/// Updates are tracked by git commit rather than published releases: the apply
/// path is `git pull --ff-only && ./docker-run.sh`, so any new commit on the
/// branch is an available update, whether or not a release was cut.
///
/// Fails soft: network and rate-limit problems are reported as an error message
/// instead of a 500.
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
    let branch = state.config.update.branch.trim();
    let branch = if branch.is_empty() { "main" } else { branch };
    let can_apply = can_apply(&state.config.update);
    let url = format!("https://api.github.com/repos/{repo}/commits/{branch}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("keystone-update-check")
        .build()
        .map_err(|e| AppError::Internal(format!("failed to build http client: {e}")))?;

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return Ok(update_check_response(
                None,
                false,
                can_apply,
                Some(format!("update check failed: {e}")),
            ));
        }
    };

    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN {
        return Ok(update_check_response(
            None,
            false,
            can_apply,
            Some("update check rate limited by GitHub — try again later".to_string()),
        ));
    }
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(update_check_response(
            None,
            false,
            can_apply,
            Some(format!("branch '{branch}' was not found in {repo}")),
        ));
    }
    if !status.is_success() {
        return Ok(update_check_response(
            None,
            false,
            can_apply,
            Some(format!("update check failed with status {status}")),
        ));
    }

    let commit: GithubCommit = match resp.json().await {
        Ok(c) => c,
        Err(e) => {
            return Ok(update_check_response(
                None,
                false,
                can_apply,
                Some(format!("failed to parse commit info: {e}")),
            ));
        }
    };

    let remote_sha = commit.sha.unwrap_or_default();
    if remote_sha.is_empty() {
        return Ok(update_check_response(
            None,
            false,
            can_apply,
            Some("GitHub returned a commit without a SHA".to_string()),
        ));
    }

    let latest = UpdateReleaseDto {
        tag: remote_sha.chars().take(7).collect(),
        name: commit
            .commit
            .as_ref()
            .and_then(|c| c.message.as_deref())
            .map(commit_summary)
            .unwrap_or_default(),
        url: commit
            .html_url
            .unwrap_or_else(|| format!("https://github.com/{repo}/commit/{remote_sha}")),
        published_at: commit
            .commit
            .as_ref()
            .and_then(|c| c.author.as_ref())
            .and_then(|a| a.date.clone()),
        is_prerelease: false,
    };

    // Only claim an update when the running commit is known; otherwise the
    // latest commit is still shown, but the admin gets no false positive.
    let update_available = match current_commit() {
        Some(current) => !commits_match(&current, &remote_sha),
        None => false,
    };

    Ok(update_check_response(
        Some(latest),
        update_available,
        can_apply,
        None,
    ))
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

/// Store the login captcha configuration (admin-managed Turnstile keys).
///
/// Keys take effect on the next login attempt: verification reads them from the
/// database per request rather than from a cached startup snapshot, so rotating
/// a key here does not require a restart.
pub async fn update_captcha_setting(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<UpdateCaptchaSettingRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;

    let site_key = body.site_key.trim();
    if site_key.len() > MAX_CAPTCHA_KEY_LEN {
        return Err(AppError::BadRequest("site key is too long".into()));
    }
    // `None` keeps the stored secret; `Some("")` clears it. A non-empty value
    // is encrypted with the deployment's `ENCRYPTION_TOKEN` before it is
    // stored, so the Turnstile secret is not readable from the database.
    let secret_key: Option<String> = match body.secret_key.as_deref().map(str::trim) {
        None => None,
        Some("") => Some(String::new()),
        Some(secret) => {
            if secret.len() > MAX_CAPTCHA_KEY_LEN {
                return Err(AppError::BadRequest("secret key is too long".into()));
            }
            Some(state.secrets.encrypt(secret)?)
        }
    };

    AdminSettingRepository::set_captcha(state.db.pool(), body.enabled, site_key, secret_key.as_deref()).await?;

    // Enabling the captcha without a complete key pair would lock every user
    // out of login, so the server stays inactive and says so loudly rather than
    // honouring a half-configured toggle.
    let active = state.captcha_service.is_enabled(state.db.pool()).await?;
    if body.enabled && !active {
        warn!(
            requested_by = %auth.username,
            "captcha enabled without a complete key pair; the login captcha stays inactive"
        );
    }
    info!(
        admin = %auth.username,
        site_key_configured = !site_key.is_empty(),
        secret_key_updated = secret_key.is_some(),
        active,
        "admin updated the login captcha settings"
    );

    Ok(Json(MessageResponse {
        message: "setting updated".to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::{can_apply, commit_summary, commits_match, update_check_response};
    use crate::config::UpdateConfig;

    #[test]
    fn update_check_response_includes_the_running_version() {
        let response = update_check_response(None, false, false, None).0;
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
    fn commits_match_accepts_short_and_full_shas() {
        let full = "db8f15c0123456789abcdef0123456789abcdef0";
        assert!(commits_match("db8f15c", full));
        assert!(commits_match(full, "db8f15c"));
        assert!(commits_match(full, full));
        assert!(commits_match("DB8F15C", full));
    }

    #[test]
    fn commits_match_rejects_different_commits() {
        let a = "db8f15c0123456789abcdef0123456789abcdef0";
        let b = "29df5160123456789abcdef0123456789abcdef0";
        assert!(!commits_match(a, b));
        assert!(!commits_match("", a));
    }

    #[test]
    fn commit_summary_uses_the_first_line() {
        assert_eq!(commit_summary("Add mirroring\n\nLong body"), "Add mirroring");
        assert_eq!(commit_summary("  trimmed  \nbody"), "trimmed");
    }

    #[test]
    fn commit_summary_truncates_long_subjects() {
        let subject = "x".repeat(200);
        let summary = commit_summary(&subject);
        assert_eq!(summary.chars().count(), 120);
        assert!(summary.ends_with("..."));
    }
}
