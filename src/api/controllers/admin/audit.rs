use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use chrono::Utc;
use serde::Serialize;
use sqlx::{PgPool, Row};
use walkdir::WalkDir;

use crate::api::extractors::AuthUser;
use crate::error::{AppError, AppResult};
use crate::storage::backend::StorageBackend;
use crate::AppState;

/// Maximum number of storage objects the physical (on-disk) checks will
/// inspect per run, to keep the audit cheap on large systems.
const PHYSICAL_CHECK_LIMIT: i64 = 5000;

#[derive(Debug, Default, Serialize, sqlx::FromRow)]
pub struct AuditTotals {
    pub users: i64,
    pub buckets: i64,
    pub files: i64,
    pub storage_objects: i64,
    pub links: i64,
}

#[derive(Debug, Serialize)]
pub struct AuditIssue {
    pub category: &'static str,
    pub severity: &'static str,
    pub message: String,
    pub count: i64,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub generated_at: String,
    pub totals: AuditTotals,
    pub issues: Vec<AuditIssue>,
    pub passed: bool,
}

fn issue(category: &'static str, severity: &'static str, message: String, count: i64) -> AuditIssue {
    AuditIssue {
        category,
        severity,
        message,
        count,
    }
}

/// Run a `SELECT <sample text>, count(*) OVER () AS total ... LIMIT n` query
/// and return (total matching rows, up to 5 sample labels).
async fn sample_and_count(pool: &PgPool, sql: &str) -> AppResult<(i64, String)> {
    let rows = sqlx::query(sql)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("audit query failed: {e}")))?;

    let total: i64 = rows
        .first()
        .map(|r| r.try_get::<i64, _>("total").unwrap_or(0))
        .unwrap_or(0);

    let mut samples: Vec<String> = Vec::new();
    for row in rows.iter().take(5) {
        if let Ok(s) = row.try_get::<String, _>("sample") {
            samples.push(s);
        }
    }
    let mut joined = samples.join(", ");
    if joined.len() > 220 {
        joined.truncate(217);
        joined.push_str("...");
    }

    Ok((total, joined))
}

/// Database-only consistency checks.
async fn audit_database(pool: &PgPool) -> AppResult<Vec<AuditIssue>> {
    let mut issues = Vec::new();

    // 1. User quota vs sum of their active links.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT (u.username || ' used=' || u.storage_used::text || ' links=' || COALESCE(SUM(f.size), 0)::text) AS sample,
               COUNT(*) OVER () AS total
        FROM users u
        LEFT JOIN user_files uf ON uf.user_id = u.id AND uf.deleted_at IS NULL AND uf.purged_at IS NULL
        LEFT JOIN files f ON f.id = uf.file_id
        GROUP BY u.id
        HAVING u.storage_used != COALESCE(SUM(f.size), 0)
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "quota_drift",
            "warning",
            format!("{total} user(s) have storage_used different from the sum of their active links ({samples})"),
            total,
        ));
    }

    // 2. File ref_count vs number of active links (uploads/imports keep this
    // in sync; historical drift shows up here).
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT (f.blake3_hash || ' ref=' || f.ref_count::text || ' links=' || COUNT(uf.id)::text) AS sample,
               COUNT(*) OVER () AS total
        FROM files f
        LEFT JOIN user_files uf ON uf.file_id = f.id AND uf.deleted_at IS NULL AND uf.purged_at IS NULL
        GROUP BY f.id
        HAVING f.ref_count != COUNT(uf.id)
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "ref_count_drift",
            "warning",
            format!("{total} file(s) have ref_count different from their active link count ({samples})"),
            total,
        ));
    }

    // 3. File metadata rows with no physical storage objects.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT f.blake3_hash AS sample,
               COUNT(*) OVER () AS total
        FROM files f
        LEFT JOIN storage_objects so ON so.file_id = f.id
        WHERE so.id IS NULL
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "dangling_files",
            "error",
            format!("{total} file(s) have no storage objects ({samples})"),
            total,
        ));
    }

    // 4. Storage objects whose files row is missing.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT so.id AS sample,
               COUNT(*) OVER () AS total
        FROM storage_objects so
        LEFT JOIN files f ON f.id = so.file_id
        WHERE f.id IS NULL
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "orphaned_objects",
            "error",
            format!("{total} storage object(s) point at a missing file ({samples})"),
            total,
        ));
    }

    // 5. Storage objects on backends that are not buckets.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT (so.id || ' backend=' || so.backend) AS sample,
               COUNT(*) OVER () AS total
        FROM storage_objects so
        LEFT JOIN buckets b ON b.name = so.backend
        WHERE b.id IS NULL
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "unknown_backend",
            "error",
            format!("{total} storage object(s) live on an unknown backend ({samples})"),
            total,
        ));
    }

    // 6. Duplicate storage objects for the same (file, backend).
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT (so.file_id || ' backend=' || so.backend || ' count=' || COUNT(*)::text) AS sample,
               COUNT(*) OVER () AS total
        FROM storage_objects so
        GROUP BY so.file_id, so.backend
        HAVING COUNT(*) > 1
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "duplicate_objects",
            "error",
            format!("{total} (file, backend) pair(s) have more than one storage object ({samples})"),
            total,
        ));
    }

    // 7. User links pointing at a missing file.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT uf.id AS sample,
               COUNT(*) OVER () AS total
        FROM user_files uf
        LEFT JOIN files f ON f.id = uf.file_id
        WHERE f.id IS NULL
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "links_missing_file",
            "error",
            format!("{total} user_file link(s) point at a missing file ({samples})"),
            total,
        ));
    }

    // 8. User links pointing at a bucket that no longer exists.
    let (total, samples) = sample_and_count(
        pool,
        r#"
        SELECT (uf.id || ' bucket=' || uf.bucket_name) AS sample,
               COUNT(*) OVER () AS total
        FROM user_files uf
        LEFT JOIN buckets b ON b.name = uf.bucket_name
        WHERE uf.bucket_name IS NOT NULL AND b.id IS NULL
        LIMIT 5
        "#,
    )
    .await?;
    if total > 0 {
        issues.push(issue(
            "links_unknown_bucket",
            "error",
            format!("{total} user_file link(s) point at an unknown bucket ({samples})"),
            total,
        ));
    }

    Ok(issues)
}

