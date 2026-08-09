use std::sync::Arc;
use std::time::Duration;

use crate::config::WorkerConfig;
use crate::db::Database;
use crate::storage::StorageRegistry;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::info;

use crate::utils::workers::metrics::WorkerMetrics;

pub struct WorkerScheduler {
    handles: Vec<JoinHandle<()>>,
    shutdown_tx: broadcast::Sender<()>,
    metrics: Arc<WorkerMetrics>,
}

impl WorkerScheduler {
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            handles: Vec::new(),
            shutdown_tx,
            metrics: WorkerMetrics::shared(),
        }
    }

    pub fn metrics(&self) -> Arc<WorkerMetrics> {
        self.metrics.clone()
    }

    pub fn start_all(
        &mut self,
        config: &WorkerConfig,
        db: Database,
        storage: StorageRegistry,
    ) {
        let interval = Duration::from_millis(config.poll_interval_ms);
        let db = Arc::new(db);
        let storage = Arc::new(storage);

        info!(
            "starting all workers with interval {:?}, batch_size {}",
            interval, config.batch_size
        );

        let gc_handle = {
            let rx = self.shutdown_tx.subscribe();
            let db = Arc::clone(&db);
            let storage = Arc::clone(&storage);
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::utils::workers::gc::run_gc(&db, &storage, interval, rx, metrics).await;
            })
        };
        self.handles.push(gc_handle);

        let integrity_handle = {
            let rx = self.shutdown_tx.subscribe();
            let db = Arc::clone(&db);
            let storage = Arc::clone(&storage);
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::utils::workers::integrity::run_integrity_check(&db, &storage, interval, rx, metrics)
                    .await;
            })
        };
        self.handles.push(integrity_handle);

        let refcount_handle = {
            let rx = self.shutdown_tx.subscribe();
            let db = Arc::clone(&db);
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::utils::workers::refcount::run_refcount_repair(&db, interval, rx, metrics).await;
            })
        };
        self.handles.push(refcount_handle);

        let cleanup_handle = {
            let rx = self.shutdown_tx.subscribe();
            let db = Arc::clone(&db);
            let storage = Arc::clone(&storage);
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::utils::workers::cleanup::run_cleanup(&db, &storage, interval, rx, metrics).await;
            })
        };
        self.handles.push(cleanup_handle);

        let stats_handle = {
            let rx = self.shutdown_tx.subscribe();
            let db = Arc::clone(&db);
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                crate::utils::workers::stats::run_stats(&db, interval, rx, metrics).await;
            })
        };
        self.handles.push(stats_handle);

        info!("all {} workers started", self.handles.len());
    }

    pub fn shutdown(&self) {
        info!("sending shutdown signal to all workers");
        let _ = self.shutdown_tx.send(());
        for handle in &self.handles {
            handle.abort();
        }
    }

    pub fn worker_count(&self) -> usize {
        self.handles.len()
    }
}

impl Default for WorkerScheduler {
    fn default() -> Self {
        Self::new()
    }
}
