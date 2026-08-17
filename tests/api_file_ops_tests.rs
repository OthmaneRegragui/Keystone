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

fn build_multipart_in_folder(body: &[u8], filename: &str, bucket: &str, folder_id: &str) -> (String, Body) {
    let boundary = "----testboundary123";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    multipart_body.extend_from_slice(b"Content-Disposition: form-data; name=\"folder_id\"\r\n\r\n");
    multipart_body.extend_from_slice(folder_id.as_bytes());
    multipart_body.extend_from_slice(b"\r\n");
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

async fn setup_bucket_access(state: &AppState, user_id: Uuid, bucket_name: &str) {
    let bucket = match BucketRepository::create(state.db.pool(), bucket_name, "/data/test").await {
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

/// Upload a file and return (user_file_id, file_id).
async fn upload_file(app: &axum::Router, token: &str, content: &[u8], name: &str, bucket: &str) -> (String, String) {
    let (ct, body) = build_multipart(content, name, bucket);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &ct)
        .header("authorization", format!("Bearer {token}"))
        .body(body)
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), 200, "upload failed for {name}");
    let json = helpers::response_json(resp).await;
    (
        json["file"]["user_file_id"].as_str().unwrap().to_string(),
        json["file"]["id"].as_str().unwrap().to_string(),
    )
}

async fn upload_file_in_folder(
    app: &axum::Router, token: &str, content: &[u8], name: &str, bucket: &str, folder_id: &str,
) -> (String, String) {
    let (ct, body) = build_multipart_in_folder(content, name, bucket, folder_id);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/files")
        .header("content-type", &ct)
        .header("authorization", format!("Bearer {token}"))
        .body(body)
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
    assert_eq!(resp.status(), 200, "upload in folder failed for {name}");
    let json = helpers::response_json(resp).await;
    (
        json["file"]["user_file_id"].as_str().unwrap().to_string(),
        json["file"]["id"].as_str().unwrap().to_string(),
    )
}

async fn create_folder(app: &axum::Router, token: &str, name: &str, bucket: &str) -> String {
    let resp = helpers::json_post_auth(
        app,
        "/api/folders",
        &serde_json::json!({ "name": name, "bucket_name": bucket }),
        token,
    )
    .await;
    assert_eq!(resp.status(), 200, "folder create failed for {name}");
    helpers::response_json(resp).await["id"].as_str().unwrap().to_string()
}

// ─── File Move ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_move_file_requires_auth() {
    let (app, _state) = build_app().await;
    let fake_id = Uuid::new_v4();
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{fake_id}/move"),
        &serde_json::json!({}),
        "fake-token",
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_move_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{fake_id}/move"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_move_file_to_root() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // Create folder, upload file into it, then move file to root
    let folder_id = create_folder(&app, &token, "movetest", "default").await;
    let (ufid, _fid) = upload_file_in_folder(&app, &token, b"move me", "moveme.txt", "default", &folder_id).await;

    // Verify file is in the folder
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 1);

    // Move file to root (no folder_id, same bucket)
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/move"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Folder should be empty now
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 0);

    // File should be in root listing
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json["files"].as_array().unwrap().iter().any(|f| f["name"] == "moveme.txt"));
}

#[tokio::test]
async fn test_move_file_to_folder() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, _fid) = upload_file(&app, &token, b"move to folder", "dest.txt", "default").await;
    let folder_id = create_folder(&app, &token, "dest", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/move"),
        &serde_json::json!({ "folder_id": folder_id }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Verify file is in the folder
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 1);
    assert_eq!(json["files"].as_array().unwrap()[0]["name"], "dest.txt");
}

#[tokio::test]
async fn test_move_file_nonexistent_folder() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, _fid) = upload_file(&app, &token, b"data", "f.txt", "default").await;
    let fake_folder = Uuid::new_v4();

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/move"),
        &serde_json::json!({ "folder_id": fake_folder }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

// ─── File Copy ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_copy_file_requires_auth() {
    let (app, _state) = build_app().await;
    let fake_id = Uuid::new_v4();
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{fake_id}/copy"),
        &serde_json::json!({}),
        "fake-token",
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_copy_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, fid) = upload_file(&app, &token, b"copy me", "copyme.txt", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/copy"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Should now have 2 user file entries for the same physical file
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 2);
    let files = json["files"].as_array().unwrap();
    // Both reference the same physical file id
    assert!(files.iter().all(|f| f["id"] == fid));
    // One is "copyme.txt", the other is "copyme.txt - Copy"
    let names: Vec<&str> = files.iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"copyme.txt"));
    assert!(names.contains(&"copyme.txt - Copy"));

    // ref_count should be 2
    assert_eq!(files[0]["ref_count"], 2);
}

