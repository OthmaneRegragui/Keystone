mod helpers;

use std::sync::Arc;

use keystone::db::repos::{BucketRepository, GroupRepository};
use keystone::models::UserRole;
use keystone::AppState;
use uuid::Uuid;

async fn build_app() -> (axum::Router, Arc<AppState>) {
    let (state, _temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::middleware::from_fn(
            keystone::api::middleware::security_headers,
        ))
        .layer(axum::middleware::from_fn(
            keystone::api::middleware::assign_request_id,
        ))
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    (app, state)
}

async fn create_bot_for_user(
    app: &axum::Router,
    state: &Arc<AppState>,
    user_id: Uuid,
) -> String {
    let (_admin_id, _u, admin_email, admin_password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let admin_token = helpers::login_user(app, &admin_email, &admin_password).await;
    let resp = helpers::json_post_auth(
        app,
        "/api/admin/bots",
        &serde_json::json!({
            "user_id": user_id.to_string(),
            "name": "test-bot",
            "can_upload": true, "can_download": true,
            "can_copy": true, "can_edit": true,
            "can_delete": true, "can_list": true,
        }),
        &admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    json["full_key"].as_str().unwrap().to_string()
}

async fn setup_bucket_access(state: &Arc<AppState>, user_id: Uuid, bucket_name: &str) {
    let bucket = match BucketRepository::create(state.db.pool(), bucket_name, "/data/test").await {
        Ok(bucket) => bucket,
        Err(keystone::error::AppError::Conflict(_)) => {
            BucketRepository::find_by_name(state.db.pool(), bucket_name)
                .await
                .unwrap()
                .unwrap()
        }
        Err(e) => panic!("failed to create bucket: {e}"),
    };
    let group = GroupRepository::create(
        state.db.pool(),
        &format!("grp_{}", &Uuid::new_v4().to_string()[..8]),
    )
    .await
    .unwrap();
    GroupRepository::add_bucket(state.db.pool(), &group.id, &bucket.id, 0)
        .await
        .unwrap();
    GroupRepository::add_member(state.db.pool(), &group.id, &user_id.to_string())
        .await
        .unwrap();
}

// ============================================================
// reject_bots middleware
// ============================================================

#[tokio::test]
async fn test_reject_bots_blocks_bot_on_user_endpoints() {
    let (app, state) = build_app().await;
    let (user_id, _u, _e, _p) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;

    let bot_key = create_bot_for_user(&app, &state, user_id).await;

    let resp = helpers::get_auth(&app, "/api/buckets", &bot_key).await;
    assert_eq!(resp.status(), 403, "bot key must be rejected on /api/buckets");

    let resp = helpers::get_auth(&app, "/api/bot/buckets", &bot_key).await;
    assert_eq!(resp.status(), 200, "bot key must work on /api/bot/buckets");
}

#[tokio::test]
async fn test_reject_bots_allows_regular_user() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::get_auth(&app, "/api/buckets", &token).await;
    assert_eq!(resp.status(), 200, "regular user must be allowed on /api/buckets");
}

// ============================================================
// security_headers middleware
// ============================================================

#[tokio::test]
async fn test_security_headers_present() {
    let (app, _state) = build_app().await;
    let resp = helpers::get_no_auth(&app, "/api/health").await;
    assert_eq!(resp.status(), 200);

    assert_eq!(
        resp.headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
        "missing or wrong x-content-type-options header"
    );
    assert_eq!(
        resp.headers()
            .get("x-frame-options")
            .and_then(|v| v.to_str().ok()),
        Some("DENY"),
        "missing or wrong x-frame-options header"
    );
    assert!(
        resp.headers().get("referrer-policy").is_some(),
        "missing referrer-policy header"
    );
}

// ============================================================
// request_id middleware
// ============================================================

#[tokio::test]
async fn test_request_id_header() {
    let (app, _state) = build_app().await;
    let resp = helpers::get_no_auth(&app, "/api/health").await;
    assert_eq!(resp.status(), 200);

    let request_id = resp
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok());
    assert!(
        request_id.is_some() && !request_id.unwrap().is_empty(),
        "missing or empty x-request-id header"
    );
}

// NOTE: Panic handler is not testable without modifying production code.
// The catch_panic middleware wraps the response in an error body when a
// handler panics, but triggering that requires inserting a deliberate
// panic in a handler, which we should not do in the production codebase
// just for test coverage.
