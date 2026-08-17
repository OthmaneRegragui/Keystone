mod helpers;

use std::sync::Arc;

use axum::body::Body;
use bytes::Bytes;
use http_body_util::BodyExt;
use keystone::db::repos::{BucketRepository, GroupRepository, UserRepository};
use keystone::models::UserRole;
use keystone::AppState;
use uuid::Uuid;

fn build_multipart(body: &[u8], filename: &str, bucket: Option<&str>) -> (String, Body) {
    build_multipart_with_overwrite(body, filename, bucket, None)
}

/// Multipart upload with an explicit target `folder_id` field.
fn build_multipart_in_folder(body: &[u8], filename: &str, bucket: &str, folder_id: &str) -> (String, Body) {
    let boundary = "----testboundary123";
    let mut multipart_body = Vec::new();

    multipart_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    multipart_body.extend_from_slice(b"Content-Disposition: form-data; name=\"folder_id\"\r\n\r\n");
    multipart_body.extend_from_slice(folder_id.as_bytes());
    multipart_body.extend_from_slice(b"\r\n");

    multipart_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    multipart_body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n"
        )
        .as_bytes(),
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

fn build_multipart_with_overwrite(
    body: &[u8],
    filename: &str,
    bucket: Option<&str>,
    overwrite: Option<bool>,
) -> (String, Body) {
    let boundary = "----testboundary123";
    let mut multipart_body = Vec::new();

    multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    multipart_body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            filename
        )
        .as_bytes(),
    );
    multipart_body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    multipart_body.extend_from_slice(body);
    multipart_body.extend_from_slice(b"\r\n");

    if let Some(bucket_name) = bucket {
        multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        multipart_body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"bucket\"\r\n\r\n",
        );
        multipart_body.extend_from_slice(bucket_name.as_bytes());
        multipart_body.extend_from_slice(b"\r\n");
    }

    if let Some(true) = overwrite {
        multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
        multipart_body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"overwrite\"\r\n\r\n",
        );
        multipart_body.extend_from_slice(b"true");
        multipart_body.extend_from_slice(b"\r\n");
    }

    multipart_body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    let content_type = format!("multipart/form-data; boundary={}", boundary);
    (content_type, Body::from(Bytes::from(multipart_body)))
}

async fn setup_bucket_access(state: &AppState, user_id: Uuid, bucket_name: &str) {
    // Bucket names are globally unique and the test DB is shared across tests
    // that run in parallel, so tolerate an existing bucket: on a name conflict
    // (created by a previous/parallel test) reuse it. Each test still gets its
    // own group, so per-test user access is unaffected.
    let bucket = match BucketRepository::create(state.db.pool(), bucket_name, "/data/test").await
    {
        Ok(bucket) => bucket,
        Err(keystone::error::AppError::Conflict(_)) => {
            BucketRepository::find_by_name(state.db.pool(), bucket_name)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("bucket '{bucket_name}' not found after conflict"))
        }
        Err(e) => panic!("failed to create bucket '{bucket_name}': {e}"),
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

async fn build_app() -> (axum::Router, Arc<AppState>) {
    let (state, _temp) = helpers::build_test_state().await;
    helpers::reset_db(&state.db).await;
    let app = keystone::api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());
    (app, state)
}