#[tokio::test]
async fn test_copy_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{fake_id}/copy"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_copy_file_to_folder() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, _fid) = upload_file(&app, &token, b"copy to folder", "infolder.txt", "default").await;
    let folder_id = create_folder(&app, &token, "copydest", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/copy"),
        &serde_json::json!({ "folder_id": folder_id }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Verify copy is in the folder
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 1);
    assert_eq!(json["files"].as_array().unwrap()[0]["name"], "infolder.txt - Copy");
}

#[tokio::test]
async fn test_copy_file_multiple_copies_generate_unique_names() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, _fid) = upload_file(&app, &token, b"multi copy", "multi.txt", "default").await;

    // First copy
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/copy"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Second copy
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/copy"),
        &serde_json::json!({ "bucket_name": "default" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 3);
    let names: Vec<&str> = json["files"].as_array().unwrap()
        .iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"multi.txt"));
    assert!(names.contains(&"multi.txt - Copy"));
    assert!(names.contains(&"multi.txt - Copy (2)"));
}

// ─── Batch Delete ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_batch_delete_requires_auth() {
    let (app, _state) = build_app().await;
    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-delete",
        &serde_json::json!({ "file_ids": [] }),
        "fake-token",
    )
    .await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_batch_delete_empty_ids() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-delete",
        &serde_json::json!({ "file_ids": [] }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn test_batch_delete_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    // Upload 3 files
    let (ufid1, _) = upload_file(&app, &token, b"del1", "del1.txt", "default").await;
    let (ufid2, _) = upload_file(&app, &token, b"del2", "del2.txt", "default").await;
    let (ufid3, _) = upload_file(&app, &token, b"del3", "del3.txt", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-delete",
        &serde_json::json!({ "file_ids": [ufid1, ufid2, ufid3] }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["success"], 3);
    assert_eq!(json["failed"], 0);

    // All files should be gone
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 0);
}

#[tokio::test]
async fn test_batch_delete_nonexistent_ids() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let fake_id = Uuid::new_v4();
    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-delete",
        &serde_json::json!({ "file_ids": [fake_id.to_string()] }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["success"], 0);
    assert_eq!(json["failed"], 1);
}

// ─── Batch Move ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_batch_move_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid1, _) = upload_file(&app, &token, b"bm1", "batch1.txt", "default").await;
    let (ufid2, _) = upload_file(&app, &token, b"bm2", "batch2.txt", "default").await;
    let folder_id = create_folder(&app, &token, "batchdest", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-move",
        &serde_json::json!({ "file_ids": [ufid1, ufid2], "folder_id": folder_id }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["success"], 2);
    assert_eq!(json["failed"], 0);

    // Files should be in the folder
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_batch_move_empty_ids() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-move",
        &serde_json::json!({ "file_ids": [] }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 400);
}

// ─── Batch Copy ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_batch_copy_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid1, fid1) = upload_file(&app, &token, b"bc1", "batchcopy1.txt", "default").await;
    let (ufid2, fid2) = upload_file(&app, &token, b"bc2", "batchcopy2.txt", "default").await;
    let folder_id = create_folder(&app, &token, "copydest", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-copy",
        &serde_json::json!({ "file_ids": [ufid1, ufid2], "folder_id": folder_id }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["success"], 2);
    assert_eq!(json["failed"], 0);

    // Original files still in root
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 2);

    // Copies in folder
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={folder_id}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 2);

    // ref_count for both physical files should be 2
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    let json = helpers::response_json(resp).await;
    for f in json["files"].as_array().unwrap() {
        assert_eq!(f["ref_count"], 2);
    }
}

#[tokio::test]
async fn test_batch_copy_empty_ids() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let resp = helpers::json_post_auth(
        &app,
        "/api/files/batch-copy",
        &serde_json::json!({ "file_ids": [] }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 400);
}

// ─── File Rename ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_rename_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (ufid, _fid) = upload_file(&app, &token, b"rename me", "oldname.txt", "default").await;

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{ufid}/rename"),
        &serde_json::json!({ "name": "newname.txt" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Verify renamed
    let resp = helpers::get_auth(&app, "/api/files?bucket=default", &token).await;
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap()[0]["name"], "newname.txt");
}

