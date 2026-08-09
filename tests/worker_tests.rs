use std::sync::Arc;

use keystone::storage::StorageRegistry;
use keystone::storage::local::LocalFsBackend;
use keystone::utils::traits::StorageBackend;
use keystone::utils::workers::metrics::WorkerMetrics;
use keystone::utils::workers::scheduler::WorkerScheduler;
use keystone::utils::workers::stats::StorageStats;

#[test]
fn test_storage_stats_display() {
    let stats = StorageStats {
        total_files: 42,
        total_size: 1234567,
        unique_hashes: 10,
        dedup_savings: 50000,
        users_count: 5,
    };

    let display = stats.to_string();
    assert!(display.contains("=== Storage Stats ==="));
    assert!(display.contains("Total files: 42"));
    assert!(display.contains("Total size: 1234567 bytes"));
    assert!(display.contains("Unique hashes: 10"));
    assert!(display.contains("Dedup savings: 50000 bytes"));
    assert!(display.contains("Users: 5"));
}

#[test]
fn test_metrics_all_counters() {
    let metrics = WorkerMetrics::new();

    metrics.record_run("gc");
    metrics.record_run("cleanup");
    metrics.record_failure("integrity");
    metrics.add_gc_deleted(10);
    metrics.add_orphans_cleaned(5);
    metrics.add_integrity_check();
    metrics.add_integrity_check();
    metrics.add_integrity_mismatch();
    metrics.add_refcount_discrepancy();
    metrics.add_refcount_discrepancy();
    metrics.add_refcount_discrepancy();

    let snap = metrics.snapshot();
    assert_eq!(snap.tasks_run, 2);
    assert_eq!(snap.tasks_failed, 1);
    assert_eq!(snap.total_gc_deleted, 10);
    assert_eq!(snap.total_orphans_cleaned, 5);
    assert_eq!(snap.total_integrity_checks, 2);
    assert_eq!(snap.total_integrity_mismatches, 1);
    assert_eq!(snap.total_refcount_discrepancies, 3);
}

#[test]
fn test_metrics_shared() {
    let metrics = WorkerMetrics::shared();
    metrics.record_run("test");
    let snap = metrics.snapshot();
    assert_eq!(snap.tasks_run, 1);
}

#[test]
fn test_metrics_default() {
    let metrics = WorkerMetrics::default();
    let snap = metrics.snapshot();
    assert_eq!(snap.tasks_run, 0);
    assert_eq!(snap.tasks_failed, 0);
    assert_eq!(snap.total_gc_deleted, 0);
    assert_eq!(snap.total_orphans_cleaned, 0);
    assert_eq!(snap.total_integrity_checks, 0);
    assert_eq!(snap.total_integrity_mismatches, 0);
    assert_eq!(snap.total_refcount_discrepancies, 0);
    assert!(snap.last_run_times.is_empty());
}

#[test]
fn test_metrics_last_run_times() {
    let metrics = WorkerMetrics::new();
    metrics.record_run("gc");
    metrics.record_run("cleanup");
    metrics.record_run("integrity");

    let snap = metrics.snapshot();
    assert_eq!(snap.last_run_times.len(), 3);
    assert!(snap.last_run_times.contains_key("gc"));
    assert!(snap.last_run_times.contains_key("cleanup"));
    assert!(snap.last_run_times.contains_key("integrity"));
}

#[test]
fn test_metrics_snapshot_isolation() {
    let metrics = WorkerMetrics::new();
    metrics.record_run("gc");

    let snap1 = metrics.snapshot();
    assert_eq!(snap1.tasks_run, 1);

    metrics.record_run("gc");
    metrics.record_run("gc");

    assert_eq!(snap1.tasks_run, 1);

    let snap2 = metrics.snapshot();
    assert_eq!(snap2.tasks_run, 3);
}

#[test]
fn test_scheduler_new() {
    let scheduler = WorkerScheduler::new();
    assert_eq!(scheduler.worker_count(), 0);
}

#[test]
fn test_scheduler_default() {
    let scheduler = WorkerScheduler::default();
    assert_eq!(scheduler.worker_count(), 0);
}

#[test]
fn test_registry_list_backends_order() {
    let mut registry = StorageRegistry::new();

    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    for (i, dir) in dirs.iter().enumerate() {
        let name = format!("backend_{}", i);
        let backend =
            Arc::new(LocalFsBackend::new(dir.path()).unwrap()) as Arc<dyn StorageBackend>;
        registry.register(name, backend);
    }

    let backends = registry.list_backends();
    assert_eq!(backends.len(), 3);
    assert!(backends.contains(&"backend_0".to_string()));
    assert!(backends.contains(&"backend_1".to_string()));
    assert!(backends.contains(&"backend_2".to_string()));
}

#[test]
fn test_registry_overwrite_backend() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();

    let mut registry = StorageRegistry::new();

    let backend1 =
        Arc::new(LocalFsBackend::new(dir1.path()).unwrap()) as Arc<dyn StorageBackend>;
    registry.register("shared", backend1);

    let backend2 =
        Arc::new(LocalFsBackend::new(dir2.path()).unwrap()) as Arc<dyn StorageBackend>;
    registry.register("shared", backend2.clone());

    assert_eq!(registry.list_backends().len(), 1);

    let current = registry.get("shared").unwrap();
    assert!(Arc::ptr_eq(&backend2, &current));
}
