mod helpers;

use keystone::db::repos::api_keys::ApiKeyRepository;
use keystone::db::repos::audit::AuditLogRepository;
use keystone::db::repos::files::FileRepository;
use keystone::db::repos::folders::FolderRepository;
use keystone::db::repos::storage::StorageObjectRepository;
use keystone::db::repos::user_files::UserFileRepository;
use keystone::db::repos::users::UserRepository;
use keystone::db::rows::api_key_row::CreateApiKeyData;
use keystone::db::rows::file_row::FileRecord;
use keystone::db::rows::folder_row::FolderRecord;
use keystone::db::rows::storage_object_row::CreateStorageObjectData;
use keystone::db::rows::user_file_row::UserFileRecord;
use keystone::db::rows::user_row::CreateUserData;
use keystone::db::rows::CreateAuditLogData;
use keystone::error::AppError;
use keystone::models::UserRole;
use sqlx::PgPool;
use uuid::Uuid;

async fn create_test_user(pool: &PgPool) -> Uuid {
    let user = UserRepository::create(
        pool,
        CreateUserData {
            username: format!("user_{}", &Uuid::new_v4().to_string()[..8]),
            email: format!("u_{}@test.com", &Uuid::new_v4().to_string()[..8]),
            password_hash: "hash".to_string(),
            role: UserRole::User,
            storage_quota: 1_073_741_824,
        },
    )
    .await
    .unwrap();
    user.id
}

async fn create_test_file(pool: &PgPool, hash: &str) -> Uuid {
    let file = FileRepository::create(
        pool,
        FileRecord::new(
            hash.to_string(),
            "test.txt".to_string(),
            Some("text/plain".to_string()),
            1024,
        ),
    )
    .await
    .unwrap();
    file.id
}

// ==================== FolderRepository Tests ====================

#[tokio::test]
async fn test_folder_create() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let record = FolderRecord::new(user_id, "my-bucket".into(), "Documents".into(), None);
    let folder = FolderRepository::create(pool, record).await.unwrap();

    assert!(!folder.id.is_nil());
    assert_eq!(folder.user_id, user_id);
    assert_eq!(folder.bucket_name, "my-bucket");
    assert_eq!(folder.name, "Documents");
    assert!(folder.parent_id.is_none());
}

#[tokio::test]
async fn test_folder_create_nested() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Work".into(), None),
    )
    .await
    .unwrap();

    let child = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Projects".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    assert_eq!(child.parent_id, Some(parent.id));
    assert_eq!(child.name, "Projects");
}

#[tokio::test]
async fn test_folder_create_duplicate() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Parent".into(), None),
    )
    .await
    .unwrap();

    FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "A".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    let result = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "A".into(), Some(parent.id)),
    )
    .await;

    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_folder_find_by_id() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let created = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Docs".into(), None),
    )
    .await
    .unwrap();

    let found = FolderRepository::find_by_id(pool, created.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "Docs");

    let not_found = FolderRepository::find_by_id(pool, Uuid::new_v4()).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_folder_find_by_user_and_id() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user1 = create_test_user(pool).await;
    let user2 = create_test_user(pool).await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user1, "bucket".into(), "Shared".into(), None),
    )
    .await
    .unwrap();

    let found = FolderRepository::find_by_user_and_id(pool, user1, folder.id)
        .await
        .unwrap();
    assert!(found.is_some());

    let not_found = FolderRepository::find_by_user_and_id(pool, user2, folder.id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_folder_list_children_root() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    for name in ["Alpha", "Gamma", "Beta"] {
        FolderRepository::create(
            pool,
            FolderRecord::new(user_id, "bucket".into(), name.into(), None),
        )
        .await
        .unwrap();
    }

    let children = FolderRepository::list_children(pool, user_id, "bucket", None)
        .await
        .unwrap();
    assert_eq!(children.len(), 3);
    assert_eq!(children[0].name, "Alpha");
    assert_eq!(children[1].name, "Beta");
    assert_eq!(children[2].name, "Gamma");
}

#[tokio::test]
async fn test_folder_list_children_nested() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Parent".into(), None),
    )
    .await
    .unwrap();

    FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Child1".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Child2".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    let children = FolderRepository::list_children(pool, user_id, "bucket", Some(parent.id))
        .await
        .unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].name, "Child1");
    assert_eq!(children[1].name, "Child2");
}

#[tokio::test]
async fn test_folder_list_children_empty_bucket() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let children = FolderRepository::list_children(pool, user_id, "nonexistent", None)
        .await
        .unwrap();
    assert!(children.is_empty());
}

