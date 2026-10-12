mod helpers;

use keystone::models::UserRole;

// ─── Registration Tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_register_first_user_gets_admin() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["user"]["role"], "admin");
    assert!(json["access_token"].as_str().is_some());
    assert!(json["refresh_token"].as_str().is_some());
}

#[tokio::test]
async fn test_register_second_user_gets_user_role() {
    let (app, _tmp) = helpers::build_reset_app().await;

    // First user becomes admin
    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    // Disable block_registrations so the second user can register
    let token = helpers::login_user(&app, "alice@test.com", "password123").await;
    let setting_body = serde_json::json!({
        "key": "block_registrations",
        "value": "false",
    });
    let _ = helpers::json_put_auth(&app, "/api/admin/settings", &setting_body, &token).await;

    // Second user should get "user" role
    let resp = helpers::register_user(&app, "bob", "bob@test.com", "password123").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["user"]["role"], "user");
}

#[tokio::test]
async fn test_register_duplicate_username() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    // Need to disable block_registrations so the second register proceeds to the duplicate check.
    // First user is admin, so we log in and update the setting.
    let token = helpers::login_user(&app, "alice@test.com", "password123").await;
    let setting_body = serde_json::json!({
        "key": "block_registrations",
        "value": "false",
    });
    let _ = helpers::json_put_auth(&app, "/api/admin/settings", &setting_body, &token).await;

    let resp = helpers::register_user(&app, "alice", "other@test.com", "password123").await;
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn test_register_duplicate_email() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let token = helpers::login_user(&app, "alice@test.com", "password123").await;
    let setting_body = serde_json::json!({
        "key": "block_registrations",
        "value": "false",
    });
    let _ = helpers::json_put_auth(&app, "/api/admin/settings", &setting_body, &token).await;

    let resp = helpers::register_user(&app, "bob", "alice@test.com", "password123").await;
    assert_eq!(resp.status(), 409);
}

#[tokio::test]
async fn test_register_short_username() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::register_user(&app, "ab", "ab@test.com", "password123").await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_register_short_password() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::register_user(&app, "alice", "alice@test.com", "short").await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_register_invalid_email() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::register_user(&app, "alice", "not-an-email", "password123").await;
    assert_eq!(resp.status(), 422);
}

// ─── Login Tests ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_login_success() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["access_token"].as_str().is_some());
}

#[tokio::test]
async fn test_login_wrong_password() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "wrongpassword",
        }),
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_login_nonexistent_email() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "nobody@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 401);
}

// ─── Login CAPTCHA Tests ──────────────────────────────────────────────────────
//
// The captcha is exercised against an unreachable `siteverify` endpoint (see
// helpers::build_captcha_app), so these tests stay offline: a missing token is
// rejected before any network call, and a supplied token fails closed on the
// connection error. What they prove is that enforcement happens on the server
// and *before* the credentials are ever checked — a client that simply omits
// the widget cannot log in.

#[tokio::test]
async fn test_login_rejects_missing_captcha_token() {
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 422);

    let json = helpers::response_json(resp).await;
    assert!(json["detail"]
        .as_str()
        .unwrap()
        .contains("complete the verification challenge"));
}

#[tokio::test]
async fn test_login_rejects_blank_captcha_token() {
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
            "captcha_token": "   ",
        }),
    )
    .await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_login_fails_closed_when_verifier_unreachable() {
    // Correct credentials must NOT be enough when the captcha cannot confirm
    // the challenge: failing open here would let a Cloudflare outage silently
    // turn into an unprotected login endpoint.
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
            "captcha_token": "whatever-token",
        }),
    )
    .await;
    assert_eq!(resp.status(), 422);

    let json = helpers::response_json(resp).await;
    assert!(json["detail"]
        .as_str()
        .unwrap()
        .contains("temporarily unavailable"));
}

