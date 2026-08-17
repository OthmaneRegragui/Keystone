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

// ─── Admin Orphans ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_admin_list_orphaned_files_empty() {
    let (app, token, _state, _temp) = setup_admin().await;
    let resp = helpers::get_auth(&app, "/api/admin/orphaned-files", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 0);
    assert!(json["files"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_admin_list_orphaned_files_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let resp = helpers::get_auth(&app, "/api/admin/orphaned-files", &token).await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_admin_orphaned_files_requires_auth() {
    let (app, _state, _temp) = {
        let (state, temp) = helpers::build_test_state().await;
        helpers::reset_db(&state.db).await;
        let app = keystone::api_routes()
            .layer(axum::extract::Extension(state.clone()))
            .with_state(state.clone());
        (app, state, temp)
    };
    let resp = helpers::get_no_auth(&app, "/api/admin/orphaned-files").await;
    assert_eq!(resp.status(), 401);
}

// ─── Admin Bot Management ──────────────────────────────────────────────

#[tokio::test]
async fn test_admin_list_bots() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let body1 = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "bot-alpha",
        "can_upload": true,
    });
    helpers::json_post_auth(&app, "/api/admin/bots", &body1, &token).await;

    let body2 = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "bot-beta",
        "can_download": true,
    });
    helpers::json_post_auth(&app, "/api/admin/bots", &body2, &token).await;

    let resp = helpers::get_auth(&app, "/api/admin/bots", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
    let bots = json.as_array().unwrap();
    assert!(bots.len() >= 2);
    let names: Vec<&str> = bots.iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"bot-alpha"));
    assert!(names.contains(&"bot-beta"));
}

#[tokio::test]
async fn test_admin_update_bot() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let create_body = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "original-name",
        "can_upload": true,
        "can_download": false,
    });
    let create_resp = helpers::json_post_auth(&app, "/api/admin/bots", &create_body, &token).await;
    assert_eq!(create_resp.status(), 200);
    let create_json = helpers::response_json(create_resp).await;
    let bot_id = create_json["bot"]["id"].as_str().unwrap().to_string();

    let update_body = serde_json::json!({
        "name": "updated-name",
        "can_upload": false,
        "can_download": true,
    });
    let resp = helpers::json_put_auth(&app, &format!("/api/admin/bots/{}", bot_id), &update_body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "updated-name");
    assert_eq!(json["can_upload"], false);
    assert_eq!(json["can_download"], true);
}

#[tokio::test]
async fn test_admin_delete_bot() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let create_body = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "doomed-bot",
        "can_upload": true,
        "can_download": true,
        "can_list": true,
    });
    let create_resp = helpers::json_post_auth(&app, "/api/admin/bots", &create_body, &token).await;
    assert_eq!(create_resp.status(), 200);
    let create_json = helpers::response_json(create_resp).await;
    let bot_id = create_json["bot"]["id"].as_str().unwrap().to_string();
    let bot_key = create_json["full_key"].as_str().unwrap().to_string();

    // Bot key works before deletion
    let resp = helpers::get_auth(&app, "/api/bot/buckets", &bot_key).await;
    assert_eq!(resp.status(), 200);

    // Delete the bot
    use axum::body::Body;
    let request = axum::http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/admin/bots/{}", bot_id))
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
    assert_eq!(resp.status(), 200);

    // Bot key no longer works
    let resp = helpers::get_auth(&app, "/api/bot/buckets", &bot_key).await;
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_admin_create_bot_with_path_rules() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    // Create a bucket the user can access
    let bucket_name = "default";
    let _ = keystone::db::repos::BucketRepository::create(state.db.pool(), bucket_name, "/data/test").await;
    let group = keystone::db::repos::GroupRepository::create(
        state.db.pool(),
        &format!("grp_{}", &uuid::Uuid::new_v4().to_string()[..8]),
    )
    .await
    .unwrap();
    let bucket = keystone::db::repos::BucketRepository::find_by_name(state.db.pool(), bucket_name)
        .await
        .unwrap()
        .unwrap();
    keystone::db::repos::GroupRepository::add_bucket(state.db.pool(), &group.id, &bucket.id, 0)
        .await
        .unwrap();
    keystone::db::repos::GroupRepository::add_member(state.db.pool(), &group.id, &uid.to_string())
        .await
        .unwrap();

    let body = serde_json::json!({
        "user_id": uid.to_string(),
        "name": "path-bot",
        "can_upload": true,
        "can_download": true,
        "can_list": true,
        "path_rules": [
            { "bucket": "default", "path": "/work", "status": "allow" }
        ]
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/bots", &body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.get("full_key").is_some());
    let bot = &json["bot"];
    assert_eq!(bot["name"], "path-bot");
    let path_rules = bot["path_rules"].as_array().unwrap();
    assert_eq!(path_rules.len(), 1);
    assert_eq!(path_rules[0]["bucket"], "default");
    assert_eq!(path_rules[0]["path"], "/work");
    assert_eq!(path_rules[0]["status"], "allow");
}

#[tokio::test]
async fn test_admin_bot_requires_admin_or_permission() {
    let (app, token, _state, _temp) = setup_user().await;
    let body = serde_json::json!({
        "name": "nope-bot",
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/bots", &body, &token).await;
    assert_eq!(resp.status(), 403);
}

// ─── Group Bulk Members ────────────────────────────────────────────────

#[tokio::test]
async fn test_admin_bulk_add_group_members() {
    let (app, token, state, _temp) = setup_admin().await;
    let (uid1, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    let (uid2, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    let (uid3, _, _, _) = helpers::create_test_user(&state.db, UserRole::User, "password123").await;

    let group_body = serde_json::json!({"name": "bulk-group"});
    let group_resp = helpers::json_post_auth(&app, "/api/admin/groups", &group_body, &token).await;
    assert_eq!(group_resp.status(), 200);
    let group_json = helpers::response_json(group_resp).await;
    let group_id = group_json["id"].as_str().unwrap().to_string();

    let bulk_body = serde_json::json!({
        "user_ids": [uid1.to_string(), uid2.to_string(), uid3.to_string()],
        "group_ids": [group_id],
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/groups/members/bulk", &bulk_body, &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("3"));
}

#[tokio::test]
async fn test_admin_bulk_add_requires_admin() {
    let (app, token, _state, _temp) = setup_user().await;
    let body = serde_json::json!({
        "user_ids": ["00000000-0000-0000-0000-000000000001"],
        "group_ids": ["00000000-0000-0000-0000-000000000002"],
    });
    let resp = helpers::json_post_auth(&app, "/api/admin/groups/members/bulk", &body, &token).await;
    assert_eq!(resp.status(), 403);
}

// ─── Admin Settings - allow_user_bots ──────────────────────────────────

#[tokio::test]
async fn test_admin_update_allow_user_bots() {
    let (app, token, _state, _temp) = setup_admin().await;
    let body = serde_json::json!({
        "key": "allow_user_bots",
        "value": "true"
    });
    let resp = helpers::json_put_auth(&app, "/api/admin/settings", &body, &token).await;
    assert_eq!(resp.status(), 200);

    // Verify the setting was updated
    let resp = helpers::get_auth(&app, "/api/admin/settings", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["allow_user_bots"], true);
}
