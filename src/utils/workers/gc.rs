use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, repos::{FileRepository, StorageObjectRepository}};
use crate::storage::StorageRegistry;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::utils::workers::metrics::WorkerMetrics;

const WORKER_NAME: &str = "gc";

pub async fn run_gc(
    db: &Database,
    storage: &StorageRegistry,
    interval: Duration,
    mut shutdown: broadcast::Receiver<()>,
    metrics: Arc<WorkerMetrics>,
) {
    info!("garbage collector started with interval {:?}", interval);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                match run_gc_cycle(db, storage, &metrics).await {
                    Ok(deleted) => {
                        metrics.record_run(WORKER_NAME);
                        if deleted > 0 {
                            info!("gc cycle completed: deleted {} files", deleted);
                        }
                    }
                    Err(e) => {
                        metrics.record_failure(WORKER_NAME);
                        error!("gc cycle failed: {}", e);
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("garbage collector shutting down");
                break;
            }
        }
    }
}

async fn run_gc_cycle(
    db: &Database,
    storage: &StorageRegistry,
    metrics: &WorkerMetrics,
) -> Result<u64, String> {
    let batch_size = 50i64;
    let pool = db.pool();

    let files = FileRepository::get_zero_ref_files(pool, batch_size)
        .await
        .map_err(|e| format!("failed to query zero-ref files: {}", e))?;

    if files.is_empty() {
        return Ok(0);
    }

    let mut deleted_count = 0u64;

    for file in &files {
        let storage_objects = StorageObjectRepository::find_by_file_id(pool, file.id)
            .await
            .map_err(|e| format!("failed to query storage objects for file {}: {}", file.id, e))?;

        for obj in &storage_objects {
            if let Some(backend) = storage.get(&obj.backend) {
                match backend.delete(&obj.storage_path).await {
                    Ok(true) => {
                        info!(
                            "deleted storage object '{}' from backend '{}'",
                            obj.storage_path, obj.backend
                        );
                    }
                    Ok(false) => {
                        warn!(
                            "storage object '{}' not found in backend '{}'",
                            obj.storage_path, obj.backend
                        );
                    }
                    Err(e) => {
                        warn!(
                            "failed to delete storage object '{}' from backend '{}': {}",
                            obj.storage_path, obj.backend, e
                        );
                    }
                }
            } else {
                warn!(
                    "backend '{}' not found for storage object '{}', skipping",
                    obj.backend, obj.storage_path
                );
            }

            if let Err(e) = StorageObjectRepository::delete(pool, obj.id).await {
                warn!(
                    "failed to delete storage object record {}: {}",
                    obj.id, e
                );
            }
        }

        match FileRepository::delete(pool, file.id).await {
            Ok(true) => {
                info!(
                    "deleted file record {} (hash: {})",
                    file.id, file.blake3_hash
                );
                deleted_count += 1;
            }
            Ok(false) => {
                warn!("file record {} already deleted", file.id);
            }
            Err(e) => {
                warn!("failed to delete file record {}: {}", file.id, e);
            }
        }
    }

    metrics.add_gc_deleted(deleted_count);
    Ok(deleted_count)
}
