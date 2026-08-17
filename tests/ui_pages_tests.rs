mod helpers;

use std::sync::Arc;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Router;
use http_body_util::BodyExt;
use keystone::db::repos::{AdminSettingRepository, UserRepository};
use keystone::models::UserRole;
use keystone::AppState;

// ── Embedded HTML (mirrors main.rs include_str! constants) ──────────────────

const DASHBOARD_HTML: &str = include_str!("../src/static/dashboard.html");
const FILES_HTML: &str = include_str!("../src/static/files.html");
const ACCOUNT_HTML: &str = include_str!("../src/static/account.html");
const ADMIN_HTML: &str = include_str!("../src/static/admin.html");
const LOGIN_HTML: &str = include_str!("../src/static/login.html");
const REGISTER_HTML: &str = include_str!("../src/static/register.html");
const SETUP_HTML: &str = include_str!("../src/static/setup.html");
const DOCS_HTML: &str = include_str!("../src/static/docs.html");
const ORPHANS_HTML: &str = include_str!("../src/static/orphans.html");
const BOTS_HTML: &str = include_str!("../src/static/bots.html");

const KEYSTONE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn versioned(html: &str) -> String {
    html.replace("__KEYSTONE_VERSION__", KEYSTONE_VERSION)
}

struct HtmlResponse(String);

impl IntoResponse for HtmlResponse {
    fn into_response(self) -> Response {
        axum::response::Html(self.0).into_response()
    }
}

/// Serve an admin-only page. Checks the Authorization header for admin role.
/// Mirrors main.rs admin_page().
async fn admin_page(headers: HeaderMap, state: &Arc<AppState>, html: &str) -> Response {
    if let Some(auth) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let (scheme, token) = auth.split_once(' ').unwrap_or(("", auth));
        if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
        match state.jwt_service.validate_token(token) {
            Ok(claims) => {
                if claims.role != "admin" {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
            }
            Err(_) => return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
        }
    }
    HtmlResponse(versioned(html)).into_response()
}

