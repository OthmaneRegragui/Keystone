use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ProcessMetricsDto {
    pub cpu_usage_percent: f64,
    pub memory_used_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct DatabaseStatsDto {
    pub size_bytes: u64,
    pub active_connections: u64,
    pub total_connections: u64,
    pub cache_hit_percent: f64,
}

#[derive(Debug, Serialize)]
pub struct SystemStatsDto {
    pub app: ProcessMetricsDto,
    pub database: Option<DatabaseStatsDto>,
}