async fn delete_auth_request(
    app: &axum::Router,
    uri: &str,
    token: &str,
) -> axum::http::Response<Body> {
    let request = axum::http::Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

async fn delete_no_auth_request(
    app: &axum::Router,
    uri: &str,
) -> axum::http::Response<Body> {
    let request = axum::http::Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

// ============================================================
// Bucket Listing
// ============================================================

#[tokio::test]
async fn test_list_user_buckets_empty() {
    let (app, state) = build_app().await;
    let (_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::get_auth(&app, "/api/buckets", &token).await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_user_buckets_with_access() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    let bucket_name = format!("bk_{}", &Uuid::new_v4().to_string()[..8]);
    setup_bucket_access(&state, user_id, &bucket_name).await;

    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::get_auth(&app, "/api/buckets", &token).await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    let buckets = json.as_array().unwrap();
    assert_eq!(buckets.len(), 1);
    assert_eq!(buckets[0]["name"], bucket_name);
    assert_eq!(buckets[0]["can_upload"], true);
    assert_eq!(buckets[0]["can_download"], true);
}

#[tokio::test]
async fn test_list_user_buckets_requires_auth() {
    let (app, _state) = build_app().await;

    let resp = helpers::get_no_auth(&app, "/api/buckets").await;
    assert_eq!(resp.status(), 401);
}

// ============================================================
// File Upload (multipart)
// ============================================================

#[tokio::test]
async fn test_upload_file_requires_auth() {
    let (app, _state) = build_app().await;

    let (content_type, body) = build_multipart(b"hello", "test.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app, request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_upload_file_no_bucket_no_access() {
    let (app, state) = build_app().await;
    let (_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (content_type, body) = build_multipart(b"hello", "test.txt", None);
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app, request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn test_upload_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (content_type, body) = build_multipart(b"hello world", "test.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app, request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["file"]["name"], "test.txt");
    assert!(json["file"]["hash"].is_string());
    assert!(!json["file"]["hash"].as_str().unwrap().is_empty());
    assert_eq!(json["duplicate"], false);
}

#[tokio::test]
async fn test_upload_duplicate_file() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (content_type, body) = build_multipart(b"same content", "file.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["duplicate"], false);

    let (content_type2, body2) = build_multipart(b"same content", "file2.txt", Some("default"));
    let request2 = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type2)
        .header("authorization", format!("Bearer {}", token))
        .body(body2)
        .unwrap();

    let resp2 = tower::ServiceExt::oneshot(app, request2)
        .await
        .expect("Failed to send request");
    assert_eq!(resp2.status(), 200);
    let json2 = helpers::response_json(resp2).await;
    assert_eq!(json2["duplicate"], true);
    assert_eq!(json2["file"]["name"], "file2.txt");
}

#[tokio::test]
async fn test_upload_same_name_same_content_returns_409() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // First upload: same content + same name -> 200
    let (content_type, body) = build_multipart(b"dup content", "dup.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);

    // Second upload: identical content + identical name -> 409, not 500
    let (content_type2, body2) = build_multipart(b"dup content", "dup.txt", Some("default"));
    let request2 = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type2)
        .header("authorization", format!("Bearer {}", token))
        .body(body2)
        .unwrap();

    let resp2 = tower::ServiceExt::oneshot(app, request2)
        .await
        .expect("Failed to send request");
    assert_eq!(resp2.status(), 409);

    let json = helpers::response_json(resp2).await;
    let problem_type = json["type"].as_str().unwrap();
    assert!(
        problem_type.contains("FILE_ALREADY_EXISTS"),
        "expected FILE_ALREADY_EXISTS in type, got: {}",
        problem_type
    );
    assert_eq!(json["title"], "File Already Exists");
    assert_eq!(json["status"], 409);
}

#[tokio::test]
async fn test_upload_overwrite_same_file() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let content = b"overwrite me";

    // First upload
    let (content_type, body) = build_multipart(content, "ow.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["duplicate"], false);
    let first_user_file_id = json["file"]["user_file_id"].as_str().unwrap().to_string();

    // Second upload with overwrite=true: same row reused, no extra row created
    let (content_type2, body2) = build_multipart_with_overwrite(
        content,
        "ow.txt",
        Some("default"),
        Some(true),
    );
    let request2 = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type2)
        .header("authorization", format!("Bearer {}", token))
        .body(body2)
        .unwrap();

    let resp2 = tower::ServiceExt::oneshot(app.clone(), request2)
        .await
        .expect("Failed to send request");
    assert_eq!(resp2.status(), 200);
    let json2 = helpers::response_json(resp2).await;
    assert_eq!(json2["duplicate"], true);
    // The SAME user_files row is reused (not a second entry)
    assert_eq!(json2["file"]["user_file_id"].as_str().unwrap(), first_user_file_id);

    // Storage usage must NOT be double-counted by the overwrite
    let user = UserRepository::find_by_id(state.db.pool(), user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.storage_used, content.len() as i64);

    // Exactly one file named "ow.txt" exists (not two)
    let resp = helpers::get_auth(&app, "/api/files", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 1);
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "ow.txt");
    // Physical file ref_count must not have been double-incremented
    assert_eq!(files[0]["ref_count"], 1);
}

// ============================================================
// File Listing
// ============================================================

#[tokio::test]
async fn test_list_files_empty() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::get_auth(&app, "/api/files", &token).await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["files"].as_array().unwrap().is_empty());
    assert_eq!(json["total"], 0);
}

#[tokio::test]
async fn test_list_files_with_upload() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (content_type, body) = build_multipart(b"my data", "upload.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);

    let resp = helpers::get_auth(&app, "/api/files", &token).await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 1);
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], "upload.txt");
}

#[tokio::test]
async fn test_list_files_requires_auth() {
    let (app, _state) = build_app().await;

    let resp = helpers::get_no_auth(&app, "/api/files").await;
    assert_eq!(resp.status(), 401);
}

// ============================================================
// File Download
// ============================================================

#[tokio::test]
async fn test_download_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let file_content = b"download me please";
    let (content_type, body) =
        build_multipart(file_content, "download.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let user_file_id = json["file"]["user_file_id"].as_str().unwrap().to_string();

    let resp = helpers::get_auth(
        &app,
        &format!("/api/files/{}/download", user_file_id),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap(),
        "attachment; filename=\"download.txt\""
    );

    let downloaded = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(downloaded.as_ref(), file_content);
}

