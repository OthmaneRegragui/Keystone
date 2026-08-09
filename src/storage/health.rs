use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use crate::utils::traits::StorageBackend;

#[derive(Debug, Clone)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct HealthCheck {
    pub backend_name: String,
    pub status: HealthStatus,
    pub latency_ms: u64,
    pub last_checked: DateTime<Utc>,
}

const HEALTH_TEST_KEY: &str = "__health_check__";

pub async fn check_backend(name: &str, backend: &dyn StorageBackend) -> HealthCheck {
    let start = std::time::Instant::now();
    let status = run_health_probe(backend).await;
    let latency_ms = start.elapsed().as_millis() as u64;

    HealthCheck {
        backend_name: name.to_string(),
        status,
        latency_ms,
        last_checked: Utc::now(),
    }
}

async fn run_health_probe(backend: &dyn StorageBackend) -> HealthStatus {
    let test_value = Bytes::from("health");

    let put_result = backend.put(HEALTH_TEST_KEY, test_value).await;
    if let Err(e) = put_result {
        tracing::error!("health check put failed: {e}");
        return HealthStatus::Unhealthy;
    }

    match backend.get(HEALTH_TEST_KEY).await {
        Ok(Some(_)) => {
            let _ = backend.delete(HEALTH_TEST_KEY).await;
            HealthStatus::Healthy
        }
        Ok(None) => {
            tracing::warn!("health check get returned None after successful put");
            HealthStatus::Degraded
        }
        Err(e) => {
            tracing::error!("health check get failed: {e}");
            HealthStatus::Unhealthy
        }
    }
}

pub async fn check_all_backends(
    backends: &HashMap<String, Arc<dyn StorageBackend>>,
) -> Vec<HealthCheck> {
    let mut results = Vec::with_capacity(backends.len());
    for (name, backend) in backends {
        results.push(check_backend(name, backend.as_ref()).await);
    }
    results
}