#[tokio::test]
async fn test_rename_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/files/{fake_id}/rename"),
        &serde_json::json!({ "name": "new.txt" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

// ─── File Verify ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_verify_file_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (_ufid, fid) = upload_file(&app, &token, b"verify me", "verify.txt", "default").await;

    let resp = helpers::get_auth(&app, &format!("/api/files/{fid}/verify"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["valid"], true);
}

#[tokio::test]
async fn test_verify_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_auth(&app, &format!("/api/files/{fake_id}/verify"), &token).await;
    assert_eq!(resp.status(), 404);
}

// ─── Folder Move ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_move_folder_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let parent_id = create_folder(&app, &token, "parent", "default").await;
    let child_id = create_folder(&app, &token, "child", "default").await;

    // Move child into parent
    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/folders/{child_id}/move"),
        &serde_json::json!({ "folder_id": parent_id }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 200);

    // Child should no longer be in root
    let resp = helpers::get_auth(&app, "/api/folders?bucket=default", &token).await;
    let json = helpers::response_json(resp).await;
    let folders = json["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0]["name"], "parent");

    // Child should be inside parent
    let resp = helpers::get_auth(&app, &format!("/api/folders?bucket=default&parent_id={parent_id}"), &token).await;
    let json = helpers::response_json(resp).await;
    assert_eq!(json["folders"].as_array().unwrap().len(), 1);
    assert_eq!(json["folders"].as_array().unwrap()[0]["name"], "child");
}

// ─── Folder All / Resolve ───────────────────────────────────────────────────

#[tokio::test]
async fn test_list_all_folders() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let _parent = create_folder(&app, &token, "tree-parent", "default").await;
    let _child = create_folder(&app, &token, "tree-child", "default").await;

    let resp = helpers::get_auth(&app, "/api/folders/all?bucket=default", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    let folders = json["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 2);
}

#[tokio::test]
async fn test_resolve_folder_path() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let _f = create_folder(&app, &token, "resolve-me", "default").await;

    let resp = helpers::get_auth(&app, "/api/folders/resolve?bucket_id=default&path=/resolve-me", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert!(json.get("folder_id").is_some());
}

// ─── Download by ID ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_download_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_auth(&app, &format!("/api/files/{fake_id}/download"), &token).await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_raw_file_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_auth(&app, &format!("/api/files/{fake_id}/raw"), &token).await;
    assert_eq!(resp.status(), 404);
}

// ─── Get File Metadata ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_get_file_metadata_success() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    let (_ufid, fid) = upload_file(&app, &token, b"meta test", "meta.txt", "default").await;

    let resp = helpers::get_auth(&app, &format!("/api/files/{fid}"), &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["name"], "meta.txt");
}

#[tokio::test]
async fn test_get_file_metadata_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::get_auth(&app, &format!("/api/files/{fake_id}"), &token).await;
    assert_eq!(resp.status(), 404);
}

// ─── Delete Folder Not Found ────────────────────────────────────────────────

#[tokio::test]
async fn test_delete_folder_not_found() {
    let (app, state) = build_app().await;
    let (_uid, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    let token = helpers::login_user(&app, &email, &password).await;
    let fake_id = Uuid::new_v4();

    let resp = helpers::json_post_auth(
        &app,
        &format!("/api/folders/{fake_id}/rename"),
        &serde_json::json!({ "name": "nope" }),
        &token,
    )
    .await;
    assert_eq!(resp.status(), 404);
}

// ─── List Files with Pagination ─────────────────────────────────────────────

#[tokio::test]
async fn test_list_files_pagination() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    for i in 0..5 {
        upload_file(&app, &token, format!("content{i}").as_bytes(), &format!("page{i}.txt"), "default").await;
    }

    // Page 1
    let resp = helpers::get_auth(&app, "/api/files?bucket=default&page=1&per_page=2", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert_eq!(json["total"], 5);

    // Page 3
    let resp = helpers::get_auth(&app, "/api/files?bucket=default&page=3&per_page=2", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["files"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_list_files_search() {
    let (app, state) = build_app().await;
    let (user_id, _u, email, password) =
        helpers::create_test_user(&state.db, UserRole::User, "pass123").await;
    setup_bucket_access(&state, user_id, "default").await;
    let token = helpers::login_user(&app, &email, &password).await;

    upload_file(&app, &token, b"report data", "report.pdf", "default").await;
    upload_file(&app, &token, b"other data", "photo.jpg", "default").await;

    let resp = helpers::get_auth(&app, "/api/files?bucket=default&search=report", &token).await;
    assert_eq!(resp.status(), 200);
    let json = helpers::response_json(resp).await;
    assert_eq!(json["total"], 1);
    assert_eq!(json["files"].as_array().unwrap()[0]["name"], "report.pdf");
}