#[tokio::test]
async fn test_raw_file_serves_inline() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let file_content = b"raw bytes please";
    let (content_type, body) =
        build_multipart(file_content, "photo.png", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let user_file_id = json["file"]["user_file_id"].as_str().unwrap().to_string();

    let resp = helpers::get_auth(
        &app,
        &format!("/api/files/{}/raw", user_file_id),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap(),
        "inline; filename=\"photo.png\""
    );
    assert_eq!(
        resp.headers().get("content-type").unwrap().to_str().unwrap(),
        "image/png"
    );

    let raw = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(raw.as_ref(), file_content);
}

#[tokio::test]
async fn test_raw_file_requires_auth() {
    let (app, _state) = build_app().await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_no_auth(&app, &format!("/api/files/{}/raw", fake_id)).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_download_file_requires_auth() {
    let (app, _state) = build_app().await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_no_auth(&app, &format!("/api/files/{}/download", fake_id)).await;
    assert_eq!(resp.status(), 401);
}

// ============================================================
// File Delete
// ============================================================

#[tokio::test]
async fn test_delete_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (content_type, body) = build_multipart(b"delete me", "delete.txt", Some("default"));
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &content_type)
        .header("authorization", format!("Bearer {}", token))
        .body(body)
        .unwrap();

    let resp = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let user_file_id = json["file"]["user_file_id"].as_str().unwrap().to_string();

    let resp =
        delete_auth_request(&app, &format!("/api/files/{}", user_file_id), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("deleted"));

    let resp = helpers::get_auth(&app, "/api/files", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 0);
}

#[tokio::test]
async fn test_delete_file_requires_auth() {
    let (app, _state) = build_app().await;
    let fake_id = Uuid::new_v4();

    let resp =
        delete_no_auth_request(&app, &format!("/api/files/{}", fake_id)).await;
    assert_eq!(resp.status(), 401);
}

// ============================================================
// Folder Endpoints
// ============================================================

