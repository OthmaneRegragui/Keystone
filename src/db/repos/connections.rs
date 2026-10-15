use chrono::Utc;
use sqlx::PgPool;

use crate::db::is_unique_violation;
use crate::error::{AppError, AppResult};
use crate::models::Connection;

/// Columns selected for every `Connection` read, in `ConnectionRow` order.
const COLUMNS: &str = "id, name, connection_type, config, created_at";

/// Raw `connections` row. `created_at` is stored as an RFC 3339 string.
#[derive(sqlx::FromRow)]
struct ConnectionRow {
    id: String,
    name: String,
    connection_type: String,
    config: String,
    created_at: String,
}

impl ConnectionRow {
    fn into_connection(self) -> Connection {
        Connection {
            id: self.id,
            name: self.name,
            connection_type: self.connection_type,
            config: self.config,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .unwrap_or_default()
                .with_timezone(&Utc),
        }
    }
}

pub struct ConnectionRepository;

impl ConnectionRepository {
    pub async fn list(pool: &PgPool) -> AppResult<Vec<Connection>> {
        let rows: Vec<ConnectionRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM connections ORDER BY name ASC"))
                .fetch_all(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to list connections: {e}")))?;

        Ok(rows
            .into_iter()
            .map(ConnectionRow::into_connection)
            .collect())
    }

    pub async fn find_by_id(pool: &PgPool, id: &str) -> AppResult<Option<Connection>> {
        let row: Option<ConnectionRow> =
            sqlx::query_as(&format!("SELECT {COLUMNS} FROM connections WHERE id = $1"))
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to find connection: {e}")))?;

        Ok(row.map(ConnectionRow::into_connection))
    }

    pub async fn create(
        pool: &PgPool,
        name: &str,
        connection_type: &str,
        config: &str,
    ) -> AppResult<Connection> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            "INSERT INTO connections (id, name, connection_type, config, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(name)
        .bind(connection_type)
        .bind(config)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("connection '{name}' already exists"))
            } else {
                AppError::Internal(format!("failed to create connection: {e}"))
            }
        })?;

        Ok(Connection {
            id,
            name: name.to_string(),
            connection_type: connection_type.to_string(),
            config: config.to_string(),
            created_at: Utc::now(),
        })
    }

    pub async fn delete(pool: &PgPool, id: &str) -> AppResult<()> {
        let affected = sqlx::query("DELETE FROM connections WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete connection: {e}")))?
            .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound("connection not found".into()));
        }
        Ok(())
    }
}