/// On-disk consistency checks: registered blobs that are missing from disk,
/// and files on disk that no storage object tracks.
async fn audit_physical(
    pool: &PgPool,
    backends: &[(String, Arc<dyn StorageBackend>)],
    bucket_paths: &[(String, String)],
) -> AppResult<Vec<AuditIssue>> {
    let mut issues = Vec::new();

    let objects: Vec<(String, String)> = sqlx::query_as(
        "SELECT backend, storage_path FROM storage_objects ORDER BY id LIMIT $1",
    )
    .bind(PHYSICAL_CHECK_LIMIT)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Internal(format!("failed to list storage objects: {e}")))?;

    let mut missing = 0i64;
    let mut missing_samples: Vec<String> = Vec::new();
    let mut checked = 0i64;
    for (backend_name, storage_path) in &objects {
        let Some((_, backend)) = backends.iter().find(|(n, _)| n == backend_name) else {
            continue; // unknown backends are reported by audit_database
        };
        match backend.get(storage_path).await {
            Ok(Some(_)) => {
                checked += 1;
            }
            Ok(None) => {
                missing += 1;
                checked += 1;
                if missing_samples.len() < 5 {
                    missing_samples.push(storage_path.clone());
                }
            }
            Err(e) => {
                tracing::warn!("audit: failed to read {} on {}: {e}", storage_path, backend_name);
            }
        }
        if checked >= PHYSICAL_CHECK_LIMIT {
            break;
        }
    }
    if missing > 0 {
        let mut message = format!(
            "{missing} storage object(s) have no blob on disk ({})",
            missing_samples.join(", ")
        );
        if checked >= PHYSICAL_CHECK_LIMIT {
            message.push_str(" [first 5000 objects checked]");
        }
        issues.push(issue("missing_blobs", "error", message, missing));
    }

    // Untracked physical files: files inside a bucket's directory that no
    // storage object row accounts for.
    let mut untracked = 0i64;
    let mut untracked_samples: Vec<String> = Vec::new();
    for (bucket_name, bucket_path) in bucket_paths {
        let registered: HashSet<String> = sqlx::query_scalar(
            "SELECT storage_path FROM storage_objects WHERE backend = $1",
        )
        .bind(bucket_name)
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list storage paths: {e}")))?
        .into_iter()
        .collect();

        let root = PathBuf::from(bucket_path);
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&root).min_depth(1) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            if !registered.contains(&rel) {
                untracked += 1;
                if untracked_samples.len() < 5 {
                    untracked_samples.push(rel);
                }
            }
        }
    }
    if untracked > 0 {
        issues.push(issue(
            "untracked_files",
            "warning",
            format!(
                "{untracked} file(s) exist on disk but are not tracked by any storage object ({})",
                untracked_samples.join(", ")
            ),
            untracked,
        ));
    }

    Ok(issues)
}

