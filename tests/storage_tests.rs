mod helpers;

use keystone::storage::local::LocalFsBackend;
use keystone::utils::traits::StorageBackend;
use bytes::Bytes;

#[tokio::test]
async fn test_local_fs_put_and_get() {
    let temp_dir = helpers::setup_test_storage();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");

    let data = Bytes::from("Hello, World!");
    backend.put("test/file.txt", data.clone())
        .await
        .expect("Failed to put file");

    let retrieved = backend.get("test/file.txt")
        .await
        .expect("Failed to get file");

    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap(), data);
}

#[tokio::test]
async fn test_local_fs_exists() {
    let temp_dir = helpers::setup_test_storage();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");

    assert!(!backend.exists("nonexistent.txt").await.unwrap());

    backend.put("exists.txt", Bytes::from("test"))
        .await
        .expect("Failed to put file");

    assert!(backend.exists("exists.txt").await.unwrap());
}

#[tokio::test]
async fn test_local_fs_delete() {
    let temp_dir = helpers::setup_test_storage();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");

    backend.put("delete_me.txt", Bytes::from("test"))
        .await
        .expect("Failed to put file");

    let deleted = backend.delete("delete_me.txt")
        .await
        .expect("Failed to delete file");

    assert!(deleted);
    assert!(!backend.exists("delete_me.txt").await.unwrap());
}

#[tokio::test]
async fn test_local_fs_delete_nonexistent() {
    let temp_dir = helpers::setup_test_storage();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");

    let deleted = backend.delete("nonexistent.txt")
        .await
        .expect("Failed to delete file");

    assert!(!deleted);
}

#[tokio::test]
async fn test_local_fs_list() {
    let temp_dir = helpers::setup_test_storage();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");

    backend.put("dir/file1.txt", Bytes::from("test1"))
        .await
        .expect("Failed to put file");
    backend.put("dir/file2.txt", Bytes::from("test2"))
        .await
        .expect("Failed to put file");
    backend.put("other/file3.txt", Bytes::from("test3"))
        .await
        .expect("Failed to put file");

    let files = backend.list("dir/")
        .await
        .expect("Failed to list files");

    assert_eq!(files.len(), 2);
}