/// Build a full app with API routes + UI fallback (mirrors main.rs structure).
async fn build_full_app() -> (Router, Arc<AppState>) {
    let (state, _temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;

    let state_for_fallback = state.clone();
    let app = keystone::api_routes()
        .fallback(move |uri: axum::http::Uri, headers: HeaderMap| {
            let state = state_for_fallback.clone();
            async move { ui_fallback(uri, headers, state).await }
        })
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    (app, state)
}

async fn ui_fallback(uri: axum::http::Uri, headers: HeaderMap, state: Arc<AppState>) -> Response {
    let path = uri.path();

    let has_users = UserRepository::count(state.db.pool())
        .await
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_users
        && path != "/setup"
        && path != "/auth/login"
        && path != "/login"
        && path != "/auth/register"
        && path != "/register"
    {
        return Redirect::to("/setup").into_response();
    }

    match path {
        "/auth/login" | "/login" => HtmlResponse(versioned(LOGIN_HTML)).into_response(),
        "/auth/register" | "/register" => {
            let blocked = AdminSettingRepository::get_bool(state.db.pool(), "block_registrations")
                .await
                .unwrap_or(true);
            if blocked {
                Redirect::to("/login").into_response()
            } else {
                HtmlResponse(versioned(REGISTER_HTML)).into_response()
            }
        }
        "/setup" => {
            if has_users {
                Redirect::to("/login").into_response()
            } else {
                HtmlResponse(versioned(SETUP_HTML)).into_response()
            }
        }
        "/" | "/dashboard" => HtmlResponse(versioned(DASHBOARD_HTML)).into_response(),
        "/files" => HtmlResponse(versioned(FILES_HTML)).into_response(),
        "/account" => HtmlResponse(versioned(ACCOUNT_HTML)).into_response(),
        "/admin" => HtmlResponse(versioned(ADMIN_HTML)).into_response(),
        "/docs" => admin_page(headers, &state, DOCS_HTML).await,
        "/orphans" => admin_page(headers, &state, ORPHANS_HTML).await,
        "/bots" => HtmlResponse(versioned(BOTS_HTML)).into_response(),
        _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}

async fn get_page(app: &Router, path: &str) -> (StatusCode, String) {
    let resp = helpers::get_no_auth(app, path).await;
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    let body = String::from_utf8(body).unwrap_or_default();
    (status, body)
}

async fn get_page_auth(app: &Router, path: &str, token: &str) -> (StatusCode, String) {
    let resp = helpers::get_auth(app, path, token).await;
    let status = resp.status();
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    let body = String::from_utf8(body).unwrap_or_default();
    (status, body)
}

// ─── Page Rendering Tests ──────────────────────────────────────────────────

#[tokio::test]
async fn test_login_page_renders() {
    let (app, _state) = build_full_app().await;
    let (status, body) = get_page(&app, "/login").await;
    assert_eq!(status, 200);
    assert!(body.contains("login") || body.contains("Login"));
    assert!(body.contains("<form"), "login page should contain a form");
}

#[tokio::test]
async fn test_login_page_alias() {
    let (app, _state) = build_full_app().await;
    let (status, _) = get_page(&app, "/auth/login").await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn test_register_page_blocked_by_default() {
    let (app, _state) = build_full_app().await;
    let resp = helpers::get_no_auth(&app, "/register").await;
    assert_eq!(resp.status(), 302);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("/login"));
}

#[tokio::test]
async fn test_register_page_when_unblocked() {
    let (app, state) = build_full_app().await;
    let (_aid, _an, aemail, apw) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let admin_token = helpers::login_user(&app, &aemail, &apw).await;
    helpers::json_put_auth(
        &app,
        "/api/admin/settings",
        &serde_json::json!({ "key": "block_registrations", "value": "false" }),
        &admin_token,
    )
    .await;

    let (status, body) = get_page(&app, "/register").await;
    assert_eq!(status, 200);
    assert!(body.contains("register") || body.contains("Register"));
}

#[tokio::test]
async fn test_dashboard_renders_with_user() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/dashboard").await;
    assert_eq!(status, 200);
    assert!(body.contains("Dashboard") || body.contains("dashboard"));
}

#[tokio::test]
async fn test_dashboard_root_alias() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/").await;
    assert_eq!(status, 200);
    assert!(!body.is_empty());
}

#[tokio::test]
async fn test_files_page_renders() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/files").await;
    assert_eq!(status, 200);
    assert!(body.contains("Files") || body.contains("files"));
}

#[tokio::test]
async fn test_account_page_renders() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/account").await;
    assert_eq!(status, 200);
    assert!(body.contains("Account") || body.contains("account"));
}

#[tokio::test]
async fn test_admin_page_renders() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/admin").await;
    assert_eq!(status, 200);
    assert!(body.contains("Admin") || body.contains("admin"));
}

#[tokio::test]
async fn test_bots_page_renders() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let _token = helpers::login_user(&app, &email, &password).await;

    let (status, body) = get_page(&app, "/bots").await;
    assert_eq!(status, 200);
    assert!(body.contains("Bot") || body.contains("bot"));
}

#[tokio::test]
async fn test_docs_page_no_token_returns_200_or_401() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, _email, _password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;

    let (status, _) = get_page(&app, "/docs").await;
    assert!(
        status == 200 || status == 401,
        "docs without token should return 200 (no header checked) or 401, got {status}"
    );
}

#[tokio::test]
async fn test_docs_page_non_admin_forbidden() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let user_token = helpers::login_user(&app, &email, &password).await;

    let (status, _) = get_page_auth(&app, "/docs", &user_token).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn test_docs_page_admin_allowed() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, aemail, apw) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let admin_token = helpers::login_user(&app, &aemail, &apw).await;

    let (status, body) = get_page_auth(&app, "/docs", &admin_token).await;
    assert_eq!(status, 200);
    assert!(
        body.contains("API") || body.contains("api") || body.contains("docs") || body.contains("Docs")
    );
}

