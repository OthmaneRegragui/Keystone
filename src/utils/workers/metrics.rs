use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug)]
pub struct WorkerMetrics {
    pub tasks_run: AtomicU64,
    pub tasks_failed: AtomicU64,
    pub total_gc_deleted: AtomicU64,
    pub total_orphans_cleaned: AtomicU64,
    pub total_integrity_checks: AtomicU64,
    pub total_integrity_mismatches: AtomicU64,
    pub total_refcount_discrepancies: AtomicU64,
    last_run_times: Mutex<HashMap<String, Instant>>,
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub tasks_run: u64,
    pub tasks_failed: u64,
    pub total_gc_deleted: u64,
    pub total_orphans_cleaned: u64,
    pub total_integrity_checks: u64,
    pub total_integrity_mismatches: u64,
    pub total_refcount_discrepancies: u64,
    pub last_run_times: HashMap<String, Instant>,
}

impl WorkerMetrics {
    pub fn new() -> Self {
        Self {
            tasks_run: AtomicU64::new(0),
            tasks_failed: AtomicU64::new(0),
            total_gc_deleted: AtomicU64::new(0),
            total_orphans_cleaned: AtomicU64::new(0),
            total_integrity_checks: AtomicU64::new(0),
            total_integrity_mismatches: AtomicU64::new(0),
            total_refcount_discrepancies: AtomicU64::new(0),
            last_run_times: Mutex::new(HashMap::new()),
        }
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    pub fn record_run(&self, worker_name: &str) {
        self.tasks_run.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut times) = self.last_run_times.lock() {
            times.insert(worker_name.to_string(), Instant::now());
        }
    }

    pub fn record_failure(&self, worker_name: &str) {
        self.tasks_failed.fetch_add(1, Ordering::Relaxed);
        tracing::error!("worker '{}' encountered an error", worker_name);
    }

    pub fn add_gc_deleted(&self, count: u64) {
        self.total_gc_deleted.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_orphans_cleaned(&self, count: u64) {
        self.total_orphans_cleaned
            .fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_integrity_mismatch(&self) {
        self.total_integrity_mismatches
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_integrity_check(&self) {
        self.total_integrity_checks
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_refcount_discrepancy(&self) {
        self.total_refcount_discrepancies
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            tasks_run: self.tasks_run.load(Ordering::Relaxed),
            tasks_failed: self.tasks_failed.load(Ordering::Relaxed),
            total_gc_deleted: self.total_gc_deleted.load(Ordering::Relaxed),
            total_orphans_cleaned: self.total_orphans_cleaned.load(Ordering::Relaxed),
            total_integrity_checks: self.total_integrity_checks.load(Ordering::Relaxed),
            total_integrity_mismatches: self
                .total_integrity_mismatches
                .load(Ordering::Relaxed),
            total_refcount_discrepancies: self
                .total_refcount_discrepancies
                .load(Ordering::Relaxed),
            last_run_times: self
                .last_run_times
                .lock()
                .map(|m| m.clone())
                .unwrap_or_default(),
        }
    }
}

impl Default for WorkerMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MetricsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "=== Worker Metrics ===")?;
        writeln!(f, "Tasks run: {}", self.tasks_run)?;
        writeln!(f, "Tasks failed: {}", self.tasks_failed)?;
        writeln!(f, "GC deleted: {}", self.total_gc_deleted)?;
        writeln!(f, "Orphans cleaned: {}", self.total_orphans_cleaned)?;
        writeln!(f, "Integrity checks: {}", self.total_integrity_checks)?;
        writeln!(
            f,
            "Integrity mismatches: {}",
            self.total_integrity_mismatches
        )?;
        writeln!(
            f,
            "Refcount discrepancies: {}",
            self.total_refcount_discrepancies
        )?;
        for (name, time) in &self.last_run_times {
            writeln!(f, "  {} last ran: {:?}", name, time.elapsed())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_new() {
        let metrics = WorkerMetrics::new();
        assert_eq!(metrics.tasks_run.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.tasks_failed.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.total_gc_deleted.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_record_run() {
        let metrics = WorkerMetrics::new();
        metrics.record_run("gc");
        metrics.record_run("gc");
        assert_eq!(metrics.tasks_run.load(Ordering::Relaxed), 2);
        assert!(metrics
            .last_run_times
            .lock()
            .unwrap()
            .contains_key("gc"));
    }

    #[test]
    fn test_record_failure() {
        let metrics = WorkerMetrics::new();
        metrics.record_failure("integrity");
        assert_eq!(metrics.tasks_failed.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_snapshot() {
        let metrics = WorkerMetrics::new();
        metrics.record_run("gc");
        metrics.add_gc_deleted(5);
        let snap = metrics.snapshot();
        assert_eq!(snap.tasks_run, 1);
        assert_eq!(snap.total_gc_deleted, 5);
    }

    #[test]
    fn test_display() {
        let metrics = WorkerMetrics::new();
        metrics.record_run("test");
        metrics.add_gc_deleted(3);
        let snap = metrics.snapshot();
        let display = snap.to_string();
        assert!(display.contains("Tasks run: 1"));
        assert!(display.contains("GC deleted: 3"));
    }
}
