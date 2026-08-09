use crate::config::DatabaseConfig;
use crate::error::{AppError, AppResult};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;
use tracing::info;

pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Connect and run migrations with default pool settings.
    pub async fn new(url: &str) -> AppResult<Self> {
        let config = DatabaseConfig {
            url: url.to_string(),
            ..DatabaseConfig::default()
        };
        Self::new_with_config(&config).await
    }

    /// Connect and run migrations, honoring the configured pool sizes and
    /// timeouts from `database` settings.
    pub async fn new_with_config(config: &DatabaseConfig) -> AppResult<Self> {
        let pool = build_pool(config).await?;

        run_migrations(&pool).await?;

        info!(
            "database initialized with migrations at {}",
            redact_db_url(&config.url)
        );
        Ok(Self { pool })
    }

    /// Connect without running migrations, with default pool settings.
    pub async fn connect(url: &str) -> AppResult<Self> {
        let config = DatabaseConfig {
            url: url.to_string(),
            ..DatabaseConfig::default()
        };
        let pool = build_pool(&config).await?;

        info!("database connected at {}", redact_db_url(&config.url));
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
        info!("database connection pool closed");
    }
}

/// Build a connection pool from the database configuration.
async fn build_pool(config: &DatabaseConfig) -> AppResult<PgPool> {
    PgPoolOptions::new()
        .max_connections(config.max_connections.max(1))
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.connect_timeout_secs))
        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
        .connect(&config.url)
        .await
        .map_err(|e| {
            AppError::Internal(redact_db_url(&format!(
                "failed to connect to database: {e}"
            )))
        })
}

/// Redact the password from a Postgres connection URL so credentials never
/// end up in logs or client-visible error details.
fn redact_db_url(url: &str) -> String {
    if let Some((userinfo, rest)) = url.split_once('@') {
        if let Some((user, _password)) = userinfo.rsplit_once(':') {
            return format!("{user}:****@{rest}");
        }
    }
    url.to_string()
}

pub async fn run_migrations(pool: &PgPool) -> AppResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to run migrations: {e}")))?;

    info!("database migrations completed successfully");
    Ok(())
}
