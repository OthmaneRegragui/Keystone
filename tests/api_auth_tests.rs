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
        }),
        admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_api_key_creation_requires_group_permission() {
    let (app, _state, admin_token, user_token, uid) = setup_admin_and_user().await;
    let gid = create_group_and_add_member(&app, &admin_token, &uid, "api-blocked").await;

    // Group allows neither by default -> creation must be blocked (403)
    let body = serde_json::json!({
        "name": "my-key",
        "scopes": ["files:read"],
        "expires_in_days": 30,
    });
    let resp = helpers::json_post_auth(&app, "/api/api-keys", &body, &user_token).await;
    assert_eq!(resp.status(), 403);

    // Grant API keys for the group -> creation now allowed
    set_group_permissions(&app, &admin_token, &gid, true, false).await;
    let resp = helpers::json_post_auth(&app, "/api/api-keys", &body, &user_token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_api_key_regenerate_requires_group_permission() {
    let (app, _state, admin_token, user_token, uid) = setup_admin_and_user().await;
    let gid = create_group_and_add_member(&app, &admin_token, &uid, "reg-blocked").await;

    let resp = helpers::json_post_auth(&app, "/api/api-keys/regenerate", &serde_json::json!({}), &user_token).await;
    assert_eq!(resp.status(), 403);

    set_group_permissions(&app, &admin_token, &gid, true, false).await;
    let resp = helpers::json_post_auth(&app, "/api/api-keys/regenerate", &serde_json::json!({}), &user_token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_no_group_user_falls_back_to_global_api_key_setting() {
    let (app, _state, admin_token, user_token, _uid) = setup_admin_and_user().await;

    let body = serde_json::json!({
        "name": "my-key",
        "scopes": ["files:read"],
        "expires_in_days": 30,
    });

    // Global setting off, no groups -> blocked
    let resp = helpers::json_post_auth(&app, "/api/api-keys", &body, &user_token).await;
    assert_eq!(resp.status(), 403);

    // Enable globally -> allowed
    let resp = helpers::json_put_auth(
        &app,
        "/api/admin/settings",
        &serde_json::json!({ "key": "allow_user_api_keys", "value": "true" }),
        &admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let resp = helpers::json_post_auth(&app, "/api/api-keys", &body, &user_token).await;
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
async fn test_any_group_allows_api_keys() {
    let (app, _state, admin_token, user_token, uid) = setup_admin_and_user().await;
    // Two groups: one blocks, one allows -> ANY-allow semantics should permit.
    let gid_block = create_group_and_add_member(&app, &admin_token, &uid, "api-block").await;
    let gid_allow = create_group_and_add_member(&app, &admin_token, &uid, "api-allow").await;
    set_group_permissions(&app, &admin_token, &gid_block, false, false).await;
    set_group_permissions(&app, &admin_token, &gid_allow, true, false).await;

    let body = serde_json::json!({
        "name": "my-key",
        "scopes": ["files:read"],
        "expires_in_days": 30,
    });
    let resp = helpers::json_post_auth(&app, "/api/api-keys", &body, &user_token).await;
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
