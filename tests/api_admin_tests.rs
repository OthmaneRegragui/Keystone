mod helpers;

use keystone::models::UserRole;

async fn setup_admin() -> (axum::Router, String, std::sync::Arc<keystone::AppState>, tempfile::TempDir) {
    let (state, temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    let (_, _, email, password) = helpers::create_test_user(&state.db, UserRole::Admin, "password123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    (app, token, state, temp)
}

async fn setup_user() -> (axum::Router, String, std::sync::Arc<keystone::AppState>, tempfile::TempDir) {
    let (state, temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    let (_, _, email, password) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    (app, token, state, temp)
}

// ─── Admin Stats ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_admin_stats_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let resp = helpers::get_auth(&app, "/api/admin/stats", &token).await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_admin_stats_returns_data() {
    let (app, token, _state, _temp) = setup_admin().await;
    let resp = helpers::get_auth(&app, "/api/admin/stats", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.get("total_users").is_some());
    assert!(json.get("total_files").is_some());
    assert!(json.get("total_buckets").is_some());
    assert!(json.get("total_groups").is_some());
}

// ─── Admin Settings ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_admin_get_settings() {
    let (app, token, _state, _temp) = setup_admin().await;
    let resp = helpers::get_auth(&app, "/api/admin/settings", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.get("block_registrations").is_some());
    assert!(json.get("allow_user_api_keys").is_some());
    assert!(json.get("allow_user_password_change").is_some());
}

#[tokio::test]
async fn test_admin_update_setting() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "key": "block_registrations",
        "value": "true"
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/settings", &body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_update_unknown_setting() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "key": "nonexistent_key",
        "value": "something"
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/settings", &body, &token).await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_admin_settings_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let resp = helpers::get_auth(&app, "/api/admin/settings", &token).await;
    assert_eq!(resp.status(), 403);
}

// ─── Admin Bucket Management ─────────────────────────────────────────────

#[tokio::test]
async fn test_admin_create_bucket() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "name": "test-bucket",
        "path": "/tmp/test-bucket"
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/buckets", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "test-bucket");
}

