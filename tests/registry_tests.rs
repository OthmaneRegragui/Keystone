mod helpers;

use std::sync::Arc;

use keystone::storage::StorageRegistry;
use keystone::storage::local::LocalFsBackend;
use keystone::utils::traits::StorageBackend;

#[test]
fn test_registry_new() {
    let registry = StorageRegistry::new();
    assert!(registry.list_backends().is_empty());
}

#[test]
fn test_registry_register() {
    let temp_dir = helpers::setup_test_storage();
    let backend =
        Arc::new(LocalFsBackend::new(temp_dir.path()).unwrap()) as Arc<dyn StorageBackend>;

    let mut registry = StorageRegistry::new();
    registry.register("local", backend);

    assert!(registry.get("local").is_some());
    assert_eq!(registry.list_backends().len(), 1);
}

#[test]
fn test_registry_register_multiple() {
    let temp_dir1 = helpers::setup_test_storage();
    let temp_dir2 = helpers::setup_test_storage();
    let backend1 =
        Arc::new(LocalFsBackend::new(temp_dir1.path()).unwrap()) as Arc<dyn StorageBackend>;
    let backend2 =
        Arc::new(LocalFsBackend::new(temp_dir2.path()).unwrap()) as Arc<dyn StorageBackend>;

    let mut registry = StorageRegistry::new();
    registry.register("first", backend1);
    registry.register("second", backend2);

    assert_eq!(registry.list_backends().len(), 2);
    assert!(registry.get("first").is_some());
    assert!(registry.get("second").is_some());
}

#[test]
fn test_registry_remove() {
    let temp_dir1 = helpers::setup_test_storage();
    let temp_dir2 = helpers::setup_test_storage();
    let backend1 =
        Arc::new(LocalFsBackend::new(temp_dir1.path()).unwrap()) as Arc<dyn StorageBackend>;
    let backend2 =
        Arc::new(LocalFsBackend::new(temp_dir2.path()).unwrap()) as Arc<dyn StorageBackend>;

    let mut registry = StorageRegistry::new();
    registry.register("first", backend1);
    registry.register("second", backend2);

    assert!(registry.remove("second"));
    assert_eq!(registry.list_backends().len(), 1);
    assert!(registry.get("second").is_none());

    assert!(!registry.remove("nonexistent"));
}

#[test]
fn test_registry_remove_last() {
    let temp_dir = helpers::setup_test_storage();
    let backend =
        Arc::new(LocalFsBackend::new(temp_dir.path()).unwrap()) as Arc<dyn StorageBackend>;

    let mut registry = StorageRegistry::new();
    registry.register("only", backend);

    assert!(registry.remove("only"));
    assert!(registry.list_backends().is_empty());
}
