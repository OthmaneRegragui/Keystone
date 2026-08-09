use std::sync::Arc;
use std::time::Duration;

use crate::db::Database;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::utils::workers::metrics::WorkerMetrics;

const WORKER_NAME: &str = "refcount";

pub async fn run_refcount_repair(
    db: &Database,
    interval: Duration,
    mut shutdown: broadcast::Receiver<()>,
    metrics: Arc<WorkerMetrics>,
) {
    info!(
        "refcount repair started with interval {:?}",
        interval
    );

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                match run_refcount_cycle(db, &metrics).await {
                    Ok(discrepancies) => {
                        metrics.record_run(WORKER_NAME);
                        if discrepancies > 0 {
                            warn!(
                                "refcount check found {} discrepancies",
                                discrepancies
                            );
                        }
                    }
                    Err(e) => {
                        metrics.record_failure(WORKER_NAME);
                        error!("refcount check cycle failed: {}", e);
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("refcount repair shutting down");
                break;
            }
        }
    }
}

async fn run_refcount_cycle(
    db: &Database,
    metrics: &WorkerMetrics,
) -> Result<u64, String> {
    let pool = db.pool();

    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        r#"SELECT f.id, f.ref_count, COUNT(so.id)
           FROM files f
           LEFT JOIN storage_objects so ON f.id = so.file_id
           GROUP BY f.id
           HAVING f.ref_count != COUNT(so.id)"#,
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("failed to query refcount discrepancies: {}", e))?;

    let discrepancies = rows.len() as u64;

    for (file_id, db_ref_count, storage_count) in &rows {
        warn!(
            "refcount discrepancy for file {}: db_ref_count={}, storage_object_count={}",
            file_id, db_ref_count, storage_count
        );
        metrics.add_refcount_discrepancy();
    }

    Ok(discrepancies)
}