#[tokio::test]
async fn test_admin_list_buckets() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body1 = serde_json::json!({"name": "bucket-alpha", "path": "/tmp/alpha"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &body1, &token).await;
    let body2 = serde_json::json!({"name": "bucket-beta", "path": "/tmp/beta"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &body2, &token).await;

    let resp = helpers::get_auth(&app, "/api/admin/buckets", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
    assert!(json.as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn test_admin_delete_bucket() {
    let (app, token, _state, _temp) = setup_admin().await;
    let keep_body = serde_json::json!({"name": "keep-default", "path": "/tmp/keep"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &keep_body, &token).await;

    let create_body = serde_json::json!({"name": "deleteme", "path": "/tmp/deleteme"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &create_body, &token).await;

    let delete_body = serde_json::json!({"name": "deleteme"});
    let resp = helpers::json_post_auth(&app, "/api/admin/buckets/delete", &delete_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_set_default_bucket_removed() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body1 = serde_json::json!({"name": "first-bucket", "path": "/tmp/first"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &body1, &token).await;
    let body2 = serde_json::json!({"name": "second-bucket", "path": "/tmp/second"});
    helpers::json_post_auth(&app, "/api/admin/buckets", &body2, &token).await;

    // set-default endpoint was removed
    let set_default_body = serde_json::json!({"name": "second-bucket"});
    let resp = helpers::json_post_auth(&app, "/api/admin/buckets/set-default", &set_default_body, &token).await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_admin_bucket_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let body = serde_json::json!({"name": "nope", "path": "/tmp/nope"});
    let resp = helpers::json_post_auth(&app, "/api/admin/buckets", &body, &token).await;
    assert_eq!(resp.status(), 403);
}

// ─── Admin User Management ───────────────────────────────────────────────

#[tokio::test]
async fn test_admin_list_users() {
    let (app, token, _state, _temp) = setup_admin().await;
    let resp = helpers::get_auth(&app, "/api/admin/users", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
    assert!(!json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_admin_create_user() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "username": "newuser",
        "email": "newuser@test.com",
        "password": "securepass123",
        "role": "user",
        "group_ids": []
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/users", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["username"], "newuser");
    assert_eq!(json["role"], "user");
}

#[tokio::test]
async fn test_admin_get_user() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let resp = helpers::get_auth(&app, &format!("/api/admin/users/single?id={}", uid), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["id"], uid.to_string());
}

#[tokio::test]
async fn test_admin_update_user() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let body = serde_json::json!({
        "id": uid.to_string(),
        "email": "updated@test.com"
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/users/update", &body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_update_user_quota() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let body = serde_json::json!({
        "user_id": uid.to_string(),
        "storage_quota": 2_147_483_648_i64
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/users/quota", &body, &token).await;
    assert_eq!(resp.status(), 200);
}

// ─── Admin Group Management ──────────────────────────────────────────────

#[tokio::test]
async fn test_admin_create_group() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({"name": "dev-team"});
    let resp = helpers::json_post_auth(&app, "/api/admin/groups", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "dev-team");
}

#[tokio::test]
async fn test_admin_list_groups() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body1 = serde_json::json!({"name": "group-a"});
    helpers::json_post_auth(&app, "/api/admin/groups", &body1, &token).await;
    let body2 = serde_json::json!({"name": "group-b"});
    helpers::json_post_auth(&app, "/api/admin/groups", &body2, &token).await;

    let resp = helpers::get_auth(&app, "/api/admin/groups", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
    assert!(json.as_array().unwrap().len() >= 2);
}

#[tokio::test]
async fn test_admin_delete_group() {
    let (app, token, _state, _temp) = setup_admin().await;
    let create_body = serde_json::json!({"name": "to-delete"});
    let create_resp = helpers::json_post_auth(&app, "/api/admin/groups", &create_body, &token).await;
    let json = helpers::response_json(create_resp).await;
    let group_id = json["id"].as_str().unwrap().to_string();

    let delete_body = serde_json::json!({"id": group_id});
    let resp = helpers::delete_auth(&app, "/api/admin/groups/delete", &delete_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_add_group_member() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let create_body = serde_json::json!({"name": "member-group"});
    let create_resp = helpers::json_post_auth(&app, "/api/admin/groups", &create_body, &token).await;
    let json = helpers::response_json(create_resp).await;
    let group_id = json["id"].as_str().unwrap().to_string();

    let member_body = serde_json::json!({
        "group_id": group_id,
        "user_id": uid.to_string()
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/groups/members", &member_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_remove_group_member() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let create_body = serde_json::json!({"name": "rm-member-group"});
    let create_resp = helpers::json_post_auth(&app, "/api/admin/groups", &create_body, &token).await;
    let json = helpers::response_json(create_resp).await;
    let group_id = json["id"].as_str().unwrap().to_string();

    let add_body = serde_json::json!({"group_id": group_id, "user_id": uid.to_string()});
    helpers::json_post_auth(&app, "/api/admin/groups/members", &add_body, &token).await;

    let remove_body = serde_json::json!({"group_id": group_id, "user_id": uid.to_string()});
    let resp = helpers::delete_auth(&app, "/api/admin/groups/members/remove", &remove_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_add_group_bucket() {
    let (app, token, _state, _temp) = setup_admin().await;
    let bucket_body = serde_json::json!({"name": "group-linked-bucket", "path": "/tmp/glb"});
    let bucket_resp = helpers::json_post_auth(&app, "/api/admin/buckets", &bucket_body, &token).await;
    let bucket_json = helpers::response_json(bucket_resp).await;
    let bucket_id = bucket_json["id"].as_str().unwrap().to_string();

    let group_body = serde_json::json!({"name": "bucket-group"});
    let group_resp = helpers::json_post_auth(&app, "/api/admin/groups", &group_body, &token).await;
    let group_json = helpers::response_json(group_resp).await;
    let group_id = group_json["id"].as_str().unwrap().to_string();

    let link_body = serde_json::json!({
        "group_id": group_id,
        "bucket_id": bucket_id
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/groups/buckets", &link_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_update_group_bucket_permissions() {
    let (app, token, _state, _temp) = setup_admin().await;
    let perm_bucket_body = serde_json::json!({"name": "perm-bucket", "path": "/tmp/perm"});
    let bucket_resp = helpers::json_post_auth(&app, "/api/admin/buckets", &perm_bucket_body, &token).await;
    let bucket_json = helpers::response_json(bucket_resp).await;
    let bucket_id = bucket_json["id"].as_str().unwrap().to_string();

    let group_body = serde_json::json!({"name": "perm-group"});
    let group_resp = helpers::json_post_auth(&app, "/api/admin/groups", &group_body, &token).await;
    let group_json = helpers::response_json(group_resp).await;
    let group_id = group_json["id"].as_str().unwrap().to_string();

    let link_body = serde_json::json!({"group_id": group_id, "bucket_id": bucket_id});
    helpers::json_post_auth(&app, "/api/admin/groups/buckets", &link_body, &token).await;

    let perm_body = serde_json::json!({
        "group_id": group_id,
        "bucket_id": bucket_id,
        "can_upload": true,
        "can_download": false
    });
    let resp = helpers::json_patch_auth(&app, "/api/admin/groups/buckets/permissions", &perm_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_set_group_bucket_user_limit() {
    let (app, token, _state, _temp) = setup_admin().await;
    let limit_bucket_body = serde_json::json!({"name": "limit-bucket", "path": "/tmp/limit"});
    let bucket_resp = helpers::json_post_auth(&app, "/api/admin/buckets", &limit_bucket_body, &token).await;
    let bucket_json = helpers::response_json(bucket_resp).await;
    let bucket_id = bucket_json["id"].as_str().unwrap().to_string();

    let group_body = serde_json::json!({"name": "limit-group"});
    let group_resp = helpers::json_post_auth(&app, "/api/admin/groups", &group_body, &token).await;
    let group_json = helpers::response_json(group_resp).await;
    let group_id = group_json["id"].as_str().unwrap().to_string();

    let link_body = serde_json::json!({"group_id": group_id, "bucket_id": bucket_id});
    helpers::json_post_auth(&app, "/api/admin/groups/buckets", &link_body, &token).await;

    let limit_body = serde_json::json!({
        "group_id": group_id,
        "bucket_id": bucket_id,
        "user_storage_limit": 5000000_i64
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/groups/buckets/user-limit", &limit_body, &token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_admin_group_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let body = serde_json::json!({"name": "nope"});
    let resp = helpers::json_post_auth(&app, "/api/admin/groups", &body, &token).await;
    assert_eq!(resp.status(), 403);
}

// ─── Admin API Key Management ────────────────────────────────────────────

#[tokio::test]
async fn test_admin_list_api_keys() {
    let (app, token, _state, _temp) = setup_admin().await;
    let resp = helpers::get_auth(&app, "/api/admin/api-keys", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
}

#[tokio::test]
async fn test_admin_create_api_key() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let body = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "test-key",
        "scopes": ["files:read"],
        "expires_in_days": 30
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/api-keys", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "test-key");
    assert!(json.get("full_key").is_some());
}

#[tokio::test]
async fn test_admin_create_bot_api_key() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "name": "bot-key",
        "scopes": ["files:read", "files:write"]
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/api-keys", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "bot-key");
    assert!(json.get("full_key").is_some());
}
