mod helpers;

use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use http_body_util::BodyExt;
use keystone::db::repos::{BucketRepository, GroupRepository};
use keystone::models::UserRole;
use keystone::AppState;
use uuid::Uuid;

fn build_multipart(body: &[u8], filename: &str, bucket: &str) -> (String, Body) {
    let boundary = "----testboundary123";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    multipart_body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n").as_bytes(),
    );
    multipart_body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    multipart_body.extend_from_slice(body);
    multipart_body.extend_from_slice(b"\r\n");
    multipart_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    multipart_body.extend_from_slice(b"Content-Disposition: form-data; name=\"bucket\"\r\n\r\n");
    multipart_body.extend_from_slice(bucket.as_bytes());
    multipart_body.extend_from_slice(b"\r\n");
    multipart_body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let content_type = format!("multipart/form-data; boundary={boundary}");
    (content_type, Body::from(Bytes::from(multipart_body)))
}

async fn delete_file(app: &axum::Router, token: &str, user_file_id: &str) {
    let request = axum::http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/files/{user_file_id}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
    assert_eq!(resp.status(), 200);
}

async fn setup_bucket_access(state: &AppState, user_id: Uuid, bucket_name: &str) {
    let bucket = match BucketRepository::create(state.db.pool(), bucket_name, "/data/test").await {
        Ok(bucket) => bucket,
        Err(keystone::error::AppError::Conflict(_)) => {
            BucketRepository::find_by_name(state.db.pool(), bucket_name).await.unwrap().unwrap()
        }
        Err(e) => panic!("failed to create bucket: {e}"),
    };
    let group = GroupRepository::create(state.db.pool(), &format!("grp_{}", &Uuid::new_v4().to_string()[..8])).await.unwrap();
    GroupRepository::add_bucket(state.db.pool(), &group.id, &bucket.id, 0).await.unwrap();
    GroupRepository::add_member(state.db.pool(), &group.id, &user_id.to_string()).await.unwrap();
}

async fn setup_app() -> (axum::Router, Arc<AppState>) {
    let (state, _temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    (app, state)
}

// ─── Dashboard Stats ───────────────────────────────────────────────────

#[tokio::test]
async fn test_dashboard_requires_auth() {
    let (app, _state) = setup_app().await;
    let resp = helpers::get_no_auth(&app, "/api/dashboard/stats").await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_dashboard_returns_empty_stats() {
    let (app, state) = setup_app().await;
    let (_uid, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::get_auth(&app, "/api/dashboard/stats", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total_files"], 0);
    assert_eq!(json["storage_used"], 0);
    assert!(json["recent_files"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_dashboard_stats_after_upload() {
    let (app, state) = setup_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // Upload a file
    let file_content = b"dashboard test content";
    let (content_type, body) = build_multipart(file_content, "dash.txt", "default");
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {token}"))
        .body(body)
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
    assert_eq!(resp.status(), 200);

    // Check dashboard stats
    let resp = helpers::get_auth(&app, "/api/dashboard/stats", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["total_files"].as_i64().unwrap() > 0);
    assert!(json["storage_used"].as_i64().unwrap() > 0);
    assert!(json["quota_bytes"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn test_dashboard_stats_after_delete() {
    let (app, state) = setup_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "password123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // Upload a file
    let file_content = b"will be deleted";
    let (content_type, body) = build_multipart(file_content, "transient.txt", "default");
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {token}"))
        .body(body)
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), request).await.unwrap();
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let user_file_id = json["file"]["user_file_id"].as_str().unwrap().to_string();

    // Verify file exists
    let resp = helpers::get_auth(&app, "/api/dashboard/stats", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total_files"], 1);
    assert!(json["storage_used"].as_i64().unwrap() > 0);

    // Delete the file
    delete_file(&app, &token, &user_file_id).await;

    // Check stats reflect deletion
    let resp = helpers::get_auth(&app, "/api/dashboard/stats", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total_files"], 0);
    assert_eq!(json["storage_used"], 0);
}
