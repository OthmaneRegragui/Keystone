mod helpers;

use keystone::db::repos::FileRepository;
use keystone::db::rows::FileRecord;

#[tokio::test]
async fn test_create_and_find_file() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let file = FileRepository::create(
        pool,
        FileRecord::new(
            "abc123def456".to_string(),
            "test.txt".to_string(),
            Some("text/plain".to_string()),
            1024,
        ),
    )
    .await
    .expect("Failed to create file");

    assert_eq!(file.original_name, "test.txt");
    assert_eq!(file.size, 1024);

    let found = FileRepository::find_by_id(pool, file.id)
        .await
        .expect("Failed to find file");

    assert!(found.is_some());
    assert_eq!(found.unwrap().blake3_hash, "abc123def456");
}

#[tokio::test]
async fn test_find_by_hash() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let hash = "unique_hash_12345";

    FileRepository::create(
        pool,
        FileRecord::new(
            hash.to_string(),
            "duplicate.txt".to_string(),
            None,
            512,
        ),
    )
    .await
    .expect("Failed to create file");

    let found = FileRepository::find_by_hash(pool, hash)
        .await
        .expect("Failed to find by hash");

    assert!(found.is_some());
    assert_eq!(found.unwrap().original_name, "duplicate.txt");
}

#[tokio::test]
async fn test_list_files() {
    let db = helpers::setup_reset_db().await;
    let pool = db.pool();

    for i in 0..5 {
        FileRepository::create(
            pool,
            FileRecord::new(
                format!("hash_{}", i),
                format!("file_{}.txt", i),
                None,
                100 * (i + 1),
            ),
        )
        .await
        .expect("Failed to create file");
    }

    let files = FileRepository::list(pool, 0, 3, None)
        .await
        .expect("Failed to list files");

    assert_eq!(files.len(), 3);

    let count = FileRepository::count(pool, None)
        .await
        .expect("Failed to count files");

    assert_eq!(count, 5);
}

#[tokio::test]
async fn test_delete_file() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let file = FileRepository::create(
        pool,
        FileRecord::new(
            "delete_me".to_string(),
            "deletable.txt".to_string(),
            None,
            100,
        ),
    )
    .await
    .expect("Failed to create file");

    let deleted = FileRepository::delete(pool, file.id)
        .await
        .expect("Failed to delete file");

    assert!(deleted);

    let found = FileRepository::find_by_id(pool, file.id)
        .await
        .expect("Failed to find file");

    assert!(found.is_none());
}

#[tokio::test]
async fn test_update_ref_count() {
    let db = helpers::setup_test_db().await;
    let pool = db.pool();

    let file = FileRepository::create(
        pool,
        FileRecord::new(
            "refcount_test".to_string(),
            "refcount.txt".to_string(),
            None,
            100,
        ),
    )
    .await
    .expect("Failed to create file");

    FileRepository::update_ref_count(pool, file.id, 2)
        .await
        .expect("Failed to update ref count");

    let updated = FileRepository::find_by_id(pool, file.id)
        .await
        .expect("Failed to find file")
        .unwrap();

    assert_eq!(updated.ref_count, 3);
}