#[tokio::test]
async fn test_create_folder_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/folders",
        &serde_json::json!({
            "name": "Documents",
            "bucket_name": "default",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    assert!(json["id"].is_string());
    assert_eq!(json["name"], "Documents");
    assert_eq!(json["bucket_name"], "default");
}

#[tokio::test]
async fn test_list_folder_contents() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/folders",
        &serde_json::json!({
            "name": "Projects",
            "bucket_name": "default",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let resp = helpers::get_auth(&app, "/api/folders?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);

    let json = helpers::response_json(resp).await;
    let folders = json["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0]["name"], "Projects");
}

#[tokio::test]
async fn test_rename_folder_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/folders",
        &serde_json::json!({
            "name": "Old Name",
            "bucket_name": "default",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let folder_id = json["id"].as_str().unwrap().to_string();

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/folders/{}/rename", folder_id),
        &serde_json::json!({
            "name": "New Name",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("New Name"));
}

#[tokio::test]
async fn test_delete_folder_success() {
    let (app, state) = build_app().await;
    let (user_id, _username, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;

    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/folders",
        &serde_json::json!({
            "name": "To Delete",
            "bucket_name": "default",
        }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let folder_id = json["id"].as_str().unwrap().to_string();

    let resp = delete_auth_request(
        &app,
        &format!("/api/folders/{}", folder_id),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["message"].as_str().unwrap().contains("deleted"));

    let resp = helpers::get_auth(&app, "/api/folders?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["folders"].as_array().unwrap().is_empty());
}

// ─── Bot endpoint separation ────────────────────────────────────────────
// Bots may only use the dedicated /api/bot/* namespace (buckets + file/folder
// operations). Ordinary users and JWT sessions are rejected there, and bot
// keys are rejected on the regular /api/* endpoints.

async fn create_bot_for_user(
    app: &axum::Router,
    state: &AppState,
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
            "name": "ci-bot",
            "can_upload": true,
            "can_download": true,
            "can_copy": true,
            "can_edit": true,
            "can_delete": true,
            "can_list": true,
        }),
        &admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200, "failed to create bot: {:?}", resp.status());
    let json = helpers::response_json(resp).await;
    json["full_key"].as_str().unwrap().to_string()
}

async fn create_bot_with_path_rules(
    app: &axum::Router,
    state: &AppState,
    user_id: Uuid,
    path_rules: &serde_json::Value,
) -> String {
    let (_admin_id, _u, admin_email, admin_password) =
        helpers::create_test_user(&state.db, UserRole::Admin, "pass123").await;
    let admin_token = helpers::login_user(app, &admin_email, &admin_password).await;

    let resp = helpers::json_post_auth(
        app,
        "/api/admin/bots",
        &serde_json::json!({
            "user_id": user_id.to_string(),
            "name": "ci-path-bot",
            "can_upload": true,
            "can_download": true,
            "can_copy": true,
            "can_edit": true,
            "can_delete": true,
            "can_list": true,
            "path_rules": path_rules,
        }),
        &admin_token,
    )
    .await;
    assert_eq!(resp.status(), 200, "failed to create bot: {:?}", resp.status());
    let json = helpers::response_json(resp).await;
    json["full_key"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn test_bot_can_use_bot_namespace_only() {
    let (app, state) = build_app().await;
    let (user_id, _u, _e, _p) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;

    let bot_key = create_bot_for_user(&app, &state, user_id).await;

    // Bot key works on the dedicated namespace.
    let resp = helpers::get_auth(&app, "/api/bot/buckets", &bot_key).await;
    assert_eq!(resp.status(), 200, "bot namespace should accept the bot key");

    // The same bot key is rejected on the regular user endpoint.
    let resp = helpers::get_auth(&app, "/api/buckets", &bot_key).await;
    assert_eq!(resp.status(), 403, "bot keys must be rejected on /api/buckets");

    // Bot key cannot reach admin endpoints either.
    let resp = helpers::get_auth(&app, "/api/admin/stats", &bot_key).await;
    assert_eq!(resp.status(), 403, "bot keys must be rejected on admin endpoints");
}

#[tokio::test]
async fn test_regular_user_rejected_from_bot_namespace() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;

    let user_token = helpers::login_user(&app, &email, &password).await;

    // A normal JWT session is not a bot and must be rejected on /api/bot/*.
    let resp = helpers::get_auth(&app, "/api/bot/buckets", &user_token).await;
    assert_eq!(resp.status(), 403, "regular users must be rejected on the bot namespace");

    // ...while the regular endpoint still works for them.
    let resp = helpers::get_auth(&app, "/api/buckets", &user_token).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_bot_path_rules_restrict_access() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // Create a folder "work" and upload a file into it, plus a root-level file.
    let resp = helpers::json_post_auth(
        &app,
        "/api/folders",
        &serde_json::json!({ "name": "work", "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let folder_id = helpers::response_json(resp).await["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (ct1, body1) = build_multipart(b"root content", "root.txt", Some("default"));
    let req1 = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &ct1)
        .header("authorization", format!("Bearer {token}"))
        .body(body1)
        .unwrap();
    let resp1 = tower::ServiceExt::oneshot(app.clone(), req1).await.unwrap();
    assert_eq!(resp1.status(), 200);
    let root_file_id = helpers::response_json(resp1).await["file"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (ct2, body2) = build_multipart_in_folder(b"work content", "inside.txt", "default", &folder_id);
    let req2 = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &ct2)
        .header("authorization", format!("Bearer {token}"))
        .body(body2)
        .unwrap();
    let resp2 = tower::ServiceExt::oneshot(app.clone(), req2).await.unwrap();
    assert_eq!(resp2.status(), 200);
    let inside_file_id = helpers::response_json(resp2).await["file"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A bot allowed to access only /work must not see the root file.
    let bot_key = create_bot_with_path_rules(
        &app,
        &state,
        user_id,
        &serde_json::json!([{ "bucket": "default", "path": "/work", "status": "allow" }]),
    )
    .await;

    // Listing the folder shows the allowed file.
    let resp = helpers::get_auth(
        &app,
        &format!("/api/bot/files?bucket=default&folder_id={folder_id}"),
        &bot_key,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "folder listing should show the allowed file");
    assert_eq!(files[0]["id"], inside_file_id);

    // Listing the root only shows root-level files, and none are allowed.
    let resp = helpers::get_auth(&app, "/api/bot/files?bucket=default", &bot_key).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 0);
    assert_eq!(json["total"], 0);

    // The allowed file is reachable; the root file is not.
    let resp = helpers::get_auth(&app, &format!("/api/bot/files/{inside_file_id}"), &bot_key).await;
    assert_eq!(resp.status(), 200, "allowed file should be readable");

    let resp = helpers::get_auth(&app, &format!("/api/bot/files/{root_file_id}"), &bot_key).await;
    assert_eq!(resp.status(), 403, "root file must be blocked by path rules");

    // The bucket still shows up (an allow rule makes it reachable).
    let resp = helpers::get_auth(&app, "/api/bot/buckets", &bot_key).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(
        json.as_array().unwrap().iter().any(|b| b["name"] == "default"),
        "bucket with an allow rule must be listed"
    );

    // Listing the bucket root is forbidden for a sub-path-only rule.
    let resp = helpers::get_auth(&app, "/api/bot/folders?bucket=default", &bot_key).await;
    assert_eq!(resp.status(), 403, "sub-path-only bot cannot list the bucket root");
}

// ============================================================