#[tokio::test]
async fn test_folder_update_name() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Old".into(), None),
    )
    .await
    .unwrap();

    let updated = FolderRepository::update_name(pool, folder.id, "New").await.unwrap();
    assert!(updated);

    let found = FolderRepository::find_by_id(pool, folder.id).await.unwrap().unwrap();
    assert_eq!(found.name, "New");
}

#[tokio::test]
async fn test_folder_update_name_conflict() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Parent".into(), None),
    )
    .await
    .unwrap();

    FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "A".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    let folder_b = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "B".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    let result = FolderRepository::update_name(pool, folder_b.id, "A").await;
    assert!(matches!(result, Err(AppError::Conflict(_))));
}

#[tokio::test]
async fn test_folder_delete_root() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "ToDelete".into(), None),
    )
    .await
    .unwrap();

    let deleted = FolderRepository::delete(pool, folder.id).await.unwrap();
    assert!(deleted);

    let found = FolderRepository::find_by_id(pool, folder.id).await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_folder_delete_recursive() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Parent".into(), None),
    )
    .await
    .unwrap();

    let child = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Child".into(), Some(parent.id)),
    )
    .await
    .unwrap();

    FolderRepository::delete(pool, parent.id).await.unwrap();

    // Parent and all descendants should be deleted
    let parent_after = FolderRepository::find_by_id(pool, parent.id).await.unwrap();
    assert!(parent_after.is_none());

    let child_after = FolderRepository::find_by_id(pool, child.id).await.unwrap();
    assert!(child_after.is_none());
}

#[tokio::test]
async fn test_folder_count_files() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash1").await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Docs".into(), None),
    )
    .await
    .unwrap();

    let count = FolderRepository::count_files(pool, folder.id).await.unwrap();
    assert_eq!(count, 0);

    let mut uf_record = UserFileRecord::new(user_id, file_id, "a.txt".into(), None, Some("bucket".into()));
    uf_record.folder_id = Some(folder.id);
    UserFileRepository::create(pool, uf_record).await.unwrap();

    let count = FolderRepository::count_files(pool, folder.id).await.unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_folder_count_subfolders() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let parent = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Parent".into(), None),
    )
    .await
    .unwrap();

    for name in ["Sub1", "Sub2"] {
        FolderRepository::create(
            pool,
            FolderRecord::new(user_id, "bucket".into(), name.into(), Some(parent.id)),
        )
        .await
        .unwrap();
    }

    let count = FolderRepository::count_subfolders(pool, parent.id).await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_folder_get_path() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let root = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "root".into(), None),
    )
    .await
    .unwrap();

    let sub = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "sub".into(), Some(root.id)),
    )
    .await
    .unwrap();

    let subsub = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "subsub".into(), Some(sub.id)),
    )
    .await
    .unwrap();

    let path = FolderRepository::get_path(pool, subsub.id).await.unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(path[0], (root.id, "root".to_string()));
    assert_eq!(path[1], (sub.id, "sub".to_string()));
    assert_eq!(path[2], (subsub.id, "subsub".to_string()));
}

// ==================== UserFileRepository Tests ====================

#[tokio::test]
async fn test_userfile_create() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "abc123").await;

    let record = UserFileRecord::new(
        user_id,
        file_id,
        "photo.jpg".into(),
        Some("image/jpeg".into()),
        Some("my-bucket".into()),
    );
    let uf = UserFileRepository::create(pool, record).await.unwrap();

    assert!(!uf.id.is_nil());
    assert_eq!(uf.user_id, user_id);
    assert_eq!(uf.file_id, file_id);
    assert_eq!(uf.original_name, "photo.jpg");
    assert_eq!(uf.mime_type.as_deref(), Some("image/jpeg"));
    assert_eq!(uf.bucket_name.as_deref(), Some("my-bucket"));
    assert!(uf.folder_id.is_none());
}

