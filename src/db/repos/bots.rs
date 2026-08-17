use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::{Bot, BotPathRule};
use crate::db::rows::bot_row::{bot_columns_serialized, BotRow, CreateBotData};
use sqlx::PgPool;
use uuid::Uuid;

/// Update payload: `Option<Option<..>>` distinguishes "leave unchanged" (outer
/// `None`) from "explicitly set to null/empty" (`Some(None)`), which the API
/// uses to lift a restriction.
#[derive(Debug, Clone, Default)]
pub struct UpdateBotData {
    pub name: Option<String>,
    pub can_upload: Option<bool>,
    pub can_download: Option<bool>,
    pub can_copy: Option<bool>,
    pub can_edit: Option<bool>,
    pub can_delete: Option<bool>,
    pub can_list: Option<bool>,
    pub path_rules: Option<Option<Vec<BotPathRule>>>,
    pub upload_limit_bytes: Option<i64>,
}

pub struct BotRepository;

impl BotRepository {
    pub async fn create(pool: &PgPool, data: CreateBotData) -> AppResult<Bot> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let path_rules = bot_columns_serialized(&data);

        sqlx::query(
            r#"INSERT INTO bots (
                   id, user_id, key_id, name,
                   can_upload, can_download, can_copy, can_edit, can_delete, can_list,
                   path_rules,
                   upload_limit_bytes, uploaded_bytes, created_at, updated_at
               ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 0, $13, $13)"#,
        )
        .bind(&id)
        .bind(data.user_id.to_string())
        .bind(data.key_id.to_string())
        .bind(&data.name)
        .bind(data.can_upload)
        .bind(data.can_download)
        .bind(data.can_copy)
        .bind(data.can_edit)
        .bind(data.can_delete)
        .bind(data.can_list)
        .bind(&path_rules)
        .bind(data.upload_limit_bytes)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to insert bot: {e}")))?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("bot not found after insert".to_string()))
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<Bot>> {
        let row = sqlx::query_as::<_, BotRow>("SELECT * FROM bots WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query bot: {e}")))?;
        Ok(row.map(Bot::from))
    }

    pub async fn find_by_key_id(pool: &PgPool, key_id: Uuid) -> AppResult<Option<Bot>> {
        let row = sqlx::query_as::<_, BotRow>("SELECT * FROM bots WHERE key_id = $1")
            .bind(key_id.to_string())
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query bot by key: {e}")))?;
        Ok(row.map(Bot::from))
    }

    pub async fn find_by_user_and_id(pool: &PgPool, user_id: Uuid, id: Uuid) -> AppResult<Option<Bot>> {
        let row = sqlx::query_as::<_, BotRow>(
            "SELECT * FROM bots WHERE id = $1 AND user_id = $2",
        )
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query bot: {e}")))?;
        Ok(row.map(Bot::from))
    }

    pub async fn list_by_user(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<Bot>> {
        let rows = sqlx::query_as::<_, BotRow>(
            "SELECT * FROM bots WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list bots: {e}")))?;
        Ok(rows.into_iter().map(Bot::from).collect())
    }

    pub async fn list_all(pool: &PgPool) -> AppResult<Vec<Bot>> {
        let rows = sqlx::query_as::<_, BotRow>(
            "SELECT * FROM bots ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list bots: {e}")))?;
        Ok(rows.into_iter().map(Bot::from).collect())
    }

    /// Update an owned bot's configuration. Returns the updated bot, or `None`
    /// when the row does not exist / does not belong to `user_id`.
    ///
    /// `Some(None)` on a list field lifts that restriction (stores NULL).
    pub async fn update(
        pool: &PgPool,
        user_id: Uuid,
        id: Uuid,
        data: UpdateBotData,
    ) -> AppResult<Option<Bot>> {
        let rules_bind = |v: Option<Option<Vec<BotPathRule>>>| -> (bool, Option<String>) {
            match v {
                None => (false, None),
                Some(rules) => (
                    true,
                    rules.map(|r| serde_json::to_string(&r).unwrap_or_else(|_| "[]".to_string())),
                ),
            }
        };

        let (rules_provided, rules) = rules_bind(data.path_rules);

        let row = sqlx::query_as::<_, BotRow>(
            r#"UPDATE bots SET
                   name = COALESCE($1, name),
                   can_upload = COALESCE($2, can_upload),
                   can_download = COALESCE($3, can_download),
                   can_copy = COALESCE($4, can_copy),
                   can_edit = COALESCE($5, can_edit),
                   can_delete = COALESCE($6, can_delete),
                   can_list = COALESCE($7, can_list),
                   path_rules = CASE WHEN $8 THEN $9 ELSE path_rules END,
                   upload_limit_bytes = COALESCE($10, upload_limit_bytes),
                   updated_at = $11
               WHERE id = $12 AND user_id = $13
               RETURNING *"#,
        )
        .bind(data.name)
        .bind(data.can_upload)
        .bind(data.can_download)
        .bind(data.can_copy)
        .bind(data.can_edit)
        .bind(data.can_delete)
        .bind(data.can_list)
        .bind(rules_provided)
        .bind(rules)
        .bind(data.upload_limit_bytes)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update bot: {e}")))?;

        Ok(row.map(Bot::from))
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM bots WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete bot: {e}")))?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Point a bot at a newly issued API key (key rotation).
    pub async fn update_key_id(pool: &PgPool, id: Uuid, key_id: Uuid) -> AppResult<Option<Bot>> {
        let row = sqlx::query_as::<_, BotRow>(
            "UPDATE bots SET key_id = $1, updated_at = $2 WHERE id = $3 RETURNING *",
        )
        .bind(key_id.to_string())
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update bot key: {e}")))?;
        Ok(row.map(Bot::from))
    }

    /// Atomically charge `bytes` against the bot's lifetime upload cap.
    /// Returns `false` when the charge would exceed the cap (0 = unlimited),
    /// leaving `uploaded_bytes` untouched.
    pub async fn charge_uploaded_bytes(
        pool: &PgPool,
        key_id: Uuid,
        bytes: i64,
    ) -> AppResult<bool> {
        if bytes <= 0 {
            return Ok(true);
        }
        let affected = sqlx::query(
            r#"UPDATE bots
               SET uploaded_bytes = uploaded_bytes + $1,
                   updated_at = $2
               WHERE key_id = $3
                 AND (upload_limit_bytes = 0
                      OR uploaded_bytes + $1 <= upload_limit_bytes)"#,
        )
        .bind(bytes)
        .bind(Utc::now().to_rfc3339())
        .bind(key_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to charge bot upload: {e}")))?
        .rows_affected();

        Ok(affected > 0)
    }
}
