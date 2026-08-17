use crate::db::is_unique_violation;
use crate::error::{AppError, AppResult};
use crate::models::UserFolder;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::rows::folder_row::{FolderRecord, FolderRow};

/// Flat row for admin bucket export: folder + username.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FolderExportRow {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub bucket_name: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub created_at: String,
}

pub struct FolderRepository;

impl FolderRepository {
    /// Create a new virtual folder.
    pub async fn create(pool: &PgPool, record: FolderRecord) -> AppResult<UserFolder> {
        let now = chrono::Utc::now().to_rfc3339();
        let id = record.id.to_string();

        sqlx::query(
            r#"INSERT INTO user_folders (id, user_id, bucket_name, parent_id, name, created_at)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
        )
        .bind(&id)
        .bind(record.user_id.to_string())
        .bind(&record.bucket_name)
        .bind(record.parent_id.map(|p| p.to_string()))
        .bind(&record.name)
        .bind(&now)
        .execute(pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict(format!("folder '{}' already exists in this location", record.name))
            } else {
                AppError::Internal(format!("failed to create folder: {e}"))
            }
        })?;

        Self::find_by_id(pool, Uuid::parse_str(&id).unwrap())
            .await?
            .ok_or_else(|| AppError::Internal("folder not found after insert".to_string()))
    }

    /// Find a folder by ID, ensuring it belongs to the given user.
    pub async fn find_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<UserFolder>> {
        let row = sqlx::query_as::<_, FolderRow>(
            "SELECT * FROM user_folders WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query folder: {e}")))?;

        Ok(row.map(UserFolder::from))
    }

    /// Find a folder by user + id (ownership check).
    pub async fn find_by_user_and_id(
        pool: &PgPool,
        user_id: Uuid,
        folder_id: Uuid,
    ) -> AppResult<Option<UserFolder>> {
        let row = sqlx::query_as::<_, FolderRow>(
            "SELECT * FROM user_folders WHERE user_id = $1 AND id = $2",
        )
        .bind(user_id.to_string())
        .bind(folder_id.to_string())
        .fetch_optional(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to query folder: {e}")))?;

        Ok(row.map(UserFolder::from))
    }

    /// List direct child folders of a parent folder in a bucket.
    /// parent_id = None means root level.
    pub async fn list_children(
        pool: &PgPool,
        user_id: Uuid,
        bucket_name: &str,
        parent_id: Option<Uuid>,
    ) -> AppResult<Vec<UserFolder>> {
        let rows: Vec<FolderRow> = match parent_id {
            Some(pid) => {
                sqlx::query_as(
                    "SELECT * FROM user_folders WHERE user_id = $1 AND bucket_name = $2 AND parent_id = $3 ORDER BY name ASC",
                )
                .bind(user_id.to_string())
                .bind(bucket_name)
                .bind(pid.to_string())
                .fetch_all(pool)
                .await
            }
            None => {
                sqlx::query_as(
                    "SELECT * FROM user_folders WHERE user_id = $1 AND bucket_name = $2 AND parent_id IS NULL ORDER BY name ASC",
                )
                .bind(user_id.to_string())
                .bind(bucket_name)
                .fetch_all(pool)
                .await
            }
        }
        .map_err(|e| AppError::Internal(format!("failed to list folders: {e}")))?;

        Ok(rows.into_iter().map(UserFolder::from).collect())
    }

    /// Rename a folder.
    pub async fn update_name(pool: &PgPool, id: Uuid, new_name: &str) -> AppResult<bool> {
        let affected = sqlx::query("UPDATE user_folders SET name = $1 WHERE id = $2")
            .bind(new_name)
            .bind(id.to_string())
            .execute(pool)
            .await
            .map_err(|e| {
                if is_unique_violation(&e) {
                    AppError::Conflict(format!("folder name '{}' already exists in this location", new_name))
                } else {
                    AppError::Internal(format!("failed to rename folder: {e}"))
                }
            })?
            .rows_affected();
        Ok(affected > 0)
    }

    /// Delete a folder AND all its contents recursively (subfolders + files are deleted).
    pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<bool> {
        // 1. Collect all descendant folder IDs (including the folder itself) via recursive CTE
        let all_folder_ids: Vec<(String,)> = sqlx::query_as(
            r#"WITH RECURSIVE tree(id) AS (
                   SELECT id FROM user_folders WHERE id = $1
                   UNION ALL
                   SELECT uf.id FROM user_folders uf
                   INNER JOIN tree t ON uf.parent_id = t.id
               )
               SELECT id FROM tree"#,
        )
        .bind(id.to_string())
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to collect folder tree: {e}")))?;

        if all_folder_ids.is_empty() {
            return Ok(false);
        }

        // 2. Soft-delete ALL files in those folders (set deleted_at = now), then
        // hard-delete the folders themselves. Both run in one transaction so a
        // mid-way failure cannot leave files soft-deleted but folders kept (or
        // the reverse).
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| AppError::Internal(format!("failed to begin folder delete: {e}")))?;

        let now = chrono::Utc::now().to_rfc3339();
        let folder_id_strs: Vec<String> = all_folder_ids.iter().map(|(id,)| id.clone()).collect();

        // The file soft-delete binds `now` as $1, so the folder ids must occupy
        // $2..$N (a placeholder list starting at $1 would compare `folder_id`
        // against the timestamp and silently drop the last folder id).
        let file_placeholders: Vec<String> = folder_id_strs
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 2))
            .collect();
        let file_sql = format!(
            "UPDATE user_files SET deleted_at = $1 WHERE folder_id IN ({}) AND deleted_at IS NULL",
            file_placeholders.join(", ")
        );
        let mut query = sqlx::query(&file_sql).bind(&now);
        for fid in &folder_id_strs {
            query = query.bind(fid);
        }
        let _ = query
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to soft-delete files in folder tree: {e}")))?;

        // 3. Hard-delete all folders in the tree (deepest first via the CTE).
        // No `now` bound here, so the ids start at $1.
        let folder_placeholders: Vec<String> = folder_id_strs
            .iter()
            .enumerate()
            .map(|(i, _)| format!("${}", i + 1))
            .collect();
        let del_query = format!(
            "DELETE FROM user_folders WHERE id IN ({})",
            folder_placeholders.join(", ")
        );
        let mut query = sqlx::query(&del_query);
        for fid in &folder_id_strs {
            query = query.bind(fid);
        }
        let affected = query
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::Internal(format!("failed to delete folder tree: {e}")))?
            .rows_affected();

        tx.commit()
            .await
            .map_err(|e| AppError::Internal(format!("failed to commit folder delete: {e}")))?;

        Ok(affected > 0)
    }

    /// List all folders in a bucket for a user (flat list for building client-side tree).
    pub async fn list_all_for_bucket(
        pool: &PgPool,
        user_id: Uuid,
        bucket_name: &str,
    ) -> AppResult<Vec<UserFolder>> {
        let rows: Vec<FolderRow> = sqlx::query_as(
            "SELECT * FROM user_folders WHERE user_id = $1 AND bucket_name = $2 ORDER BY name ASC",
        )
        .bind(user_id.to_string())
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list all folders: {e}")))?;

        Ok(rows.into_iter().map(UserFolder::from).collect())
    }

    pub async fn list_all_for_bucket_admin(
        pool: &PgPool,
        bucket_name: &str,
    ) -> AppResult<Vec<FolderExportRow>> {
        let rows = sqlx::query_as::<_, FolderExportRow>(
            r#"SELECT uf.id, uf.user_id, u.username, uf.bucket_name,
                      uf.parent_id, uf.name, uf.created_at
               FROM user_folders uf
               JOIN users u ON uf.user_id = u.id
               WHERE uf.bucket_name = $1
               ORDER BY u.username, uf.name"#,
        )
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list all bucket folders admin: {e}")))?;

        Ok(rows)
    }

    /// Count files directly in a folder.
    pub async fn count_files(pool: &PgPool, folder_id: Uuid) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_files WHERE folder_id = $1 AND deleted_at IS NULL",
        )
        .bind(folder_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count folder files: {e}")))?;

        Ok(result.0)
    }

    /// Count subfolders directly in a folder.
    pub async fn count_subfolders(pool: &PgPool, folder_id: Uuid) -> AppResult<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM user_folders WHERE parent_id = $1",
        )
        .bind(folder_id.to_string())
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to count subfolders: {e}")))?;

        Ok(result.0)
    }

    /// Build the full path for a folder by traversing the parent chain.
    pub async fn get_path(pool: &PgPool, folder_id: Uuid) -> AppResult<Vec<(Uuid, String)>> {
        let mut path = Vec::new();
        let mut current_id = Some(folder_id);

        while let Some(id) = current_id {
            let folder = Self::find_by_id(pool, id).await?;
            match folder {
                Some(f) => {
                    path.push((f.id, f.name));
                    current_id = f.parent_id;
                }
                None => break,
            }
        }

        path.reverse();
        Ok(path)
    }

    /// Resolve a slash-separated path (e.g. "Documents/Work") into a folder ID,
    /// walking segment by segment from root. Returns the final folder ID and its
    /// full breadcrumb path. All queries are scoped to user_id + bucket_name.
    /// Each segment is matched exactly (parameterized query, no injection risk).
    pub async fn resolve_path(
        pool: &PgPool,
        user_id: Uuid,
        bucket_name: &str,
        path: &str,
    ) -> AppResult<Option<(Uuid, Vec<(Uuid, String)>)>> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return Ok(None);
        }

        // Guard: max 32 segments to prevent abuse
        if segments.len() > 32 {
            return Ok(None);
        }

        let mut current_parent_id: Option<Uuid> = None;
        let mut breadcrumb: Vec<(Uuid, String)> = Vec::new();

        for segment in &segments {
            // Sanitize: segment must not be empty, must not contain null bytes,
            // and must be reasonable length (255 chars max for a folder name)
            if segment.len() > 255 || segment.contains('\0') {
                return Ok(None);
            }

            let row: Option<FolderRow> = sqlx::query_as(
                "SELECT * FROM user_folders
                 WHERE user_id = $1 AND bucket_name = $2 AND name = $3
                 AND parent_id IS NOT DISTINCT FROM $4
                 LIMIT 1",
            )
            .bind(user_id.to_string())
            .bind(bucket_name)
            .bind(*segment)
            .bind(current_parent_id.map(|id| id.to_string()))
            .fetch_optional(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to resolve folder path: {e}")))?;

            match row {
                Some(r) => {
                    let folder = UserFolder::from(r);
                    current_parent_id = Some(folder.id);
                    breadcrumb.push((folder.id, folder.name));
                }
                None => return Ok(None), // segment not found — path invalid
            }
        }

        let final_id = current_parent_id
            .ok_or_else(|| AppError::Internal("resolve_path: no final id".to_string()))?;

        // Build full breadcrumb: Root + each segment
        let mut full_path = vec![(Uuid::nil(), "Root".to_string())];
        full_path.extend(breadcrumb);

        Ok(Some((final_id, full_path)))
    }

    /// Move a folder to a new parent (or root if None).
    /// Prevents moving a folder into its own descendants (cycle detection).
    /// Also prevents moving if a folder with the same name already exists at the target location.
    pub async fn move_folder(
        pool: &PgPool,
        folder_id: Uuid,
        new_parent_id: Option<Uuid>,
    ) -> AppResult<bool> {
        // Get the folder being moved
        let folder = Self::find_by_id(pool, folder_id).await?;
        let folder = match folder {
            Some(f) => f,
            None => return Ok(false),
        };

        // Can't move into itself
        if Some(folder_id) == new_parent_id {
            return Err(AppError::BadRequest("cannot move folder into itself".into()));
        }

        // Cycle detection: if new_parent_id is set, walk up from new_parent_id
        // and make sure we never hit folder_id
        if let Some(target_id) = new_parent_id {
            let mut cursor = Some(target_id);
            while let Some(current) = cursor {
                if current == folder_id {
                    return Err(AppError::BadRequest(
                        "cannot move folder into one of its own subfolders".into(),
                    ));
                }
                let parent_folder = Self::find_by_id(pool, current).await?;
                cursor = parent_folder.and_then(|f| f.parent_id);
            }
        }

        // Check name conflict at target location
        let conflict = match new_parent_id {
            Some(pid) => {
                sqlx::query_as::<_, FolderRow>(
                    "SELECT * FROM user_folders WHERE user_id = $1 AND bucket_name = $2 AND parent_id = $3 AND name = $4 AND id != $5 LIMIT 1",
                )
                .bind(folder.user_id.to_string())
                .bind(&folder.bucket_name)
                .bind(pid.to_string())
                .bind(&folder.name)
                .bind(folder_id.to_string())
                .fetch_optional(pool)
                .await
            }
            None => {
                sqlx::query_as::<_, FolderRow>(
                    "SELECT * FROM user_folders WHERE user_id = $1 AND bucket_name = $2 AND parent_id IS NULL AND name = $3 AND id != $4 LIMIT 1",
                )
                .bind(folder.user_id.to_string())
                .bind(&folder.bucket_name)
                .bind(&folder.name)
                .bind(folder_id.to_string())
                .fetch_optional(pool)
                .await
            }
        }
        .map_err(|e| AppError::Internal(format!("failed to check folder name conflict: {e}")))?;

        if conflict.is_some() {
            return Err(AppError::Conflict(format!(
                "a folder named '{}' already exists at the target location",
                folder.name
            )));
        }

        // Perform the move
        let affected = sqlx::query("UPDATE user_folders SET parent_id = $1 WHERE id = $2")
            .bind(new_parent_id.map(|p| p.to_string()))
            .bind(folder_id.to_string())
            .execute(pool)
            .await
            .map_err(|e| AppError::Internal(format!("failed to move folder: {e}")))?
            .rows_affected();

        Ok(affected > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folder_record_new() {
        let user_id = Uuid::new_v4();
        let record = FolderRecord::new(user_id, "my-bucket".into(), "Documents".into(), None);
        assert!(!record.id.is_nil());
        assert_eq!(record.user_id, user_id);
        assert_eq!(record.name, "Documents");
        assert!(record.parent_id.is_none());
    }

    #[test]
    fn test_folder_record_nested() {
        let user_id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let record = FolderRecord::new(user_id, "my-bucket".into(), "Work".into(), Some(parent_id));
        assert_eq!(record.parent_id, Some(parent_id));
    }
}
