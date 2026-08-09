use std::sync::Arc;
use tokio::sync::RwLock;

use keystone::api::routes::api_routes;
use keystone::config::Settings;
use keystone::db::repos::UserRepository;
use keystone::db::rows::user_row::CreateUserData;
use keystone::db::Database;
use keystone::models::UserRole;
use keystone::storage::local::LocalFsBackend;
use keystone::storage::StorageRegistry;
use keystone::utils::auth::jwt::JwtService;
use keystone::utils::auth::session::SessionService;
use keystone::AppState;
use uuid::Uuid;

/// Create a database connection for testing.
///
/// Requires a running PostgreSQL instance (see `test_db_url` for the default
/// connection string and the `TEST_DATABASE_URL`/`TEST_DATABASE_BASE_URL`
/// overrides). `Database::new` runs migrations automatically, so the schema is
/// created on first use.
///
/// By default each test binary gets its own dedicated database (named after
/// the binary, e.g. `keystone_test_api_auth_tests`), so concurrently running
/// test binaries never share rows. The database is created on demand if it
/// does not exist yet.
pub async fn setup_test_db() -> Database {
    let url = test_db_url();
    ensure_test_db_exists(&url).await;
    Database::new(&url)
        .await
        .expect("Failed to create test database")
}

/// Like `setup_test_db`, but truncates all application tables first so the
/// test starts from a clean, freshly-migrated state regardless of what
/// sibling tests in the same binary created.
///
/// NOTE: only safe when tests within a binary run serially
/// (`RUST_TEST_THREADS=1` or `--test-threads=1`).
pub async fn setup_reset_db() -> Database {
    let db = setup_test_db().await;
    reset_db(&db).await;
    db
}

/// Create a temporary directory for file storage tests
pub fn setup_test_storage() -> tempfile::TempDir {
    tempfile::tempdir().expect("Failed to create temp dir")
}

/// Derive a stable, per-binary test database name from the test executable,
/// e.g. `target/debug/deps/api_auth_tests-11e270538e4e92a2` becomes
/// `keystone_test_api_auth_tests`.
pub fn test_db_name() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "tests".to_string());

    // Strip any extension, then the trailing `-<hash>` cargo appends to
    // binaries inside target/debug/deps/.
    let stem = exe.split('.').next().unwrap_or(&exe);
    let mut parts: Vec<&str> = stem.split('-').collect();
    if parts.len() > 1 {
        let last = parts.last().unwrap();
        if last.len() >= 8 && last.chars().all(|c| c.is_ascii_hexdigit()) {
            parts.pop();
        }
    }
    format!("keystone_test_{}", parts.join("_"))
}

/// Base URL of a maintenance/admin database (defaults to the `postgres` DB on
/// localhost). Used to create per-binary test databases on demand.
fn test_admin_url() -> String {
    std::env::var("TEST_DATABASE_BASE_URL")
        .unwrap_or_else(|_| "postgres://keystone:keystone@localhost:5432/postgres".to_string())
}

/// Get the database URL for tests.
///
/// * If `TEST_DATABASE_URL` is set, it is used verbatim (the caller is
///   responsible for creating that database, e.g. CI provisioning).
/// * Otherwise a per-binary database name is derived and the URL points at
///   `TEST_DATABASE_BASE_URL` (default `.../postgres` on localhost).
pub fn test_db_url() -> String {
    if let Ok(url) = std::env::var("TEST_DATABASE_URL") {
        return url;
    }
    // Replace the maintenance database name (e.g. `/postgres`) with the
    // per-binary test database name, keeping host/port/user/password.
    let admin_url = test_admin_url();
    let base = admin_url.trim_end_matches('/');
    let base = base
        .rsplit_once('/')
        .map(|(host, _db)| host)
        .unwrap_or(base);
    format!("{}/{}", base, test_db_name())
}

/// Create the per-binary test database if it does not exist yet. No-op when
/// `TEST_DATABASE_URL` is set explicitly (assumed pre-provisioned).
async fn ensure_test_db_exists(_url: &str) {
    if std::env::var("TEST_DATABASE_URL").is_ok() {
        return;
    }
    let db_name = test_db_name();

    let pool = sqlx::PgPool::connect(&test_admin_url())
        .await
        .expect("Failed to connect to maintenance database for test setup");

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)",
    )
    .bind(&db_name)
    .fetch_one(&pool)
    .await
    .expect("Failed to check for test database");

    if !exists {
        // `db_name` is derived from the executable name and sanitized
        // (`[a-z0-9_]` only), and is quoted, so no injection is possible.
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&pool)
            .await
            .expect("Failed to create per-binary test database");
    }

    pool.close().await;
}

/// Truncate every application table and re-seed the default admin settings,
/// mirroring the state right after `run_migrations` on a brand-new database.
///
/// NOTE: only safe when tests within a binary run serially
/// (`RUST_TEST_THREADS=1` or `--test-threads=1`).
pub async fn reset_db(db: &Database) {
    sqlx::query(
        "TRUNCATE TABLE \
            files, users, api_keys, storage_objects, audit_logs, \
            admin_settings, buckets, user_groups, group_members, group_buckets, \
            storage_paths, user_folders, user_files \
         CASCADE",
    )
    .execute(db.pool())
    .await
    .expect("Failed to truncate test tables");

    // Re-seed defaults, mirroring migration 0002_admin_settings.sql.
    sqlx::query(
        "INSERT INTO admin_settings (key, value, updated_at) VALUES \
            ('block_registrations', 'true', NOW()), \
            ('default_bucket', 'default', NOW()), \
            ('allow_multi_bucket', 'false', NOW()) \
         ON CONFLICT (key) DO NOTHING",
    )
    .execute(db.pool())
    .await
    .expect("Failed to re-seed admin settings");
}