#[tokio::test]
async fn test_userfile_find_by_id() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    let created = UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file_id, "doc.pdf".into(), None, None),
    )
    .await
    .unwrap();

    let found = UserFileRepository::find_by_id(pool, created.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().original_name, "doc.pdf");

    let not_found = UserFileRepository::find_by_id(pool, Uuid::new_v4()).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_userfile_find_by_user_and_file() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user1 = create_test_user(pool).await;
    let user2 = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    UserFileRepository::create(
        pool,
        UserFileRecord::new(user1, file_id, "file.txt".into(), None, None),
    )
    .await
    .unwrap();

    let found = UserFileRepository::find_by_user_and_file(pool, user1, file_id)
        .await
        .unwrap();
    assert!(found.is_some());

    let not_found = UserFileRepository::find_by_user_and_file(pool, user2, file_id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_userfile_find_by_user_and_id() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    let created = UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file_id, "f.txt".into(), None, None),
    )
    .await
    .unwrap();

    let found = UserFileRepository::find_by_user_and_id(pool, user_id, created.id)
        .await
        .unwrap();
    assert!(found.is_some());

    let wrong_user = create_test_user(pool).await;
    let not_found = UserFileRepository::find_by_user_and_id(pool, wrong_user, created.id)
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_userfile_list_by_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    for i in 0..5 {
        let file_id = create_test_file(pool, &format!("hash_{i}")).await;
        UserFileRepository::create(
            pool,
            UserFileRecord::new(user_id, file_id, format!("file_{i}.txt"), None, None),
        )
        .await
        .unwrap();
    }

    let page1 = UserFileRepository::list_by_user(pool, user_id, 0, 3, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(page1.len(), 3);

    let page2 = UserFileRepository::list_by_user(pool, user_id, 3, 3, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);

    let all = UserFileRepository::list_by_user(pool, user_id, 0, 100, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 5);

    for (uf, hash, size, ref_count) in &all {
        assert_eq!(uf.user_id, user_id);
        assert!(!hash.is_empty());
        assert_eq!(*size, 1024);
        assert_eq!(*ref_count, 1);
    }
}

#[tokio::test]
async fn test_userfile_list_by_user_search() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let file1 = create_test_file(pool, "hash1").await;
    let file2 = create_test_file(pool, "hash2").await;

    UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file1, "report.pdf".into(), None, None),
    )
    .await
    .unwrap();
    UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file2, "photo.jpg".into(), None, None),
    )
    .await
    .unwrap();

    let results = UserFileRepository::list_by_user(pool, user_id, 0, 100, Some("report"), None, None, None)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.original_name, "report.pdf");

    let all = UserFileRepository::list_by_user(pool, user_id, 0, 100, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_userfile_list_by_user_bucket_filter() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let file1 = create_test_file(pool, "hash1").await;
    let file2 = create_test_file(pool, "hash2").await;

    UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file1, "a.txt".into(), None, Some("bucket-a".into())),
    )
    .await
    .unwrap();
    UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file2, "b.txt".into(), None, Some("bucket-b".into())),
    )
    .await
    .unwrap();

    let results =
        UserFileRepository::list_by_user(pool, user_id, 0, 100, None, Some("bucket-a"), None, None)
            .await
            .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.original_name, "a.txt");
}

#[tokio::test]
async fn test_userfile_list_by_user_folder_filter() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Docs".into(), None),
    )
    .await
    .unwrap();

    let file1 = create_test_file(pool, "hash1").await;
    let file2 = create_test_file(pool, "hash2").await;

    let mut uf1 = UserFileRecord::new(user_id, file1, "in_folder.txt".into(), None, Some("bucket".into()));
    uf1.folder_id = Some(folder.id);
    UserFileRepository::create(pool, uf1).await.unwrap();

    let uf2 = UserFileRecord::new(user_id, file2, "root_file.txt".into(), None, Some("bucket".into()));
    UserFileRepository::create(pool, uf2).await.unwrap();

    let results =
        UserFileRepository::list_by_user(pool, user_id, 0, 100, None, None, Some(folder.id), None)
            .await
            .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.original_name, "in_folder.txt");
}

#[tokio::test]
async fn test_userfile_count_by_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let count = UserFileRepository::count_by_user(pool, user_id, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(count, 0);

    for i in 0..3 {
        let fid = create_test_file(pool, &format!("h{i}")).await;
        UserFileRepository::create(
            pool,
            UserFileRecord::new(user_id, fid, format!("f{i}.txt"), None, None),
        )
        .await
        .unwrap();
    }

    let count = UserFileRepository::count_by_user(pool, user_id, None, None, None, None)
        .await
        .unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn test_userfile_delete() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    let uf = UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file_id, "f.txt".into(), None, None),
    )
    .await
    .unwrap();

    let deleted = UserFileRepository::delete(pool, uf.id).await.unwrap();
    assert!(deleted);

    let deleted_again = UserFileRepository::delete(pool, uf.id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_userfile_update_name() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    let uf = UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file_id, "old.txt".into(), None, None),
    )
    .await
    .unwrap();

    let updated = UserFileRepository::update_name(pool, uf.id, "new.txt")
        .await
        .unwrap();
    assert!(updated);

    let found = UserFileRepository::find_by_id(pool, uf.id).await.unwrap().unwrap();
    assert_eq!(found.original_name, "new.txt");

    let not_updated = UserFileRepository::update_name(pool, Uuid::new_v4(), "x.txt")
        .await
        .unwrap();
    assert!(!not_updated);
}

