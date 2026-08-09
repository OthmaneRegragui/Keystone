use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, repos::StorageObjectRepository};
use crate::storage::StorageRegistry;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::utils::workers::metrics::WorkerMetrics;

const WORKER_NAME: &str = "cleanup";

pub async fn run_cleanup(
    db: &Database,
    storage: &StorageRegistry,
    interval: Duration,
    mut shutdown: broadcast::Receiver<()>,
    metrics: Arc<WorkerMetrics>,
) {
    info!("orphan cleanup started with interval {:?}", interval);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                match run_cleanup_cycle(db, storage, &metrics).await {
                    Ok(cleaned) => {
                        metrics.record_run(WORKER_NAME);
                        if cleaned > 0 {
                            info!("cleanup cycle completed: removed {} orphaned objects", cleaned);
                        }
                    }
                    Err(e) => {
                        metrics.record_failure(WORKER_NAME);
                        error!("cleanup cycle failed: {}", e);
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("orphan cleanup shutting down");
                break;
            }
        }
    }
}

async fn run_cleanup_cycle(
    db: &Database,
    storage: &StorageRegistry,
    metrics: &WorkerMetrics,
) -> Result<u64, String> {
    let pool = db.pool();

    let orphans = StorageObjectRepository::find_orphaned(pool)
        .await
        .map_err(|e| format!("failed to query orphaned storage objects: {}", e))?;

    if orphans.is_empty() {
        return Ok(0);
    }

    let mut cleaned = 0u64;

    for obj in &orphans {
        if let Some(backend) = storage.get(&obj.backend) {
            match backend.delete(&obj.storage_path).await {
                Ok(true) => {
                    info!(
                        "deleted orphaned storage object '{}' from backend '{}'",
                        obj.storage_path, obj.backend
                    );
                }
                Ok(false) => {
                    warn!(
                        "orphaned storage object '{}' already absent from backend '{}'",
                        obj.storage_path, obj.backend
                    );
                }
                Err(e) => {
                    warn!(
                        "failed to delete orphaned storage object '{}' from backend '{}': {}",
                        obj.storage_path, obj.backend, e
                    );
                }
            }
        } else {
            warn!(
                "backend '{}' not found for orphaned object '{}', skipping storage deletion",
                obj.backend, obj.storage_path
            );
        }

        match StorageObjectRepository::delete(pool, obj.id).await {
            Ok(true) => {
                cleaned += 1;
            }
            Ok(false) => {
                warn!("orphaned storage object record {} already deleted", obj.id);
            }
            Err(e) => {
                warn!("failed to delete orphaned storage object record {}: {}", obj.id, e);
            }
        }
    }

    metrics.add_orphans_cleaned(cleaned);
    Ok(cleaned)
}