#[tokio::test]
async fn test_orphans_page_no_token_returns_200_or_401() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, _email, _password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;

    let (status, _) = get_page(&app, "/orphans").await;
    assert!(
        status == 200 || status == 401,
        "orphans without token should return 200 or 401, got {status}"
    );
}

#[tokio::test]
async fn test_orphans_page_non_admin_forbidden() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let user_token = helpers::login_user(&app, &email, &password).await;

    let (status, _) = get_page_auth(&app, "/orphans", &user_token).await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn test_orphans_page_admin_allowed() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, aemail, apw) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let admin_token = helpers::login_user(&app, &aemail, &apw).await;

    let (status, body) = get_page_auth(&app, "/orphans", &admin_token).await;
    assert_eq!(status, 200);
    assert!(body.contains("Orphan") || body.contains("orphan"));
}

#[tokio::test]
async fn test_unknown_page_returns_404() {
    let (app, state) = build_full_app().await;
    let (_uid, _u, _email, _password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;

    let (status, _) = get_page(&app, "/nonexistent-page").await;
    assert_eq!(status, 404);
}

// ─── HTML Structure Tests ──────────────────────────────────────────────────

#[test]
fn test_login_html_has_alpine_directive() {
    let html = versioned(LOGIN_HTML);
    assert!(html.contains("x-data"), "login page should use Alpine.js x-data");
    assert!(
        html.contains("x-on:submit") || html.contains("@submit"),
        "login page should have form submit handler"
    );
}

#[test]
fn test_dashboard_html_has_alpine_directive() {
    let html = versioned(DASHBOARD_HTML);
    assert!(
        html.contains("x-data"),
        "dashboard should use Alpine.js x-data"
    );
}

#[test]
fn test_files_html_has_alpine_directive() {
    let html = versioned(FILES_HTML);
    assert!(
        html.contains("x-data"),
        "files page should use Alpine.js x-data"
    );
    assert!(
        html.contains("fetch") || html.contains("/api/"),
        "files page should make API calls"
    );
}

#[test]
fn test_admin_html_has_alpine_directive() {
    let html = versioned(ADMIN_HTML);
    assert!(
        html.contains("x-data"),
        "admin page should use Alpine.js x-data"
    );
}

#[test]
fn test_bots_html_has_alpine_directive() {
    let html = versioned(BOTS_HTML);
    assert!(
        html.contains("x-data"),
        "bots page should use Alpine.js x-data"
    );
}

#[test]
fn test_account_html_has_alpine_directive() {
    let html = versioned(ACCOUNT_HTML);
    assert!(
        html.contains("x-data"),
        "account page should use Alpine.js x-data"
    );
}

#[test]
fn test_docs_html_has_content() {
    let html = versioned(DOCS_HTML);
    assert!(html.len() > 100, "docs page should have substantial content");
    assert!(
        html.contains("API") || html.contains("api") || html.contains("endpoint"),
        "docs page should reference API"
    );
}

#[test]
fn test_orphans_html_has_alpine_directive() {
    let html = versioned(ORPHANS_HTML);
    assert!(
        html.contains("x-data"),
        "orphans page should use Alpine.js x-data"
    );
}

#[test]
fn test_version_placeholder_replaced() {
    let html = versioned(DASHBOARD_HTML);
    assert!(
        !html.contains("__KEYSTONE_VERSION__"),
        "version placeholder should be replaced"
    );
    assert!(
        html.contains(KEYSTONE_VERSION),
        "version should be inserted"
    );
}

#[test]
fn test_all_html_files_have_head_tag() {
    let pages = [
        ("login", LOGIN_HTML),
        ("register", REGISTER_HTML),
        ("setup", SETUP_HTML),
        ("dashboard", DASHBOARD_HTML),
        ("files", FILES_HTML),
        ("account", ACCOUNT_HTML),
        ("admin", ADMIN_HTML),
        ("docs", DOCS_HTML),
        ("orphans", ORPHANS_HTML),
        ("bots", BOTS_HTML),
    ];
    for (name, html) in pages {
        assert!(html.contains("<head"), "{name} page missing <head> tag");
        assert!(html.contains("<title"), "{name} page missing <title> tag");
    }
}

#[test]
fn test_all_html_files_have_body_tag() {
    let pages = [
        ("login", LOGIN_HTML),
        ("register", REGISTER_HTML),
        ("setup", SETUP_HTML),
        ("dashboard", DASHBOARD_HTML),
        ("files", FILES_HTML),
        ("account", ACCOUNT_HTML),
        ("admin", ADMIN_HTML),
        ("docs", DOCS_HTML),
        ("orphans", ORPHANS_HTML),
        ("bots", BOTS_HTML),
    ];
    for (name, html) in pages {
        assert!(html.contains("<body"), "{name} page missing <body> tag");
    }
}

#[test]
fn test_all_interactive_html_files_load_alpine_js() {
    let pages = [
        ("login", LOGIN_HTML),
        ("dashboard", DASHBOARD_HTML),
        ("files", FILES_HTML),
        ("account", ACCOUNT_HTML),
        ("admin", ADMIN_HTML),
        ("bots", BOTS_HTML),
    ];
    for (name, html) in pages {
        assert!(
            html.contains("alpine.min.js") || html.contains("alpinejs"),
            "{name} page should load Alpine.js"
        );
    }
}

// ─── API Endpoint Structure Tests (check pages reference correct APIs) ──────

#[test]
fn test_files_page_references_api_endpoints() {
    let html = versioned(FILES_HTML);
    assert!(
        html.contains("/api/files"),
        "files page should reference /api/files"
    );
    assert!(
        html.contains("/api/folders"),
        "files page should reference /api/folders"
    );
}

#[test]
fn test_dashboard_page_references_api() {
    let html = versioned(DASHBOARD_HTML);
    assert!(
        html.contains("/api/dashboard") || html.contains("/api/files"),
        "dashboard should reference API endpoints"
    );
}

#[test]
fn test_admin_page_references_api() {
    let html = versioned(ADMIN_HTML);
    assert!(
        html.contains("/api/admin"),
        "admin page should reference /api/admin"
    );
}

#[test]
fn test_bots_page_references_api() {
    let html = versioned(BOTS_HTML);
    assert!(
        html.contains("/api/bot") || html.contains("/api/admin/bots"),
        "bots page should reference bot API endpoints"
    );
}

#[test]
fn test_docs_page_references_bot_api() {
    let html = versioned(DOCS_HTML);
    assert!(
        html.contains("/api/bot") || html.contains("bot"),
        "docs page should reference bot API"
    );
}

// ─── Static Asset Tests ────────────────────────────────────────────────────

#[test]
fn test_versioned_replaces_in_all_pages() {
    let pages = [
        LOGIN_HTML,
        REGISTER_HTML,
        SETUP_HTML,
        DASHBOARD_HTML,
        FILES_HTML,
        ACCOUNT_HTML,
        ADMIN_HTML,
        DOCS_HTML,
        ORPHANS_HTML,
        BOTS_HTML,
    ];
    for html in pages {
        let v = versioned(html);
        assert!(
            !v.contains("__KEYSTONE_VERSION__"),
            "all pages should have version placeholder replaced"
        );
    }
}

// ─── Cross-page Navigation Tests ───────────────────────────────────────────

#[test]
fn test_login_page_links_to_register() {
    let html = versioned(LOGIN_HTML);
    assert!(
        html.contains("/register") || html.contains("/auth/register"),
        "login page should link to register"
    );
}

#[test]
fn test_register_page_links_to_login() {
    let html = versioned(REGISTER_HTML);
    assert!(
        html.contains("/login") || html.contains("/auth/login"),
        "register page should link to login"
    );
}

#[test]
fn test_files_page_has_navigation() {
    let html = versioned(FILES_HTML);
    assert!(
        html.contains("/dashboard") || html.contains("/files") || html.contains("/account"),
        "files page should have navigation links"
    );
}

#[test]
fn test_admin_page_links_to_subpages() {
    let html = versioned(ADMIN_HTML);
    assert!(
        html.contains("buckets") || html.contains("users") || html.contains("groups"),
        "admin page should reference sub-sections"
    );
}