#[tokio::test]
async fn test_userfile_update_folder() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;
    let file_id = create_test_file(pool, "hash").await;

    let folder = FolderRepository::create(
        pool,
        FolderRecord::new(user_id, "bucket".into(), "Docs".into(), None),
    )
    .await
    .unwrap();

    let uf = UserFileRepository::create(
        pool,
        UserFileRecord::new(user_id, file_id, "f.txt".into(), None, Some("bucket".into())),
    )
    .await
    .unwrap();
    assert!(uf.folder_id.is_none());

    let moved = UserFileRepository::update_folder(pool, uf.id, Some(folder.id))
        .await
        .unwrap();
    assert!(moved);

    let found = UserFileRepository::find_by_id(pool, uf.id).await.unwrap().unwrap();
    assert_eq!(found.folder_id, Some(folder.id));

    let moved_back = UserFileRepository::update_folder(pool, uf.id, None)
        .await
        .unwrap();
    assert!(moved_back);

    let found = UserFileRepository::find_by_id(pool, uf.id).await.unwrap().unwrap();
    assert!(found.folder_id.is_none());
}

// ==================== ApiKeyRepository Tests ====================

#[tokio::test]
async fn test_apikey_create() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let key = ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "my-key".into(),
            key_prefix: "ks_abc".into(),
            key_hash: "hash_value".into(),
            scopes: vec!["files:read".into()],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    assert!(!key.id.is_nil());
    assert_eq!(key.user_id, Some(user_id));
    assert_eq!(key.name, "my-key");
    assert_eq!(key.key_prefix, "ks_abc");
    assert_eq!(key.key_hash, "hash_value");
    assert_eq!(key.scopes, vec!["files:read".to_string()]);
    assert!(key.is_active);
    assert!(key.last_used_at.is_none());
}

#[tokio::test]
async fn test_apikey_find_by_id() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let created = ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "key1".into(),
            key_prefix: "ks_1".into(),
            key_hash: "hash1".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let found = ApiKeyRepository::find_by_id(pool, created.id).await.unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "key1");

    let not_found = ApiKeyRepository::find_by_id(pool, Uuid::new_v4()).await.unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_apikey_find_by_key_hash() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "key1".into(),
            key_prefix: "ks_1".into(),
            key_hash: "unique_hash_123".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let found = ApiKeyRepository::find_by_key_hash(pool, "unique_hash_123")
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().key_hash, "unique_hash_123");

    let not_found = ApiKeyRepository::find_by_key_hash(pool, "nonexistent")
        .await
        .unwrap();
    assert!(not_found.is_none());
}

#[tokio::test]
async fn test_apikey_list_by_user() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    for i in 0..3 {
        ApiKeyRepository::create(
            pool,
            CreateApiKeyData {
                user_id: Some(user_id),
                name: format!("key_{i}"),
                key_prefix: format!("ks_{i}"),
                key_hash: format!("hash_{i}"),
                scopes: vec![],
                expires_at: None,
            },
        )
        .await
        .unwrap();
    }

    let keys = ApiKeyRepository::list_by_user(pool, user_id).await.unwrap();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn test_apikey_list_bot_keys() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "user-key".into(),
            key_prefix: "ks_u".into(),
            key_hash: "hash_u".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: None,
            name: "bot-key".into(),
            key_prefix: "ks_b".into(),
            key_hash: "hash_b".into(),
            scopes: vec!["bot:access".into()],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let bot_keys = ApiKeyRepository::list_bot_keys(pool).await.unwrap();
    assert_eq!(bot_keys.len(), 1);
    assert_eq!(bot_keys[0].name, "bot-key");
    assert!(bot_keys[0].user_id.is_none());
}