async fn query_totals(pool: &PgPool) -> AppResult<AuditTotals> {
    sqlx::query_as::<_, AuditTotals>(
        r#"
        SELECT
            (SELECT COUNT(*) FROM users) AS users,
            (SELECT COUNT(*) FROM buckets) AS buckets,
            (SELECT COUNT(*) FROM files) AS files,
            (SELECT COUNT(*) FROM storage_objects) AS storage_objects,
            (SELECT COUNT(*) FROM user_files WHERE deleted_at IS NULL AND purged_at IS NULL) AS links
        "#,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| AppError::Internal(format!("failed to compute audit totals: {e}")))
}

/// GET /api/admin/audit
pub async fn run_audit(
    State(state): State<Arc<AppState>>,
    _user: AuthUser,
) -> AppResult<Json<AuditReport>> {
    let pool = state.db.pool();

    let mut issues = audit_database(pool).await?;

    // Collect registered backends so the on-disk checks can run without
    // holding the storage registry lock across awaits.
    let backends: Vec<(String, Arc<dyn StorageBackend>)> = {
        let registry = state.storage.read().await;
        registry
            .list_backends()
            .iter()
            .filter_map(|name| registry.get(name).map(|b| (name.clone(), b)))
            .collect()
    };

    let bucket_paths: Vec<(String, String)> = sqlx::query_as("SELECT name, path FROM buckets")
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::Internal(format!("failed to list buckets: {e}")))?;

    issues.extend(audit_physical(pool, &backends, &bucket_paths).await?);

    let totals = query_totals(pool).await?;
    let passed = !issues.iter().any(|i| i.severity == "error");

    Ok(Json(AuditReport {
        generated_at: Utc::now().to_rfc3339(),
        totals,
        issues,
        passed,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{audit_database, audit_physical};
    use crate::db::repos::UserFileRepository;
    use crate::storage::backend::StorageBackend;
    use crate::storage::local::LocalFsBackend;

    const DEFAULT_TEST_URL: &str = "postgres://keystone:keystone@localhost:5433/keystone_test";

    // The DB tests share a single test database, so they must run one at a
    // time (each truncates everything at the start).
    static DB_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

    async fn lock_db() -> tokio::sync::MutexGuard<'static, ()> {
        DB_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    /// Connect to a dedicated test database. Skips (returns None) when the
    /// database is not reachable so `cargo test --lib` stays green anywhere.
    /// Set KEYSTONE_TEST_DATABASE_URL to point at a disposable database.
    async fn test_pool() -> Option<PgPool> {
        let url = std::env::var("KEYSTONE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| DEFAULT_TEST_URL.to_string());
        let pool = PgPool::connect(&url).await.ok()?;
        sqlx::migrate!("./migrations").run(&pool).await.ok()?;
        Some(pool)
    }

    async fn truncate_all(pool: &PgPool) {
        sqlx::query(
            "TRUNCATE user_files, user_folders, files, storage_objects, users, buckets, group_buckets, audit_logs CASCADE",
        )
        .execute(pool)
        .await
        .expect("truncate failed");
    }

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    async fn insert_user(pool: &PgPool, id: &str, username: &str, used: i64) {
        sqlx::query(
            "INSERT INTO users (id, username, email, password_hash, role, storage_quota, storage_used, created_at, updated_at)
             VALUES ($1, $2, $3, $4, 'user', 1073741824, $5, $6, $6)",
        )
        .bind(id)
        .bind(username)
        .bind(format!("{username}@test.local"))
        .bind("not-a-real-hash")
        .bind(used)
        .bind(now())
        .execute(pool)
        .await
        .expect("insert user failed");
    }

    async fn insert_bucket(pool: &PgPool, id: &str, name: &str, path: &str) {
        sqlx::query("INSERT INTO buckets (id, name, path, created_at) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(name)
            .bind(path)
            .bind(now())
            .execute(pool)
            .await
            .expect("insert bucket failed");
    }

    async fn insert_file(pool: &PgPool, id: &str, hash: &str, size: i64, ref_count: i64) {
        sqlx::query(
            "INSERT INTO files (id, blake3_hash, original_name, mime_type, size, ref_count, created_at, updated_at)
             VALUES ($1, $2, 'blob.bin', 'application/octet-stream', $3, $4, $5, $5)",
        )
        .bind(id)
        .bind(hash)
        .bind(size)
        .bind(ref_count)
        .bind(now())
        .execute(pool)
        .await
        .expect("insert file failed");
    }

    async fn insert_object(pool: &PgPool, id: &str, file_id: &str, backend: &str, storage_path: &str) {
        sqlx::query(
            "INSERT INTO storage_objects (id, file_id, backend, storage_path, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(file_id)
        .bind(backend)
        .bind(storage_path)
        .bind(now())
        .execute(pool)
        .await
        .expect("insert storage object failed");
    }

    async fn insert_link(pool: &PgPool, id: &str, user_id: &str, file_id: &str, name: &str, bucket: Option<&str>) {
        sqlx::query(
            "INSERT INTO user_files (id, user_id, file_id, original_name, mime_type, bucket_name, created_at)
             VALUES ($1, $2, $3, $4, NULL, $5, $6)",
        )
        .bind(id)
        .bind(user_id)
        .bind(file_id)
        .bind(name)
        .bind(bucket)
        .bind(now())
        .execute(pool)
        .await
        .expect("insert user_file failed");
    }

    fn categories(issues: &[super::AuditIssue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.category).collect()
    }

    #[tokio::test]
    async fn audit_passes_on_a_consistent_bucket() {
        let _db_guard = lock_db().await;
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: test database not reachable (KEYSTONE_TEST_DATABASE_URL)");
            return;
        };
        truncate_all(&pool).await;

        let u1 = Uuid::new_v4().to_string();
        let b1 = Uuid::new_v4().to_string();
        let f1 = Uuid::new_v4().to_string();
        let o1 = Uuid::new_v4().to_string();
        let l1 = Uuid::new_v4().to_string();

        insert_user(&pool, &u1, "consistent", 40).await;
        insert_bucket(&pool, &b1, "b1", "/tmp/audit-b1").await;
        insert_file(&pool, &f1, "aa11", 40, 1).await;
        insert_object(&pool, &o1, &f1, "b1", "b3/aa/aa11").await;
        insert_link(&pool, &l1, &u1, &f1, "a.txt", Some("b1")).await;

        let issues = audit_database(&pool).await.expect("audit failed");
        assert!(
            issues.is_empty(),
            "expected a clean audit, got: {issues:?}"
        );

        let totals = super::query_totals(&pool).await.expect("totals failed");
        assert_eq!(totals.users, 1);
        assert_eq!(totals.buckets, 1);
        assert_eq!(totals.files, 1);
        assert_eq!(totals.storage_objects, 1);
        assert_eq!(totals.links, 1);
    }

    #[tokio::test]
    async fn audit_detects_drift_and_orphans() {
        let _db_guard = lock_db().await;
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: test database not reachable (KEYSTONE_TEST_DATABASE_URL)");
            return;
        };
        truncate_all(&pool).await;

        let u1 = Uuid::new_v4().to_string();
        let b1 = Uuid::new_v4().to_string();
        let f1 = Uuid::new_v4().to_string();
        let f2 = Uuid::new_v4().to_string();
        let f3 = Uuid::new_v4().to_string();
        let f4 = Uuid::new_v4().to_string(); // deleted below to orphan its object
        let f5 = Uuid::new_v4().to_string(); // deleted below to orphan its link

        insert_user(&pool, &u1, "drifty", 50).await; // quota drift: links only sum to 40
        insert_bucket(&pool, &b1, "b1", "/tmp/audit-b1").await;
        insert_file(&pool, &f1, "aa11", 40, 1).await;
        insert_file(&pool, &f2, "bb22", 10, 3).await; // ref drift: 0 active links
        insert_file(&pool, &f3, "cc33", 10, 1).await; // dangling: no storage objects
        insert_file(&pool, &f4, "dd44", 10, 1).await;
        insert_file(&pool, &f5, "ee55", 10, 1).await;

        insert_object(&pool, &Uuid::new_v4().to_string(), &f1, "b1", "b3/aa/aa11").await;
        insert_object(&pool, &Uuid::new_v4().to_string(), &f2, "b1", "b3/bb/bb22").await;
        // Object on an unknown backend.
        insert_object(&pool, &Uuid::new_v4().to_string(), &f1, "ghost", "b3/aa/dupe1").await;
        // Duplicate (f1, b1) pair.
        insert_object(&pool, &Uuid::new_v4().to_string(), &f1, "b1", "b3/aa/aa11-dupe").await;
        // Orphaned object: create it under a real file, then delete the file.
        // (Drop the FK tests are allowed to produce state the schema prevents
        // in production — the audit exists exactly to catch it.)
        sqlx::query("ALTER TABLE storage_objects DROP CONSTRAINT IF EXISTS storage_objects_file_id_fkey")
            .execute(&pool)
            .await
            .expect("drop fk failed");
        sqlx::query("ALTER TABLE user_files DROP CONSTRAINT IF EXISTS user_files_file_id_fkey")
            .execute(&pool)
            .await
            .expect("drop fk failed");
        let orphan_obj = Uuid::new_v4().to_string();
        insert_object(&pool, &orphan_obj, &f4, "b1", "b3/zz/orphan").await;
        sqlx::query("DELETE FROM files WHERE id = $1")
            .bind(&f4)
            .execute(&pool)
            .await
            .expect("delete f4 failed");

        insert_link(&pool, &Uuid::new_v4().to_string(), &u1, &f1, "a.txt", Some("b1")).await;
        // Link whose file row is missing (created under a real file, then the
        // file is deleted).
        let orphan_link = Uuid::new_v4().to_string();
        insert_link(&pool, &orphan_link, &u1, &f5, "ghost-file.txt", Some("b1")).await;
        sqlx::query("DELETE FROM files WHERE id = $1")
            .bind(&f5)
            .execute(&pool)
            .await
            .expect("delete f5 failed");
        // Link whose bucket is missing.
        insert_link(&pool, &Uuid::new_v4().to_string(), &u1, &f1, "ghost-bucket.txt", Some("ghost")).await;

        let issues = audit_database(&pool).await.expect("audit failed");
        let cats = categories(&issues);
        let count = |cat: &str| {
            issues
                .iter()
                .find(|i| i.category == cat)
                .map(|i| i.count)
                .unwrap_or(0)
        };

        assert!(cats.contains(&"quota_drift"), "expected quota_drift, got {cats:?}");
        assert!(cats.contains(&"ref_count_drift"), "expected ref_count_drift, got {cats:?}");
        assert_eq!(count("dangling_files"), 1);
        assert_eq!(count("orphaned_objects"), 1);
        assert_eq!(count("unknown_backend"), 1);
        assert_eq!(count("duplicate_objects"), 1);
        assert_eq!(count("links_missing_file"), 1);
        assert_eq!(count("links_unknown_bucket"), 1);
    }

    #[tokio::test]
    async fn same_file_and_name_can_exist_in_two_buckets_but_not_twice_in_one() {
        let _db_guard = lock_db().await;
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: test database not reachable (KEYSTONE_TEST_DATABASE_URL)");
            return;
        };
        truncate_all(&pool).await;

        let u1 = Uuid::new_v4().to_string();
        let b1 = Uuid::new_v4().to_string();
        let b2 = Uuid::new_v4().to_string();
        let f1 = Uuid::new_v4().to_string();

        insert_user(&pool, &u1, "multi", 0).await;
        insert_bucket(&pool, &b1, "b1", "/tmp/audit-b1").await;
        insert_bucket(&pool, &b2, "b2", "/tmp/audit-b2").await;
        insert_file(&pool, &f1, "dd44", 10, 1).await;

        insert_link(&pool, &Uuid::new_v4().to_string(), &u1, &f1, "a.txt", Some("b1")).await;
        insert_link(&pool, &Uuid::new_v4().to_string(), &u1, &f1, "a.txt", Some("b2")).await;

        let err = sqlx::query(
            "INSERT INTO user_files (id, user_id, file_id, original_name, mime_type, bucket_name, created_at)
             VALUES ($1, $2, $3, 'a.txt', NULL, 'b1', $4)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&u1)
        .bind(&f1)
        .bind(now())
        .execute(&pool)
        .await
        .expect_err("duplicate link in the same bucket should be rejected");

        match err {
            sqlx::Error::Database(db_err) => {
                assert!(
                    db_err.is_unique_violation(),
                    "expected a unique violation, got: {db_err}"
                );
            }
            other => panic!("expected a database error, got: {other}"),
        }
    }

    #[tokio::test]
    async fn find_active_in_bucket_is_bucket_scoped() {
        let _db_guard = lock_db().await;
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: test database not reachable (KEYSTONE_TEST_DATABASE_URL)");
            return;
        };
        truncate_all(&pool).await;

        let u1 = Uuid::new_v4().to_string();
        let b1 = Uuid::new_v4().to_string();
        let b2 = Uuid::new_v4().to_string();
        let f1 = Uuid::new_v4().to_string();
        let l1 = Uuid::new_v4().to_string();

        insert_user(&pool, &u1, "finder", 0).await;
        insert_bucket(&pool, &b1, "b1", "/tmp/audit-b1").await;
        insert_bucket(&pool, &b2, "b2", "/tmp/audit-b2").await;
        insert_file(&pool, &f1, "ee55", 10, 1).await;

        insert_link(&pool, &l1, &u1, &f1, "a.txt", Some("b1")).await;
        insert_link(&pool, &Uuid::new_v4().to_string(), &u1, &f1, "a.txt", Some("b2")).await;

        let uid = Uuid::parse_str(&u1).unwrap();
        let fid = Uuid::parse_str(&f1).unwrap();

        let in_b1 = UserFileRepository::find_active_in_bucket_by_user_file_and_name(
            &pool, uid, fid, "a.txt", "b1",
        )
        .await
        .expect("query failed");
        let in_b2 = UserFileRepository::find_active_in_bucket_by_user_file_and_name(
            &pool, uid, fid, "a.txt", "b2",
        )
        .await
        .expect("query failed");

        assert!(in_b1.is_some(), "b1 link should be found");
        assert!(in_b2.is_some(), "b2 link should be found independently");
        assert_ne!(in_b1.unwrap().id, in_b2.unwrap().id);

        // Soft-delete the b1 link: the same name in b2 must still be found,
        // and b1 must now be empty.
        sqlx::query("UPDATE user_files SET deleted_at = $1 WHERE id = $2")
            .bind(now())
            .bind(&l1)
            .execute(&pool)
            .await
            .expect("soft delete failed");

        let after_delete = UserFileRepository::find_active_in_bucket_by_user_file_and_name(
            &pool, uid, fid, "a.txt", "b1",
        )
        .await
        .expect("query failed");
        let b2_still = UserFileRepository::find_active_in_bucket_by_user_file_and_name(
            &pool, uid, fid, "a.txt", "b2",
        )
        .await
        .expect("query failed");

        assert!(after_delete.is_none(), "soft-deleted b1 link must not be found");
        assert!(b2_still.is_some(), "b2 link must be unaffected by the b1 delete");
    }

    #[tokio::test]
    async fn physical_audit_detects_missing_and_untracked_blobs() {
        let _db_guard = lock_db().await;
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: test database not reachable (KEYSTONE_TEST_DATABASE_URL)");
            return;
        };
        truncate_all(&pool).await;

        let dir = tempfile::tempdir().expect("tempdir failed");
        let root = dir.path().to_string_lossy().to_string();
        let bucket_name = "phys";
        let backend = Arc::new(LocalFsBackend::new(&root).expect("backend init failed"));

        let f1 = Uuid::new_v4().to_string();
        insert_file(&pool, &f1, "ff66", 10, 1).await;
        // Registered object whose blob was never written to disk.
        insert_object(&pool, &Uuid::new_v4().to_string(), &f1, bucket_name, "b3/aa/tracked-but-missing").await;
        // Registered object that does exist on disk.
        std::fs::create_dir_all(format!("{root}/b3/bb")).expect("mkdir failed");
        std::fs::write(format!("{root}/b3/bb/present.bin"), b"data").expect("write failed");
        insert_object(&pool, &Uuid::new_v4().to_string(), &f1, bucket_name, "b3/bb/present.bin").await;
        // File on disk that no storage object tracks.
        std::fs::create_dir_all(format!("{root}/b3/cc")).expect("mkdir failed");
        std::fs::write(format!("{root}/b3/cc/stray.bin"), b"stray").expect("write failed");

        let backends: Vec<(String, Arc<dyn StorageBackend>)> = vec![(bucket_name.to_string(), backend)];
        let bucket_paths = vec![(bucket_name.to_string(), root.clone())];
        let issues = audit_physical(&pool, &backends, &bucket_paths)
            .await
            .expect("physical audit failed");

        let count = |cat: &str| {
            issues
                .iter()
                .find(|i| i.category == cat)
                .map(|i| i.count)
                .unwrap_or(0)
        };
        assert_eq!(count("missing_blobs"), 1, "issues: {issues:?}");
        assert_eq!(count("untracked_files"), 1, "issues: {issues:?}");
    }
}