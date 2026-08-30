use chrono::Utc;
use crate::error::{AppError, AppResult};
use crate::models::share::{SharedItem, SharedItemType};
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::share_row::{CreateSharedItemData, SharedItemRow};

/// Flat row for JOIN queries that combine shared_items + users columns.
#[derive(Debug, Clone, sqlx::FromRow)]
struct SharedItemWithUserRow {
    pub id: String,
    pub shared_by_user_id: String,
    pub shared_with_user_id: String,
    pub item_type: String,
    pub item_id: String,
    pub created_at: String,
    pub username: String,
    pub email: String,
}

pub struct ShareRepository;

impl ShareRepository {
    /// Create a single share record.
    ///
    /// Returns `true` when a new row was inserted and `false` when the share
    /// already existed (the insert is a no-op via `ON CONFLICT DO NOTHING`).
    /// Callers use the boolean so failed/deduped shares are not reported as
    /// successful new share.
    pub async fn create(pool: &PgPool, data: CreateSharedItemData) -> AppResult<bool> {
        let now = Utc::now().to_rfc3339();
        let id = data.id.to_string();

        let res = sqlx::query(
            r#"INSERT INTO shared_items (id, shared_by_user_id, shared_with_user_id, item_type, item_id, created_at)
               VALUES ($1, $2, $3, $4, $5, $6)
               ON CONFLICT (shared_by_user_id, shared_with_user_id, item_type, item_id) DO NOTHING"#,
        )
        .bind(&id)
        .bind(data.shared_by_user_id.to_string())
        .bind(data.shared_with_user_id.to_string())
        .bind(data.item_type.as_str())
        .bind(data.item_id.to_string())
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create share: {e}")))?;

        Ok(res.rows_affected() > 0)
    }