#[tokio::test]
async fn test_apikey_update_last_used() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let key = ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "key".into(),
            key_prefix: "ks_1".into(),
            key_hash: "hash_update_last_used".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();
    assert!(key.last_used_at.is_none());

    ApiKeyRepository::update_last_used(pool, key.id).await.unwrap();

    let updated = ApiKeyRepository::find_by_id(pool, key.id).await.unwrap().unwrap();
    assert!(updated.last_used_at.is_some());

    let result = ApiKeyRepository::update_last_used(pool, Uuid::new_v4()).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_apikey_delete() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let key = ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "key".into(),
            key_prefix: "ks_1".into(),
            key_hash: "hash_delete".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let deleted = ApiKeyRepository::delete(pool, key.id).await.unwrap();
    assert!(deleted);

    let deleted_again = ApiKeyRepository::delete(pool, key.id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_apikey_deactivate() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let key = ApiKeyRepository::create(
        pool,
        CreateApiKeyData {
            user_id: Some(user_id),
            name: "key".into(),
            key_prefix: "ks_1".into(),
            key_hash: "hash_deactivate".into(),
            scopes: vec![],
            expires_at: None,
        },
    )
    .await
    .unwrap();
    assert!(key.is_active);

    ApiKeyRepository::deactivate(pool, key.id).await.unwrap();

    let deactivated = ApiKeyRepository::find_by_id(pool, key.id).await.unwrap().unwrap();
    assert!(!deactivated.is_active);

    let result = ApiKeyRepository::deactivate(pool, Uuid::new_v4()).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

// ==================== StorageObjectRepository Tests ====================

#[tokio::test]
async fn test_storage_object_create() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let file_id = create_test_file(pool, "abc123").await;

    let obj = StorageObjectRepository::create(
        pool,
        CreateStorageObjectData {
            file_id,
            backend: "local".into(),
            storage_path: "ab/cd/ef123.jpg".into(),
        },
    )
    .await
    .unwrap();

    assert!(!obj.id.is_nil());
    assert_eq!(obj.file_id, file_id);
    assert_eq!(obj.backend, "local");
    assert_eq!(obj.storage_path, "ab/cd/ef123.jpg");

    let found = StorageObjectRepository::find_by_file_id(pool, file_id).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, obj.id);
}

#[tokio::test]
async fn test_storage_object_find_orphaned() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let file_id = create_test_file(pool, "abc123").await;

    let orphaned = StorageObjectRepository::find_orphaned(pool).await.unwrap();
    assert!(orphaned.is_empty());

    StorageObjectRepository::create(
        pool,
        CreateStorageObjectData {
            file_id,
            backend: "local".into(),
            storage_path: "path/to/file.bin".into(),
        },
    )
    .await
    .unwrap();

    let orphaned = StorageObjectRepository::find_orphaned(pool).await.unwrap();
    assert!(orphaned.is_empty());
}

// ==================== AuditLogRepository Tests ====================

#[tokio::test]
async fn test_audit_log_create() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();
    let user_id = create_test_user(pool).await;

    let log = AuditLogRepository::create(
        pool,
        CreateAuditLogData {
            user_id,
            action: "upload".into(),
            resource: "file".into(),
            resource_id: Some("file-uuid-123".into()),
            details: Some("uploaded 1 file".into()),
            ip_address: Some("127.0.0.1".into()),
        },
    )
    .await
    .unwrap();

    assert!(!log.id.is_nil());
    assert_eq!(log.user_id, user_id);
    assert_eq!(log.action, "upload");
    assert_eq!(log.resource, "file");
    assert_eq!(log.resource_id.as_deref(), Some("file-uuid-123"));
    assert_eq!(log.details.as_deref(), Some("uploaded 1 file"));
    assert_eq!(log.ip_address.as_deref(), Some("127.0.0.1"));
}

#[tokio::test]
async fn test_audit_log_list() {
    let db = helpers::setup_reset_db().await;
    let pool = db.pool();
    let user1 = create_test_user(pool).await;
    let user2 = create_test_user(pool).await;

    AuditLogRepository::create(
        pool,
        CreateAuditLogData {
            user_id: user1,
            action: "upload".into(),
            resource: "file".into(),
            resource_id: None,
            details: None,
            ip_address: None,
        },
    )
    .await
    .unwrap();

    AuditLogRepository::create(
        pool,
        CreateAuditLogData {
            user_id: user1,
            action: "delete".into(),
            resource: "file".into(),
            resource_id: None,
            details: None,
            ip_address: None,
        },
    )
    .await
    .unwrap();

    AuditLogRepository::create(
        pool,
        CreateAuditLogData {
            user_id: user2,
            action: "upload".into(),
            resource: "file".into(),
            resource_id: None,
            details: None,
            ip_address: None,
        },
    )
    .await
    .unwrap();

    let all = AuditLogRepository::list(pool, None, None, 0, 100).await.unwrap();
    assert_eq!(all.len(), 3);

    let by_user1 = AuditLogRepository::list(pool, Some(user1), None, 0, 100)
        .await
        .unwrap();
    assert_eq!(by_user1.len(), 2);

    let by_upload = AuditLogRepository::list(pool, None, Some("upload"), 0, 100)
        .await
        .unwrap();
    assert_eq!(by_upload.len(), 2);
}
