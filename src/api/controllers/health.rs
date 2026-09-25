use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue};
use axum::Json;
use crate::db::repos::{AdminSettingRepository, UserRepository};
use crate::error::{AppError, AppResult};
use serde_json::{json, Value};
use sysinfo::{ProcessRefreshKind, System};

use crate::api::extractors::AuthUser;
use crate::dto::{DatabaseStatsDto, MessageResponse, ProcessMetricsDto, SystemStatsDto};
use crate::AppState;

static SYSTEM_METRICS: OnceLock<Mutex<System>> = OnceLock::new();

fn system_metrics() -> &'static Mutex<System> {
    SYSTEM_METRICS.get_or_init(|| {
        let mut system = System::new();
        if let Ok(pid) = sysinfo::get_current_pid() {
            system.refresh_pids_specifics(
                &[pid],
                ProcessRefreshKind::new().with_memory().with_cpu(),
            );
        }
        Mutex::new(system)
    })
}

#[derive(sqlx::FromRow)]
struct DatabaseStatsRow {
    size_bytes: i64,
    active_connections: i64,
    total_connections: i64,
    cache_hit_percent: f64,
}

async fn database_stats(pool: &sqlx::PgPool) -> AppResult<DatabaseStatsDto> {
    let row = sqlx::query_as::<_, DatabaseStatsRow>(
        r#"
        SELECT
            pg_database_size(current_database()) AS size_bytes,
            (
                SELECT COUNT(*)::bigint
                FROM pg_stat_activity
                WHERE datname = current_database() AND state = 'active'
            ) AS active_connections,
            (
                SELECT COUNT(*)::bigint
                FROM pg_stat_activity
                WHERE datname = current_database()
            ) AS total_connections,
            COALESCE(
                100.0 * blks_hit / NULLIF(blks_hit + blks_read, 0),
                100.0
            )::double precision AS cache_hit_percent
        FROM pg_stat_database
        WHERE datname = current_database()
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|error| {
        AppError::Internal(format!("failed to collect database statistics: {error}"))
    })?;

    let total_connections = u64::try_from(row.total_connections).unwrap_or(0);
    let active_connections = u64::try_from(row.active_connections)
        .unwrap_or(0)
        .min(total_connections);
    let cache_hit_percent = if row.cache_hit_percent.is_finite() {
        (row.cache_hit_percent.clamp(0.0, 100.0) * 10.0).round() / 10.0
    } else {
        0.0
    };

    Ok(DatabaseStatsDto {
        size_bytes: u64::try_from(row.size_bytes).unwrap_or(0),
        active_connections,
        total_connections,
        cache_hit_percent,
    })
}

pub fn initialize_system_metrics() {
    let _ = system_metrics();
}

pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "keystone-api",
    }))
}

pub async fn system_stats(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<(HeaderMap, Json<SystemStatsDto>)> {
    auth.require_ui_session()?;
    auth.require_admin()?;

    let app_pid = sysinfo::get_current_pid().map_err(|error| {
        AppError::Internal(format!("failed to read Keystone process id: {error}"))
    })?;
    let app = {
        let mut system = system_metrics()
            .lock()
            .map_err(|_| AppError::Internal("system metrics lock poisoned".into()))?;
        system.refresh_pids_specifics(
            &[app_pid],
            ProcessRefreshKind::new().with_memory().with_cpu(),
        );
        let app_process = system
            .process(app_pid)
            .ok_or_else(|| AppError::Internal("Keystone app process not found".into()))?;

        ProcessMetricsDto {
            cpu_usage_percent: (f64::from(app_process.cpu_usage().max(0.0)) * 10.0)
                .round()
                / 10.0,
            memory_used_bytes: app_process.memory(),
        }
    };
    let database = match tokio::time::timeout(
        Duration::from_secs(2),
        database_stats(state.db.pool()),
    )
    .await
    {
        Ok(Ok(stats)) => Some(stats),
        Ok(Err(error)) => {
            tracing::warn!(%error, "failed to collect database statistics");
            None
        }
        Err(_) => {
            tracing::warn!("database statistics request timed out");
            None
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    Ok((
        headers,
        Json(SystemStatsDto { app, database }),
    ))
}

pub async fn ready(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<MessageResponse>> {
    UserRepository::count(state.db.pool()).await?;

    Ok(Json(MessageResponse {
        message: "ready".to_string(),
    }))
}

pub async fn public_settings(
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let block = AdminSettingRepository::get_bool(state.db.pool(), "block_registrations")
        .await
        .unwrap_or(true);
    Json(json!({
        "block_registrations": block,
    }))
}