    /// Find a share by ID.
    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<SharedItem>> {
        let row = sqlx::query_as::<_, SharedItemRow>(
            "SELECT * FROM shared_items WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query share: {e}")))?;

        Ok(row.map(SharedItem::from))
    }

    /// Find a specific share (sharer + recipient + item).
    pub async fn find_specific(
        pool: &PgPool,
        shared_by: Uuid,
        shared_with: Uuid,
        item_type: SharedItemType,
        item_id: Uuid,
    ) -> AppResult<Option<SharedItem>> {
        let row = sqlx::query_as::<_, SharedItemRow>(
            "SELECT * FROM shared_items WHERE shared_by_user_id = $1 AND shared_with_user_id = $2 AND item_type = $3 AND item_id = $4",
        )
        .bind(shared_by.to_string())
        .bind(shared_with.to_string())
        .bind(item_type.as_str())
        .bind(item_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query share: {e}")))?;

        Ok(row.map(SharedItem::from))
    }

    /// List all items shared with a user, along with sharer info.
    /// Returns (share, sharer_username, sharer_email).
    pub async fn list_shared_with_user(
        pool: &PgPool,
        user_id: Uuid,
    ) -> AppResult<Vec<(SharedItem, String, String)>> {
        let rows: Vec<SharedItemWithUserRow> = sqlx::query_as(
            r#"SELECT si.id, si.shared_by_user_id, si.shared_with_user_id, si.item_type, si.item_id, si.created_at,
                      u.username, u.email
               FROM shared_items si
               JOIN users u ON si.shared_by_user_id = u.id
               WHERE si.shared_with_user_id = $1
               ORDER BY si.created_at DESC"#,
        )
        .bind(user_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list shared items: {e}")))?;

        Ok(rows.into_iter().map(|row| {
            let share = SharedItem::from(SharedItemRow {
                id: row.id,
                shared_by_user_id: row.shared_by_user_id,
                shared_with_user_id: row.shared_with_user_id,
                item_type: row.item_type,
                item_id: row.item_id,
                created_at: row.created_at,
            });
            (share, row.username, row.email)
        }).collect())
    }

    /// List all shares created by a user for a specific item (to know who it's shared with).
    pub async fn list_shares_of_item(
        pool: &PgPool,
        item_type: SharedItemType,
        item_id: Uuid,
    ) -> AppResult<Vec<(SharedItem, String, String)>> {
        let rows: Vec<SharedItemWithUserRow> = sqlx::query_as(
            r#"SELECT si.id, si.shared_by_user_id, si.shared_with_user_id, si.item_type, si.item_id, si.created_at,
                      u.username, u.email
               FROM shared_items si
               JOIN users u ON si.shared_with_user_id = u.id
               WHERE si.item_type = $1 AND si.item_id = $2
               ORDER BY si.created_at DESC"#,
        )
        .bind(item_type.as_str())
        .bind(item_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list shares of item: {e}")))?;

        Ok(rows.into_iter().map(|row| {
            let share = SharedItem::from(SharedItemRow {
                id: row.id,
                shared_by_user_id: row.shared_by_user_id,
                shared_with_user_id: row.shared_with_user_id,
                item_type: row.item_type,
                item_id: row.item_id,
                created_at: row.created_at,
            });
            (share, row.username, row.email)
        }).collect())
    }

    /// Delete a share. Only the sharer can delete their own share.
    ///
    /// When the share is a *folder*, every descendant folder and file is revoked
    /// from the same recipient too. Sharing a folder materialises independent
    /// rows for the root folder, each subfolder and each file; revoking only the
    /// top-level row would otherwise leave the recipient with working access to
    /// every descendant (see the folder-sharing creation in
    /// [`Self::create_folder_shares`]). Issuing a single `DELETE` that cascades
    /// over the whole folder tree keeps revocation atomic and correct.
    pub async fn delete_share(
        pool: &PgPool,
        share_id: Uuid,
        shared_by_user_id: Uuid,
    ) -> AppResult<bool> {
        // Fetch the share first so we can tell folder shares apart from file
        // shares and read the recipient (needed for the cascade).
        let share = Self::find_by_id(pool, share_id)
            .await?
            .filter(|s| s.shared_by_user_id == shared_by_user_id);

        let Some(share) = share else {
            // Either it does not exist or it is not owned by this sharer.
            return Ok(false);
        };

        if share.item_type == SharedItemType::Folder {
            Self::delete_folder_tree_shares(
                pool,
                shared_by_user_id,
                share.shared_with_user_id,
                share.item_id,
            )
            .await?;
        } else {
            // Single placeholder row delete; there is only one row per
            // (sharer, recipient, file).
            let _ = sqlx::query("DELETE FROM shared_items WHERE id = $1")
                .bind(share_id.to_string())
                .execute(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to delete share: {e}")))?;
        }

        Ok(true)
    }

    /// Delete a folder share and every descendant folder/file share that a
    /// specific sharer granted to a specific recipient underneath `folder_id`.
    ///
    /// This is the mirror of [`Self::create_folder_shares`]: both walk the same
    /// folder subtree (recursive CTE) so the set of revoked rows always matches
    /// the set of rows that sharing created.
    async fn delete_folder_tree_shares(
        pool: &PgPool,
        shared_by_user_id: Uuid,
        shared_with_user_id: Uuid,
        folder_id: Uuid,
    ) -> AppResult<()> {
        // Collect every folder in the subtree (root + descendants).
        let folder_ids: Vec<(String,)> = sqlx::query_as::<_, (String,)>(
            r#"WITH RECURSIVE tree(id) AS (
                   SELECT id FROM user_folders WHERE id = $1
                   UNION ALL
                   SELECT uf.id FROM user_folders uf
                   INNER JOIN tree t ON uf.parent_id = t.id
               )
               SELECT id FROM tree"#,
        )
        .bind(folder_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to collect folder tree for unsharing: {e}")))?;

        let folder_id_list: Vec<String> = folder_ids.iter().map(|(id,)| id.clone()).collect();

        // Delete the folder share for (sharer, recipient) across the whole tree.
        for (fid,) in &folder_ids {
            sqlx::query(
                "DELETE FROM shared_items \
                 WHERE item_type = 'folder' AND item_id = $1 \
                   AND shared_by_user_id = $2 AND shared_with_user_id = $3",
            )
            .bind(fid)
            .bind(shared_by_user_id.to_string())
            .bind(shared_with_user_id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete folder share: {e}")))?;
        }

        // Delete the file share for (sharer, recipient) for every file inside
        // the subtree.
        if !folder_id_list.is_empty() {
            let placeholders: Vec<String> = folder_id_list
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect();
            let sql = format!(
                "DELETE FROM shared_items \
                 WHERE item_id::text IN ( \
                     SELECT id::text FROM user_files \
                     WHERE folder_id::text IN ({}) \
                       AND deleted_at IS NULL \
                 ) \
                   AND item_type = 'file' \
                   AND shared_by_user_id = ${} AND shared_with_user_id = ${}",
                placeholders.join(", "),
                folder_id_list.len() + 1,
                folder_id_list.len() + 2,
            );
            let mut query = sqlx::query(&sql);
            for fid in &folder_id_list {
                query = query.bind(fid);
            }
            query
                .bind(shared_by_user_id.to_string())
                .bind(shared_with_user_id.to_string())
                .execute(pool)
                .await
                .map_err(|e| AppError::Internal(format!("failed to delete file shares: {e}")))?;
        }

        Ok(())
    }

    /// Delete all shares for a given item (used when an item is deleted).
    pub async fn delete_by_item(pool: &PgPool, item_type: SharedItemType, item_id: Uuid) -> AppResult<u64> {
        let affected = sqlx::query(
            "DELETE FROM shared_items WHERE item_type = $1 AND item_id = $2",
        )
        .bind(item_type.as_str())
        .bind(item_id.to_string())
        .execute(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to delete shares for item: {e}")))?
        .rows_affected();

        Ok(affected)
    }

    /// Delete all shares for items in a list of folder IDs (used when a folder tree is deleted).
    pub async fn delete_by_folder_ids(pool: &PgPool, folder_ids: &[String]) -> AppResult<u64> {
        if folder_ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = folder_ids.iter().enumerate().map(|(i, _)| format!("${}", i + 1)).collect();
        let sql = format!(
            "DELETE FROM shared_items WHERE item_type = 'folder' AND item_id::text IN ({})",
            placeholders.join(", ")
        );
        let mut query = sqlx::query(&sql);
        for id in folder_ids {
            query = query.bind(id);
        }
        let affected = query
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete folder shares: {e}")))?
            .rows_affected();
        Ok(affected)
    }

    /// Delete all shares for user_file IDs (used when files in a folder tree are deleted).
    pub async fn delete_by_user_file_ids(pool: &PgPool, user_file_ids: &[String]) -> AppResult<u64> {
        if user_file_ids.is_empty() {
            return Ok(0);
        }
        let placeholders: Vec<String> = user_file_ids.iter().enumerate().map(|(i, _)| format!("${}", i + 1)).collect();
        let sql = format!(
            "DELETE FROM shared_items WHERE item_type = 'file' AND item_id::text IN ({})",
            placeholders.join(", ")
        );
        let mut query = sqlx::query(&sql);
        for id in user_file_ids {
            query = query.bind(id);
        }
        let affected = query
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete file shares: {e}")))?
            .rows_affected();
        Ok(affected)
    }

    /// Check if an item is shared with a specific user.
    pub async fn is_shared_with(
        pool: &PgPool,
        item_type: SharedItemType,
        item_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<bool> {
        let (exists,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM shared_items WHERE item_type = $1 AND item_id = $2 AND shared_with_user_id = $3)",
        )
        .bind(item_type.as_str())
        .bind(item_id.to_string())
        .bind(user_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to check share: {e}")))?;

        Ok(exists)
    }

    /// Recursively create shares for a folder and all its descendants (subfolders + files).
    pub async fn create_folder_shares(
        pool: &PgPool,
        shared_by_user_id: Uuid,
        shared_with_user_id: Uuid,
        folder_id: Uuid,
    ) -> AppResult<usize> {
        let mut count = 0;

        // Share the folder itself
        let data = CreateSharedItemData::new(
            shared_by_user_id,
            shared_with_user_id,
            SharedItemType::Folder,
            folder_id,
        );
        if let Ok(true) = Self::create(pool, data).await {
            count += 1;
        }

        // Find all descendant folder IDs via recursive CTE
        let descendant_ids: Vec<(String,)> = sqlx::query_as(
            r#"WITH RECURSIVE tree(id) AS (
                   SELECT id FROM user_folders WHERE id = $1
                   UNION ALL
                   SELECT uf.id FROM user_folders uf
                   INNER JOIN tree t ON uf.parent_id = t.id
               )
               SELECT id FROM tree"#,
        )
        .bind(folder_id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to collect folder tree for sharing: {e}")))?;

        // Share all descendant folders (excluding the root folder already shared)
        for (desc_id,) in &descendant_ids[1..] {
            let desc_uuid = Uuid::parse_str(desc_id).expect("invalid uuid");
            let data = CreateSharedItemData::new(
                shared_by_user_id,
                shared_with_user_id,
                SharedItemType::Folder,
                desc_uuid,
            );
            if let Ok(true) = Self::create(pool, data).await {
                count += 1;
            }
        }

        // Share all files in the entire folder tree
        let all_folder_ids: Vec<String> = descendant_ids.iter().map(|(id,)| id.clone()).collect();
        let placeholders: Vec<String> = all_folder_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 2))
            .collect();
        let file_sql = format!(
            "SELECT id FROM user_files WHERE folder_id IN ({}) AND deleted_at IS NULL",
            placeholders.join(", ")
        );
        let mut query = sqlx::query_as::<_, (String,)>(&file_sql)
            .bind(folder_id.to_string());
        for fid in &all_folder_ids {
            query = query.bind(fid);
        }
        let file_rows: Vec<(String,)> = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to list files for sharing: {e}")))?;

        for (file_id,) in &file_rows {
            let file_uuid = Uuid::parse_str(file_id).expect("invalid uuid");
            let data = CreateSharedItemData::new(
                shared_by_user_id,
                shared_with_user_id,
                SharedItemType::File,
                file_uuid,
            );
            if let Ok(true) = Self::create(pool, data).await {
                count += 1;
            }
        }

        Ok(count)
    }

    /// Find a user by email (for sharing by email).
    pub async fn find_user_by_email(pool: &PgPool, email: &str) -> AppResult<Option<(Uuid, String, String)>> {
        let row: Option<(String, String, String)> = sqlx::query_as(
            "SELECT id, username, email FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find user by email: {e}")))?;

        Ok(row.map(|(id, username, email)| {
            (Uuid::parse_str(&id).expect("invalid uuid"), username, email)
        }))
    }

    /// Find a user by email (returns just the UUID).
    pub async fn find_user_id_by_email(pool: &PgPool, email: &str) -> AppResult<Option<Uuid>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find user by email: {e}")))?;

        Ok(row.map(|(id,)| Uuid::parse_str(&id).expect("invalid uuid")))
    }

    /// Check which user_file IDs are shared (returns the set of shared file IDs).
    pub async fn get_shared_file_ids(pool: &PgPool, user_file_ids: &[String]) -> AppResult<Vec<String>> {
        if user_file_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = user_file_ids.iter().enumerate().map(|(i, _)| format!("${}", i + 1)).collect();
        let sql = format!(
            "SELECT DISTINCT item_id::text FROM shared_items WHERE item_type = 'file' AND item_id::text IN ({})",
            placeholders.join(", ")
        );
        let mut query = sqlx::query_as::<_, (String,)>(&sql);
        for id in user_file_ids {
            query = query.bind(id);
        }
        let rows: Vec<(String,)> = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to get shared file ids: {e}")))?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Check which folder IDs are shared (returns the set of shared folder IDs).
    pub async fn get_shared_folder_ids(pool: &PgPool, folder_ids: &[String]) -> AppResult<Vec<String>> {
        if folder_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<String> = folder_ids.iter().enumerate().map(|(i, _)| format!("${}", i + 1)).collect();
        let sql = format!(
            "SELECT DISTINCT item_id::text FROM shared_items WHERE item_type = 'folder' AND item_id::text IN ({})",
            placeholders.join(", ")
        );
        let mut query = sqlx::query_as::<_, (String,)>(&sql);
        for id in folder_ids {
            query = query.bind(id);
        }
        let rows: Vec<(String,)> = query
            .fetch_all(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to get shared folder ids: {e}")))?;

        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Find a folder share for a specific user, returning the share + sharer info.
    pub async fn find_folder_share_for_user(
        pool: &PgPool,
        folder_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Option<(SharedItem, String, String)>> {
        let row: Option<SharedItemWithUserRow> = sqlx::query_as(
            r#"SELECT si.id, si.shared_by_user_id, si.shared_with_user_id, si.item_type, si.item_id, si.created_at,
                      u.username, u.email
               FROM shared_items si
               JOIN users u ON si.shared_by_user_id = u.id
               WHERE si.item_type = 'folder' AND si.item_id = $1 AND si.shared_with_user_id = $2
               LIMIT 1"#,
        )
        .bind(folder_id.to_string())
        .bind(user_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to find folder share for user: {e}")))?;

        Ok(row.map(|row| {
            let share = SharedItem::from(SharedItemRow {
                id: row.id,
                shared_by_user_id: row.shared_by_user_id,
                shared_with_user_id: row.shared_with_user_id,
                item_type: row.item_type,
                item_id: row.item_id,
                created_at: row.created_at,
            });
            (share, row.username, row.email)
        }))
    }
}
