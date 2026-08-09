use std::sync::Arc;
use std::time::Duration;

use crate::db::{Database, repos::UserRepository};
use tokio::sync::broadcast;
use tracing::{error, info};

use crate::utils::workers::metrics::WorkerMetrics;

const WORKER_NAME: &str = "stats";

#[derive(Debug)]
pub struct StorageStats {
    pub total_files: i64,
    pub total_size: i64,
    pub unique_hashes: i64,
    pub dedup_savings: i64,
    pub users_count: i64,
}

impl std::fmt::Display for StorageStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Storage Stats ===")?;
        writeln!(f, "Total files: {}", self.total_files)?;
        writeln!(f, "Total size: {} bytes", self.total_size)?;
        writeln!(f, "Unique hashes: {}", self.unique_hashes)?;
        writeln!(f, "Dedup savings: {} bytes", self.dedup_savings)?;
        writeln!(f, "Users: {}", self.users_count)?;
        Ok(())
    }
}

pub async fn run_stats(
    db: &Database,
    interval: Duration,
    mut shutdown: broadcast::Receiver<()>,
    metrics: Arc<WorkerMetrics>,
) {
    info!("stats worker started with interval {:?}", interval);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {
                match run_stats_cycle(db).await {
                    Ok(stats) => {
                        metrics.record_run(WORKER_NAME);
                        info!("{}", stats);
                    }
                    Err(e) => {
                        metrics.record_failure(WORKER_NAME);
                        error!("stats cycle failed: {}", e);
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("stats worker shutting down");
                break;
            }
        }
    }
}

async fn run_stats_cycle(db: &Database) -> Result<StorageStats, String> {
    let pool = db.pool();

    let (total_files,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM files")
        .fetch_one(pool)
        .await
        .map_err(|e| format!("failed to count files: {}", e))?;

    let (total_size,): (i64,) =
        sqlx::query_as("SELECT COALESCE(SUM(size), 0)::BIGINT FROM files")
            .fetch_one(pool)
            .await
            .map_err(|e| format!("failed to sum file sizes: {}", e))?;

    let (unique_hashes,): (i64,) =
        sqlx::query_as("SELECT COUNT(DISTINCT blake3_hash) FROM files")
            .fetch_one(pool)
            .await
            .map_err(|e| format!("failed to count unique hashes: {}", e))?;

    let (dedup_savings,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM((ref_count - 1) * size), 0)::BIGINT FROM files WHERE ref_count > 1",
    )
    .fetch_one(pool)
    .await
    .map_err(|e| format!("failed to compute dedup savings: {}", e))?;

    let users_count = UserRepository::count(pool)
        .await
        .map_err(|e| format!("failed to count users: {}", e))?;

    Ok(StorageStats {
        total_files,
        total_size,
        unique_hashes,
        dedup_savings,
        users_count,
    })
}
