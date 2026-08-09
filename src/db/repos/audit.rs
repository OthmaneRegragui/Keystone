use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::AuditLog;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::{AuditLogRow, CreateAuditLogData};

pub struct AuditLogRepository;

impl AuditLogRepository {
    pub async fn create(pool: &PgPool, data: CreateAuditLogData) -> AppResult<AuditLog> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        sqlx::query(
            r#"INSERT INTO audit_logs (id, user_id, action, resource, resource_id, details, ip_address, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        )
        .bind(&id)
        .bind(data.user_id.to_string())
        .bind(&data.action)
        .bind(&data.resource)
        .bind(&data.resource_id)
        .bind(&data.details)
        .bind(&data.ip_address)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert audit log: {e}")))?;

        let row = sqlx::query_as::<_, AuditLogRow>("SELECT * FROM audit_logs WHERE id = $1")
            .bind(&id)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to fetch audit log: {e}")))?
            .ok_or_else(|| AppError::Internal("audit log not found after insert".to_string()))?;

        Ok(AuditLog::from(row))
    }

    pub async fn list(
        pool: &PgPool,
        user_id: Option<Uuid>,
        action: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> AppResult<Vec<AuditLog>> {
        let mut query = String::from("SELECT * FROM audit_logs WHERE 1=1");
        let mut bind_values: Vec<String> = Vec::new();

        if let Some(uid) = user_id {
            query.push_str(" AND user_id = $1");
            bind_values.push(uid.to_string());
        }
        if let Some(act) = action {
            let idx = bind_values.len() + 1;
            query.push_str(&format!(" AND action = ${idx}"));
            bind_values.push(act.to_string());
        }

        query.push_str(" ORDER BY created_at DESC");

        let limit_idx = bind_values.len() + 1;
        let offset_idx = bind_values.len() + 2;
        query.push_str(&format!(" LIMIT ${limit_idx} OFFSET ${offset_idx}"));

        let mut qx = sqlx::query_as::<_, AuditLogRow>(&query);
        for val in &bind_values {
            qx = qx.bind(val);
        }
        qx = qx.bind(limit).bind(offset);

        let rows = qx
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to list audit logs: {e}")))?;

        Ok(rows.into_iter().map(AuditLog::from).collect())
    }
}
