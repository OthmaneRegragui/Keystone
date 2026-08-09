use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::{User, UserRole};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::user_row::{role_to_string, CreateUserData, UserRow};

/// Postgres advisory-lock key (arbitrary constant) that serializes the
/// "first registered user becomes admin" bootstrap decision.
const BOOTSTRAP_LOCK_KEY: i64 = 0x4B53_7472_6170_0001;

pub struct UserRepository;

impl UserRepository {
    pub async fn create(pool: &PgPool, data: CreateUserData) -> AppResult<User> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let role = role_to_string(data.role);

        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, storage_quota, storage_used, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $7)"#,
        )
        .bind(&id)
        .bind(&data.username)
        .bind(&data.email)
        .bind(&data.password_hash)
        .bind(&role)
        .bind(data.storage_quota)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| {
            if crate::db::is_unique_violation(&e) {
                AppError::Conflict(format!("user with username '{}' or email '{}' already exists", data.username, data.email))
            } else {
                AppError::Internal(format!("failed to insert user: {e}"))
            }
        })?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("user not found after insert".to_string()))
    }

    /// Create a user from the public registration endpoint. The caller passes
    /// the regular role; if the users table is empty, the new user is promoted
    /// to `Admin` (first-user bootstrap).
    ///
    /// The count-check + insert run inside a transaction guarded by a Postgres
    /// advisory lock, so two concurrent registrations on a fresh install
    /// cannot both observe `COUNT(*) == 0` and both become admin (TOCTOU).
    pub async fn create_with_bootstrap_role(
        pool: &PgPool,
        mut data: CreateUserData,
    ) -> AppResult<User> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin transaction: {e}")))?;

        // Serialize concurrent registrations until this transaction ends.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(BOOTSTRAP_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to acquire registration lock: {e}")))?;

        let (user_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to count users: {e}")))?;

        if user_count == 0 {
            data.role = UserRole::Admin;
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let role = role_to_string(data.role);

        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, storage_quota, storage_used, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, 0, $7, $7)"#,
        )
        .bind(&id)
        .bind(&data.username)
        .bind(&data.email)
        .bind(&data.password_hash)
        .bind(&role)
        .bind(data.storage_quota)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if crate::db::is_unique_violation(&e) {
                AppError::Conflict(format!("user with username '{}' or email '{}' already exists", data.username, data.email))
            } else {
                AppError::Internal(format!("failed to insert user: {e}"))
            }
        })?;

        tx.commit()
            .await
            .map_err(|e| AppError::Internal(format!("failed to commit user creation: {e}")))?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("user not found after insert".to_string()))
    }

    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE id = $1")
            .bind(id.to_string())
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query user: {e}")))?;

        Ok(row.map(User::from))
    }

    pub async fn find_by_email(pool: &PgPool, email: &str) -> AppResult<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query user by email: {e}")))?;

        Ok(row.map(User::from))
    }

    pub async fn find_by_username(pool: &PgPool, username: &str) -> AppResult<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>("SELECT * FROM users WHERE username = $1")
            .bind(username)
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to query user by username: {e}")))?;

        Ok(row.map(User::from))
    }

    pub async fn list(pool: &PgPool, offset: i64, limit: i64) -> AppResult<Vec<User>> {
        let rows = sqlx::query_as::<_, UserRow>(
            "SELECT * FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list users: {e}")))?;

        Ok(rows.into_iter().map(User::from).collect())
    }

    pub async fn count(pool: &PgPool) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to count users: {e}")))?;

        Ok(result.0)
    }

    pub async fn update_last_login(pool: &PgPool, id: Uuid) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE users SET last_login_at = $1, updated_at = $1 WHERE id = $2",
        )
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update last login: {e}")))?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("user {id} not found")));
        }
        Ok(())
    }

    pub async fn update_storage_used(pool: &PgPool, id: Uuid, bytes: i64) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE users SET storage_used = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(bytes)
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update storage used: {e}")))?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("user {id} not found")));
        }
        Ok(())
    }

    /// Atomically charge `delta` bytes against the user's storage quota.
    /// Returns `Ok(true)` if charged, `Ok(false)` if it would exceed the quota.
    ///
    /// The UPDATE is conditional, so concurrent requests cannot each observe a
    /// stale `storage_used` (the previous absolute-SET pattern lost updates and
    /// allowed the quota to be silently exceeded under concurrency).
    pub async fn charge_storage(pool: &PgPool, id: Uuid, delta: i64) -> AppResult<bool> {
        if delta < 0 {
            return Err(AppError::Internal(
                "charge_storage requires a non-negative delta".into(),
            ));
        }
        let affected = sqlx::query(
            "UPDATE users SET storage_used = storage_used + $1, updated_at = $2 \
             WHERE id = $3 AND storage_used + $1 <= storage_quota",
        )
        .bind(delta)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to charge storage: {e}")))?
        .rows_affected();

        Ok(affected > 0)
    }

    /// Release previously charged storage (compensation when a subsequent step
    /// fails after a successful `charge_storage`). Never clamps below zero and
    /// never fails.
    pub async fn release_storage(pool: &PgPool, id: Uuid, delta: i64) -> AppResult<()> {
        if delta <= 0 {
            return Ok(());
        }
        sqlx::query(
            "UPDATE users SET storage_used = GREATEST(storage_used - $1, 0), updated_at = $2 \
             WHERE id = $3",
        )
        .bind(delta)
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to release storage: {e}")))?;

        Ok(())
    }

    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        let affected = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete user: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }

    pub async fn update_user(
        pool: &PgPool,
        id: Uuid,
        email: Option<&str>,
        role: Option<&str>,
        password_hash: Option<&str>,
    ) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let current = Self::find_by_id(pool, id).await?;
        if current.is_none() {
            return Err(AppError::NotFound(format!("user {id} not found")));
        }
        let current = current.unwrap();

        let new_email = email.unwrap_or(&current.email);
        let default_role = current.role.to_string();
        let new_role = role.unwrap_or(&default_role);
        let new_hash = password_hash.unwrap_or(&current.password_hash);

        let affected = sqlx::query(
            "UPDATE users SET email = $1, role = $2, password_hash = $3, updated_at = $4 WHERE id = $5",
        )
        .bind(new_email)
        .bind(new_role)
        .bind(new_hash)
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update user: {e}")))?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("user {id} not found")));
        }
        Ok(())
    }

    pub async fn update_password_hash(pool: &PgPool, id: Uuid, password_hash: &str) -> AppResult<()> {
        let affected = sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
            .bind(password_hash)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to update password: {e}")))?
            .rows_affected();
        if affected == 0 {
            return Err(AppError::NotFound("user not found".into()));
        }
        Ok(())
    }

    pub async fn update_storage_quota(pool: &PgPool, id: Uuid, quota: i64) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        let affected = sqlx::query(
            "UPDATE users SET storage_quota = $1, updated_at = $2 WHERE id = $3",
        )
        .bind(quota)
        .bind(&now)
        .bind(id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to update storage quota: {e}")))?
        .rows_affected();

        if affected == 0 {
            return Err(AppError::NotFound(format!("user {id} not found")));
        }
        Ok(())
    }
}