/// Build a full test AppState with in-memory DB, temp storage, JWT, etc.
pub async fn build_test_state() -> (Arc<AppState>, tempfile::TempDir) {
    let db = setup_test_db().await;
    let temp_dir = setup_test_storage();

    let jwt_service = JwtService::new("test-secret-key-for-api-tests", 60);
    let session_service = SessionService::new(720);

    let mut storage = StorageRegistry::new();
    let backend = LocalFsBackend::new(temp_dir.path()).expect("Failed to create backend");
    storage.register("default".to_string(), Arc::new(backend));

    let config = Settings::load().unwrap_or_else(|_| {
        // Fallback: create a minimal config for tests
        serde_json::from_str(r#"{
            "app_env": "test",
            "server": {"host": "127.0.0.1", "port": 3000, "workers": 1},
            "database": {"url": "postgres://keystone:keystone@localhost:5432/keystone_test", "max_connections": 10, "min_connections": 1, "connect_timeout_secs": 30, "idle_timeout_secs": 600},
            "auth": {"jwt_secret": "test-secret-key-for-api-tests", "jwt_expiration_secs": 43200, "api_key_prefix": "ks_"},
            "storage": {"backend": "local", "local_paths": ["./storage"], "max_upload_size_mb": 100},
            "worker": {"queue_size": 1000, "poll_interval_ms": 500, "batch_size": 10},
            "rate_limit": {"enabled": false, "requests_per_second": 50, "burst_size": 100},
            "cors": {"allowed_origins": ["http://localhost:3000"], "allowed_methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"], "allowed_headers": ["authorization", "content-type"], "allow_credentials": true, "max_age_secs": 3600}
        }"#).unwrap()
    });

    let state = Arc::new(AppState {
        db,
        jwt_service,
        storage: RwLock::new(storage),
        session_service,
        config,
    });

    (state, temp_dir)
}

/// Build a test Axum router with full state (including extension layer for AuthUser extractor)
pub async fn build_test_app() -> (axum::Router, tempfile::TempDir) {
    let (state, temp_dir) = build_test_state().await;
    let app = api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state);
    (app, temp_dir)
}

/// Build a test Axum router whose database has been truncated and re-seeded
/// first, so the test starts from a clean state regardless of sibling tests.
pub async fn build_reset_app() -> (axum::Router, tempfile::TempDir) {
    let (state, temp_dir) = build_test_state().await;
    reset_db(&state.db).await;
    let app = api_routes()
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state);
    (app, temp_dir)
}

/// Create a test user in the database and return (user_id, username, email, password)
pub async fn create_test_user(
    db: &Database,
    role: UserRole,
    password: &str,
) -> (Uuid, String, String, String) {
    let username = format!("user_{}", &Uuid::new_v4().to_string()[..8]);
    let email = format!("{}@test.com", username);
    let password_hash = keystone::utils::auth::password::hash_password(password)
        .expect("Failed to hash password");

    let user = UserRepository::create(
        db.pool(),
        CreateUserData {
            username: username.clone(),
            email: email.clone(),
            password_hash,
            role,
            storage_quota: 1_073_741_824, // 1GB
        },
    )
    .await
    .expect("Failed to create test user");

    (user.id, username, email, password.to_string())
}

/// Register a user via the API and return the JSON response
pub async fn register_user(
    app: &axum::Router,
    username: &str,
    email: &str,
    password: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let body = serde_json::json!({
        "username": username,
        "email": email,
        "password": password,
    });

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/auth/register")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Login a user via the API and return the access token
pub async fn login_user(
    app: &axum::Router,
    email: &str,
    password: &str,
) -> String {
    use axum::body::Body;
    use http_body_util::BodyExt;

    let body = serde_json::json!({
        "email": email,
        "password": password,
    });

    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();

    let response = tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request");

    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    json["access_token"].as_str().unwrap().to_string()
}

/// Create a JSON POST request
pub async fn json_post(
    app: &axum::Router,
    uri: &str,
    body: &serde_json::Value,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a JSON POST request with auth
pub async fn json_post_auth(
    app: &axum::Router,
    uri: &str,
    body: &serde_json::Value,
    token: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a JSON PUT request with auth
pub async fn json_put_auth(
    app: &axum::Router,
    uri: &str,
    body: &serde_json::Value,
    token: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a JSON PATCH request with auth
pub async fn json_patch_auth(
    app: &axum::Router,
    uri: &str,
    body: &serde_json::Value,
    token: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a DELETE request with auth
pub async fn delete_auth(
    app: &axum::Router,
    uri: &str,
    body: &serde_json::Value,
    token: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a GET request with auth
pub async fn get_auth(
    app: &axum::Router,
    uri: &str,
    token: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Create a GET request (no auth)
pub async fn get_no_auth(
    app: &axum::Router,
    uri: &str,
) -> axum::http::Response<axum::body::Body> {
    use axum::body::Body;

    let request = axum::http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("Failed to send request")
}

/// Parse response body as JSON
pub async fn response_json(response: axum::http::Response<axum::body::Body>) -> serde_json::Value {
    use http_body_util::BodyExt;
    let body_bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body_bytes).unwrap_or_else(|e| {
        panic!("Failed to parse response as JSON: {}. Body: {}", e, String::from_utf8_lossy(&body_bytes))
    })
}