#[tokio::test]
async fn test_login_captcha_checked_before_credentials() {
    // An unknown email must be rejected with the captcha error, not 401. If the
    // captcha ran after the credential lookup, a bot could probe which emails
    // exist (and burn Argon2) by simply omitting the token.
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "nobody@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_login_ignores_captcha_token_when_disabled() {
    // Default install: no captcha configured, so the field is simply ignored
    // and login behaves exactly as before.
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
            "captcha_token": "ignored",
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_login_captcha_disabled_when_only_site_key_set() {
    // Half-configured setups must not lock everyone out: with no secret there is
    // no way to validate a token, so the captcha stays off.
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "").await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_login_captcha_disabled_when_admin_toggle_is_off() {
    // A complete key pair with the admin toggle off must stay inactive: the
    // toggle is the operator's kill switch (e.g. during a Cloudflare outage).
    let (app, state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;
    helpers::set_captcha_enabled(&state, false).await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;
    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_captcha_settings_can_be_rotated_at_runtime() {
    // Keys are read per request, not cached at boot: an admin rotating the key
    // takes effect on the very next login, with no restart.
    let (app, state, _tmp) = helpers::build_captcha_app("old-site", "old-secret").await;
    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    helpers::set_captcha_keys(&state, "new-site", "new-secret", true).await;

    // The public settings now advertise the new site key...
    let json = helpers::response_json(helpers::get_no_auth(&app, "/api/public/settings").await).await;
    assert_eq!(json["captcha_site_key"], "new-site");

    // ...and login is enforced, still without any network call.
    let resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_public_settings_reports_captcha_state() {
    let (app, _state, _tmp) = helpers::build_captcha_app("site-key", "secret-key").await;

    let resp = helpers::get_no_auth(&app, "/api/public/settings").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["captcha_enabled"], true);
    // The site key is public by design (Cloudflare embeds it in every page).
    assert_eq!(json["captcha_site_key"], "site-key");
}

#[tokio::test]
async fn test_public_settings_reports_no_captcha_by_default() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::get_no_auth(&app, "/api/public/settings").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["captcha_enabled"], false);
    assert_eq!(json["captcha_site_key"], "");
}

// ─── Refresh Token Tests ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_refresh_token_success() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    // Login and capture full response to get the refresh_token
    let login_resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    let login_json = helpers::response_json(login_resp).await;
    let refresh_token = login_json["refresh_token"].as_str().unwrap();

    // Use refresh token to get new tokens
    let resp = helpers::json_post(
        &app,
        "/auth/refresh",
        &serde_json::json!({
            "refresh_token": refresh_token,
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["access_token"].as_str().is_some());
    assert!(json["refresh_token"].as_str().is_some());
}

#[tokio::test]
async fn test_refresh_token_invalid() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::json_post(
        &app,
        "/auth/refresh",
        &serde_json::json!({
            "refresh_token": "garbage-token-value",
        }),
    )
    .await;
    assert_eq!(resp.status(), 401);
}

// ─── Logout Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_logout_success() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let login_resp = helpers::json_post(
        &app,
        "/auth/login",
        &serde_json::json!({
            "email": "alice@test.com",
            "password": "password123",
        }),
    )
    .await;
    let login_json = helpers::response_json(login_resp).await;
    let refresh_token = login_json["refresh_token"].as_str().unwrap();

    let resp = helpers::json_post(
        &app,
        "/auth/logout",
        &serde_json::json!({
            "refresh_token": refresh_token,
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_logout_invalid_token() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::json_post(
        &app,
        "/auth/logout",
        &serde_json::json!({
            "refresh_token": "not-a-real-token",
        }),
    )
    .await;
    assert_eq!(resp.status(), 401);
}

// ─── Change Password Tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_change_password_success() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;
    let token = helpers::login_user(&app, "alice@test.com", "password123").await;

    let resp = helpers::json_post_auth(
        &app,
        "/auth/change-password",
        &serde_json::json!({
            "current_password": "password123",
            "new_password": "newsecurepass",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_change_password_wrong_current() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;
    let token = helpers::login_user(&app, "alice@test.com", "password123").await;

    let resp = helpers::json_post_auth(
        &app,
        "/auth/change-password",
        &serde_json::json!({
            "current_password": "wrongpassword",
            "new_password": "newsecurepass",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_change_password_short_new() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;
    let token = helpers::login_user(&app, "alice@test.com", "password123").await;

    let resp = helpers::json_post_auth(
        &app,
        "/auth/change-password",
        &serde_json::json!({
            "current_password": "password123",
            "new_password": "short",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
async fn test_change_password_requires_auth() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::json_post(
        &app,
        "/auth/change-password",
        &serde_json::json!({
            "current_password": "password123",
            "new_password": "newsecurepass",
        }),
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_change_password_non_admin_disabled() {
    let (state, _tmp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());

    // Create a non-admin user directly in DB (registration would create admin as first user)
    let _admin_id = helpers::create_test_user(&state.db, UserRole::Admin, "password123").await;
    let (_user_id, _username, email, _pw) =
        helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let token = helpers::login_user(&app, &email, "password123").await;

    let resp = helpers::json_post_auth(
        &app,
        "/auth/change-password",
        &serde_json::json!({
            "current_password": "password123",
            "new_password": "newsecurepass",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 403);
}

// ─── Group permission enforcement tests ────────────────────────────────────

/// Helper: create admin + non-admin users, return (app, admin_token, user_token, user_id, email).
async fn setup_admin_and_user(
) -> (axum::Router, std::sync::Arc<keystone::AppState>, String, String, String) {
    let (state, _tmp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());

    let (_aid, _an, aemail, apw) =
        helpers::create_test_user(&state.db, UserRole::Admin, "password123").await;
    let (uid, _un, uemail, _upw) =
        helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let admin_token = helpers::login_user(&app, &aemail, &apw).await;
    let user_token = helpers::login_user(&app, &uemail, &apw).await;
    (app, state, admin_token, user_token, uid.to_string())
}

/// Helper: create a group via the admin API and add `user_id` to it.
async fn create_group_and_add_member(
    app: &axum::Router,
    admin_token: &str,
    user_id: &str,
    name: &str,
) -> String {
    let resp = helpers::json_post_auth(
        app,
        "/api/admin/groups",
        &serde_json::json!({ "name": name }),
        admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let gid = json["id"].as_str().unwrap().to_string();

    let resp = helpers::json_post_auth(
        app,
        "/api/admin/groups/members",
        &serde_json::json!({ "group_id": gid, "user_id": user_id }),
        admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    gid
}

/// Helper: set a group's allow_api_keys / allow_password_change via the admin API.
async fn set_group_permissions(
    app: &axum::Router,
    admin_token: &str,
    gid: &str,
    allow_api_keys: bool,
    allow_password_change: bool,
) {
    let resp = helpers::json_put_auth(
        app,
        "/api/admin/groups/permissions",
        &serde_json::json!({
            "group_id": gid,
            "allow_api_keys": allow_api_keys,
            "allow_password_change": allow_password_change,
            "allow_bots": false,
        }),
        admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_group_password_change_permission() {
    let (app, _state, admin_token, user_token, uid) = setup_admin_and_user().await;
    let gid = create_group_and_add_member(&app, &admin_token, &uid, "pw-blocked").await;

    let body = serde_json::json!({
        "current_password": "password123",
        "new_password": "newsecurepass",
    });

    // Group allows neither by default -> blocked (403)
    let resp = helpers::json_post_auth(&app, "/auth/change-password", &body, &user_token).await;
    assert_eq!(resp.status(), 403);

    // Grant password changes for the group -> allowed
    set_group_permissions(&app, &admin_token, &gid, false, true).await;
    let resp = helpers::json_post_auth(&app, "/auth/change-password", &body, &user_token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_no_group_user_falls_back_to_global_password_change_setting() {
    let (app, _state, admin_token, user_token, _uid) = setup_admin_and_user().await;

    let body = serde_json::json!({
        "current_password": "password123",
        "new_password": "newsecurepass",
    });

    // Global setting off, no groups -> blocked
    let resp = helpers::json_post_auth(&app, "/auth/change-password", &body, &user_token).await;
    assert_eq!(resp.status(), 403);

    // Enable globally -> allowed
    let resp = helpers::json_put_auth(
        &app,
        "/api/admin/settings",
        &serde_json::json!({ "key": "allow_user_password_change", "value": "true" }),
        &admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let resp = helpers::json_post_auth(&app, "/auth/change-password", &body, &user_token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_account_permissions_endpoint() {
    let (app, _state, admin_token, user_token, uid) = setup_admin_and_user().await;
    let gid = create_group_and_add_member(&app, &admin_token, &uid, "perm-check").await;
    set_group_permissions(&app, &admin_token, &gid, true, false).await;

    let resp = helpers::get_auth(&app, "/api/me/permissions", &user_token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["allow_api_keys"], true);
    assert_eq!(json["allow_password_change"], false);
    assert_eq!(json["allow_bots"], false);
}

// ─── Forgot Password Tests ───────────────────────────────────────────────────

#[tokio::test]
async fn test_forgot_password_nonexistent() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::json_post(
        &app,
        "/auth/forgot-password",
        &serde_json::json!({
            "email": "nobody@test.com",
        }),
    )
    .await;
    // After security fix: always returns 200 to prevent user enumeration
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("reset link"));
}

#[tokio::test]
async fn test_forgot_password_success() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let _ = helpers::register_user(&app, "alice", "alice@test.com", "password123").await;

    let resp = helpers::json_post(
        &app,
        "/auth/forgot-password",
        &serde_json::json!({
            "email": "alice@test.com",
        }),
    )
    .await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().is_some());
}

// ─── Public Settings Test ────────────────────────────────────────────────────

#[tokio::test]
async fn test_public_settings() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::get_no_auth(&app, "/api/public/settings").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["block_registrations"].is_boolean());
}

// ─── Health Tests ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_check() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::get_no_auth(&app, "/api/health").await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_health_ready() {
    let (app, _tmp) = helpers::build_reset_app().await;

    let resp = helpers::get_no_auth(&app, "/api/health/ready").await;
    assert_eq!(resp.status(), 200);
}
