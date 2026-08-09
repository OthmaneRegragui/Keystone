use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, repos::{FileRepository, StorageObjectRepository}};
use crate::storage::StorageRegistry;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::utils::workers::metrics::WorkerMetrics;

const WORKER_NAME: &str = "integrity";

pub async fn run_integrity_check(
    db: &Database,
    storage: &StorageRegistry,
    interval: Duration,
    mut shutdown: broadcast::Receiver<()>,
    metrics: Arc<WorkerMetrics>,
) {
    info!(
        "integrity checker started with interval {:?}",
        interval
    );

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                match run_integrity_cycle(db, storage, &metrics).await {
                    Ok((checked, mismatches)) => {
                        metrics.record_run(WORKER_NAME);
                        if checked > 0 {
                            info!(
                                "integrity check completed: {} files checked, {} mismatches",
                                checked, mismatches
                            );
                        }
                    }
                    Err(e) => {
                        metrics.record_failure(WORKER_NAME);
                        error!("integrity check cycle failed: {}", e);
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("integrity checker shutting down");
                break;
            }
        }
    }
}

async fn run_integrity_cycle(
    db: &Database,
    storage: &StorageRegistry,
    metrics: &WorkerMetrics,
) -> Result<(u64, u64), String> {
    let batch_size = 20i64;
    let pool = db.pool();

    let files = FileRepository::list(pool, 0, batch_size, None)
        .await
        .map_err(|e| format!("failed to list files: {}", e))?;

    if files.is_empty() {
        return Ok((0, 0));
    }

    let mut checked = 0u64;
    let mut mismatches = 0u64;

    for file in &files {
        let storage_objects = StorageObjectRepository::find_by_file_id(pool, file.id)
            .await
            .map_err(|e| format!("failed to query storage objects for file {}: {}", file.id, e))?;

        if storage_objects.is_empty() {
            warn!(
                "file {} (hash: {}) has no storage objects, skipping",
                file.id, file.blake3_hash
            );
            continue;
        }

        let obj = &storage_objects[0];

        let backend = match storage.get(&obj.backend) {
            Some(b) => b,
            None => {
                warn!(
                    "backend '{}' not found for storage object '{}', skipping",
                    obj.backend, obj.storage_path
                );
                continue;
            }
        };

        let data = match backend.get(&obj.storage_path).await {
            Ok(Some(d)) => d,
            Ok(None) => {
                warn!(
                    "storage object '{}' not found in backend '{}'",
                    obj.storage_path, obj.backend
                );
                metrics.add_integrity_mismatch();
                mismatches += 1;
                continue;
            }
            Err(e) => {
                warn!(
                    "failed to read storage object '{}' from backend '{}': {}",
                    obj.storage_path, obj.backend, e
                );
                continue;
            }
        };

        let computed_hash = crate::utils::hashing::blake3::hash_bytes(&data).await;
        checked += 1;

        if computed_hash != file.blake3_hash {
            warn!(
                "integrity mismatch for file {} ({}): expected {}, got {}",
                file.id, file.original_name, file.blake3_hash, computed_hash
            );
            metrics.add_integrity_mismatch();
            mismatches += 1;
        }

        metrics.add_integrity_check();
    }

    Ok((checked, mismatches))
}
