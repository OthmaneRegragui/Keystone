use std::net::IpAddr;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum AppEnv {
    Test,
    Development,
    Production,
}

impl Default for AppEnv {
    fn default() -> Self {
        Self::Development
    }
}

impl AppEnv {
    pub fn is_test(self) -> bool {
        self == Self::Test
    }

    pub fn is_development(self) -> bool {
        self == Self::Development
    }

    pub fn is_production(self) -> bool {
        self == Self::Production
    }

    fn default_database_url(self) -> &'static str {
        match self {
            Self::Test => "postgres://keystone:keystone@localhost:5432/keystone_test",
            Self::Development => "postgres://keystone:keystone@localhost:5432/keystone",
            Self::Production => "postgres://keystone:keystone@localhost:5432/keystone",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: IpAddr,
    pub port: u16,
    pub workers: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: IpAddr::from([127, 0, 0, 1]),
            port: 3000,
            workers: available_parallelism(),
        }
    }
}

impl ServerConfig {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub connect_timeout_secs: u64,
    pub idle_timeout_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 10,
            min_connections: 1,
            connect_timeout_secs: 30,
            idle_timeout_secs: 600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub jwt_expiration_secs: i64,
    pub api_key_prefix: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "change-me-in-production".to_string(),
            jwt_expiration_secs: 43200,
            api_key_prefix: "ks_".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub backend: String,
    /// Comma-separated list of directories where files are stored.
    /// First path is the default. Example: "/mnt/disk1/storage,/mnt/disk2/storage"
    pub local_paths: Vec<String>,
    pub max_upload_size_mb: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: "local".to_string(),
            local_paths: vec!["./storage".to_string()],
            max_upload_size_mb: 100,
        }
    }
}

impl StorageConfig {
    /// Parse a comma-separated string into a Vec of paths.
    pub fn parse_paths(raw: &str) -> Vec<String> {
        raw.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    pub queue_size: usize,
    pub poll_interval_ms: u64,
    pub batch_size: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            queue_size: 1000,
            poll_interval_ms: 500,
            batch_size: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub requests_per_second: u64,
    pub burst_size: u32,
    /// When running behind a reverse proxy, the TCP peer address is the proxy,
    /// so per-IP limiting would throttle all users together. Set this to `true`
    /// to derive the client IP from the leftmost `X-Forwarded-For` entry
    /// instead. Only enable it when the proxy overwrites/strips untrusted
    /// `X-Forwarded-For` values (otherwise clients can spoof their identity).
    pub trust_proxy_headers: bool,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 50,
            burst_size: 100,
            trust_proxy_headers: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    pub allowed_origins: Vec<String>,
    pub allowed_methods: Vec<String>,
    pub allowed_headers: Vec<String>,
    pub allow_credentials: bool,
    pub max_age_secs: u64,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: vec!["http://localhost:3000".to_string()],
            allowed_methods: vec![
                "GET".to_string(),
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string(),
                "PATCH".to_string(),
                "OPTIONS".to_string(),
            ],
            allowed_headers: vec![
                "authorization".to_string(),
                "content-type".to_string(),
                "x-request-id".to_string(),
            ],
            allow_credentials: true,
            max_age_secs: 3600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Token used to encrypt/decrypt sensitive values at rest.
    /// Set it in the environment as a plain `ENCRYPTION_TOKEN` (or
    /// `KEYSTONE__SECURITY__ENCRYPTION_TOKEN`). Generate one with:
    /// `openssl rand -base64 32`
    pub encryption_token: String,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            encryption_token: "change-me-in-production".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub app_env: AppEnv,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub storage: StorageConfig,
    pub worker: WorkerConfig,
    pub rate_limit: RateLimitConfig,
    pub cors: CorsConfig,
    pub security: SecurityConfig,
}

impl Settings {
    pub fn load() -> Result<Self, config::ConfigError> {
        let _ = dotenvy::dotenv().ok();

        let app_env: AppEnv = std::env::var("APP_ENV")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default();

        let default_db_url = resolve_database_url(app_env);
        let workers = available_parallelism();

        let defaults_json = serde_json::json!({
            "app_env": app_env_to_str(app_env),
            "server": {
                "host": "127.0.0.1",
                "port": 3000,
                "workers": workers,
            },
            "database": {
                "url": default_db_url,
                "max_connections": 10,
                "min_connections": 1,
                "connect_timeout_secs": 30,
                "idle_timeout_secs": 600,
            },
            "auth": {
                "jwt_secret": "change-me-in-production",
                "jwt_expiration_secs": 43200,
                "api_key_prefix": "ks_",
            },
            "storage": {
                "backend": "local",
                "local_paths": ["./storage"],
                "max_upload_size_mb": 100
            },
            "worker": {
                "queue_size": 1000,
                "poll_interval_ms": 500,
                "batch_size": 10,
            },
            "rate_limit": {
                "enabled": true,
                "requests_per_second": 50,
                "burst_size": 100,
                "trust_proxy_headers": false,
            },
            "cors": {
                "allowed_origins": ["http://localhost:3000"],
                "allowed_methods": ["GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"],
                "allowed_headers": ["authorization", "content-type", "x-request-id"],
                "allow_credentials": true,
                "max_age_secs": 3600,
            },
            "security": {
                "encryption_token": "change-me-in-production",
            },
        });

        let defaults_str = defaults_json.to_string();

        let config = config::Config::builder()
            .add_source(
                config::File::from_str(&defaults_str, config::FileFormat::Json)
                    .required(true),
            )
            .add_source(
                config::Environment::with_prefix("KEYSTONE")
                    .separator("__")
                    .try_parsing(true)
                    .ignore_empty(true),
            )
            .build()?;

        let mut settings: Settings = config.try_deserialize()?;

        // Handle comma-separated STORAGE_LOCAL_PATHS env var
        // The config crate may deserialize it as a single string, so we parse it
        if let Ok(raw) = std::env::var("STORAGE_LOCAL_PATHS") {
            settings.storage.local_paths = StorageConfig::parse_paths(&raw);
        }

        // Handle the plain ENCRYPTION_TOKEN env var (same convention as
        // STORAGE_LOCAL_PATHS). dotenvy already loaded `.env` above, so a value
        // set there — or in the process environment — wins over the default.
        if let Ok(raw) = std::env::var("ENCRYPTION_TOKEN") {
            if !raw.trim().is_empty() {
                settings.security.encryption_token = raw.trim().to_string();
            }
        }

        // Handle the plain JWT_SECRET env var (documented in .env.example and
        // run.sh). The KEYSTONE__AUTH__JWT_SECRET form is more specific and
        // always wins when both are set. Empty values are ignored so a blank
        // `JWT_SECRET=` cannot silently reset the secret to the default.
        if std::env::var_os("KEYSTONE__AUTH__JWT_SECRET").is_none() {
            if let Ok(raw) = std::env::var("JWT_SECRET") {
                if !raw.trim().is_empty() {
                    settings.auth.jwt_secret = raw.trim().to_string();
                }
            }
        }

        // JWT_EXPIRY_MINUTES (documented in .env.example) maps to the internal
        // jwt_expiration_secs field (seconds).
        if std::env::var_os("KEYSTONE__AUTH__JWT_EXPIRATION_SECS").is_none() {
            if let Ok(raw) = std::env::var("JWT_EXPIRY_MINUTES") {
                if let Ok(minutes) = raw.trim().parse::<i64>() {
                    if minutes > 0 {
                        settings.auth.jwt_expiration_secs = minutes * 60;
                    }
                }
            }
        }

        // CORS_ALLOWED_ORIGINS: comma-separated list, same convention as
        // STORAGE_LOCAL_PATHS. Ignored when KEYSTONE__CORS__ALLOWED_ORIGINS is
        // set explicitly. Wildcards are NOT expanded here; tower-http only
        // treats `*` as a wildcard via CorsLayer::allow_origin(AllowOrigin::any),
        // so a literal `*` entry never becomes a permissive wildcard.
        if std::env::var_os("KEYSTONE__CORS__ALLOWED_ORIGINS").is_none() {
            if let Ok(raw) = std::env::var("CORS_ALLOWED_ORIGINS") {
                let origins: Vec<String> = raw
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !origins.is_empty() {
                    settings.cors.allowed_origins = origins;
                }
            }
        }

        Ok(settings)
    }

    pub fn app_env(&self) -> AppEnv {
        self.app_env
    }

    pub fn is_test(&self) -> bool {
        self.app_env.is_test()
    }

    pub fn is_development(&self) -> bool {
        self.app_env.is_development()
    }

    pub fn is_production(&self) -> bool {
        self.app_env.is_production()
    }
}

fn app_env_to_str(env: AppEnv) -> &'static str {
    match env {
        AppEnv::Test => "test",
        AppEnv::Development => "development",
        AppEnv::Production => "production",
    }
}

/// Percent-encode a single URL component (user, password, database name) for
/// use inside a connection URL. Only RFC 3986 unreserved characters are left
/// untouched; everything else becomes `%XX`. std-only.
fn url_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// Resolve the database URL.
///
/// Precedence:
/// 1. `DATABASE_URL` if explicitly set
/// 2. Derived from `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`/
///    `POSTGRES_PORT` when those are set (so bare-metal runs don't need a
///    hand-maintained `DATABASE_URL`)
/// 3. The per-environment built-in default
///
/// Note: `KEYSTONE__DATABASE__URL` (used by docker-compose to point at the
/// `postgres` container) is read by the config crate's env source below and
/// always overrides the value set here.
fn resolve_database_url(app_env: AppEnv) -> String {
    if let Ok(url) = std::env::var("DATABASE_URL") {
        if !url.trim().is_empty() {
            return url.trim().to_string();
        }
    }

    let postgres_user = std::env::var("POSTGRES_USER").ok();
    let postgres_password = std::env::var("POSTGRES_PASSWORD").ok();
    let postgres_db = std::env::var("POSTGRES_DB").ok();

    if let (Some(user), Some(password), Some(db)) = (postgres_user, postgres_password, postgres_db) {
        if !user.trim().is_empty() && !db.trim().is_empty() {
            let port = std::env::var("POSTGRES_PORT")
                .ok()
                .and_then(|p| p.trim().parse::<u16>().ok())
                .unwrap_or(5432);
            // Percent-encode the userinfo and database components so passwords
            // containing `@`, `:`, `/`, `%` or other URL-special characters
            // cannot corrupt the connection URL or be reinterpreted as
            // separators (sqlx decodes the percent-encoded components).
            return format!(
                "postgres://{}:{}@localhost:{}/{}",
                url_encode_component(user.trim()),
                url_encode_component(password.trim()),
                port,
                url_encode_component(db.trim()),
            );
        }
    }

    app_env.default_database_url().to_string()
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate process-global env vars (cargo runs tests
    /// in parallel threads sharing the process env). Without this, tests that
    /// pin POSTGRES_*/DATABASE_URL/ENCRYPTION_TOKEN race each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_app_env_default() {
        assert_eq!(AppEnv::default(), AppEnv::Development);
    }

    #[test]
    fn test_app_env_is_variants() {
        assert!(AppEnv::Development.is_development());
        assert!(!AppEnv::Test.is_development());
        assert!(!AppEnv::Production.is_development());
        assert!(AppEnv::Test.is_test());
        assert!(AppEnv::Production.is_production());
    }

    #[test]
    fn test_app_env_display_and_parse() {
        assert_eq!(AppEnv::Test.to_string(), "test");
        assert_eq!(AppEnv::Development.to_string(), "development");
        assert_eq!(AppEnv::Production.to_string(), "production");

        assert_eq!("test".parse::<AppEnv>().unwrap(), AppEnv::Test);
        assert_eq!("development".parse::<AppEnv>().unwrap(), AppEnv::Development);
        assert_eq!("production".parse::<AppEnv>().unwrap(), AppEnv::Production);
        assert!("invalid".parse::<AppEnv>().is_err());
    }

    #[test]
    fn test_app_env_default_database_url() {
        assert_eq!(
            AppEnv::Test.default_database_url(),
            "postgres://keystone:keystone@localhost:5432/keystone_test"
        );
        assert_eq!(
            AppEnv::Development.default_database_url(),
            "postgres://keystone:keystone@localhost:5432/keystone"
        );
        assert_eq!(
            AppEnv::Production.default_database_url(),
            "postgres://keystone:keystone@localhost:5432/keystone"
        );
    }

    #[test]
    fn test_server_config_address() {
        let config = ServerConfig::default();
        assert_eq!(config.address(), "127.0.0.1:3000");
    }

    #[test]
    fn test_settings_load_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE") || k.starts_with("POSTGRES_") || k == "DATABASE_URL")
            .collect();
        let prev_keys: std::collections::HashSet<String> = prev.iter().map(|(k, _)| k.clone()).collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        let prev_app_env = std::env::var("APP_ENV").ok();
        std::env::remove_var("APP_ENV");
        let prev_enc_token = std::env::var("ENCRYPTION_TOKEN").ok();
        let prev_jwt_secret = std::env::var("JWT_SECRET").ok();

        // Set ENCRYPTION_TOKEN/JWT_SECRET to empty strings instead of removing
        // them: dotenvy won't overwrite an existing var (so .env can't leak a
        // real token into this test), and Settings::load ignores empty values,
        // so the built-in defaults are what's asserted below.
        std::env::set_var("ENCRYPTION_TOKEN", "");
        std::env::set_var("JWT_SECRET", "");

        // Pin the POSTGRES_* vars (dotenvy won't override existing ones, so
        // this keeps the default-URL assertion deterministic).
        std::env::set_var("POSTGRES_USER", "keystone");
        std::env::set_var("POSTGRES_PASSWORD", "keystone");
        std::env::set_var("POSTGRES_DB", "keystone");
        std::env::set_var("POSTGRES_PORT", "5432");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(settings.app_env, AppEnv::Development);
        assert_eq!(settings.server.port, 3000);
        assert_eq!(settings.database.url, "postgres://keystone:keystone@localhost:5432/keystone");
        assert_eq!(settings.auth.jwt_secret, "change-me-in-production");
        assert_eq!(settings.security.encryption_token, "change-me-in-production");

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        if let Some(v) = prev_app_env {
            std::env::set_var("APP_ENV", v);
        }
        if let Some(v) = prev_enc_token {
            std::env::set_var("ENCRYPTION_TOKEN", v);
        } else {
            std::env::remove_var("ENCRYPTION_TOKEN");
        }
        if let Some(v) = prev_jwt_secret {
            std::env::set_var("JWT_SECRET", v);
        } else {
            std::env::remove_var("JWT_SECRET");
        }
        for k in ["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB", "POSTGRES_PORT"] {
            if !prev_keys.contains(k) {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn test_database_url_derived_from_postgres() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE") || k.starts_with("POSTGRES_") || k == "DATABASE_URL")
            .collect();
        let prev_keys: std::collections::HashSet<String> = prev.iter().map(|(k, _)| k.clone()).collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        std::env::set_var("POSTGRES_USER", "custom_user");
        std::env::set_var("POSTGRES_PASSWORD", "custom_pass");
        std::env::set_var("POSTGRES_DB", "custom_db");
        std::env::set_var("POSTGRES_PORT", "5433");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(
            settings.database.url,
            "postgres://custom_user:custom_pass@localhost:5433/custom_db"
        );

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        for k in ["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB", "POSTGRES_PORT"] {
            if !prev_keys.contains(k) {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn test_database_url_explicit_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE") || k.starts_with("POSTGRES_") || k == "DATABASE_URL")
            .collect();
        let prev_keys: std::collections::HashSet<String> = prev.iter().map(|(k, _)| k.clone()).collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        std::env::set_var("POSTGRES_USER", "custom_user");
        std::env::set_var("POSTGRES_PASSWORD", "custom_pass");
        std::env::set_var("POSTGRES_DB", "custom_db");
        std::env::set_var("POSTGRES_PORT", "5433");
        std::env::set_var("DATABASE_URL", "postgres://override:pass@remote:5432/external");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(
            settings.database.url,
            "postgres://override:pass@remote:5432/external"
        );

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        for k in ["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB", "POSTGRES_PORT", "DATABASE_URL"] {
            if !prev_keys.contains(k) {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn test_settings_encryption_token_from_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE"))
            .collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        let prev_app_env = std::env::var("APP_ENV").ok();
        std::env::remove_var("APP_ENV");
        let prev_enc_token = std::env::var("ENCRYPTION_TOKEN").ok();
        std::env::remove_var("ENCRYPTION_TOKEN");
        std::env::set_var("ENCRYPTION_TOKEN", "unit-test-token-123");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(settings.security.encryption_token, "unit-test-token-123");

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        if let Some(v) = prev_app_env {
            std::env::set_var("APP_ENV", v);
        }
        if let Some(v) = prev_enc_token {
            std::env::set_var("ENCRYPTION_TOKEN", v);
        } else {
            std::env::remove_var("ENCRYPTION_TOKEN");
        }
    }

    #[test]
    fn test_settings_serialization_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE"))
            .collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }

        let settings = Settings::load().expect("settings should load");
        let json = serde_json::to_string(&settings).expect("should serialize");
        let deserialized: Settings = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(settings.server.port, deserialized.server.port);
        assert_eq!(settings.app_env, deserialized.app_env);

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
    }

    #[test]
    fn test_url_encode_component() {
        assert_eq!(url_encode_component("keystone"), "keystone");
        assert_eq!(url_encode_component("p@ss:w/rd%"), "p%40ss%3Aw%2Frd%25");
        assert_eq!(url_encode_component("user name"), "user%20name");
        assert_eq!(url_encode_component(""), "");
    }

    #[test]
    fn test_database_url_encodes_special_chars_in_password() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE") || k.starts_with("POSTGRES_") || k == "DATABASE_URL")
            .collect();
        let prev_keys: std::collections::HashSet<String> = prev.iter().map(|(k, _)| k.clone()).collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        std::env::set_var("POSTGRES_USER", "user");
        std::env::set_var("POSTGRES_PASSWORD", "p@ss:w/rd%");
        std::env::set_var("POSTGRES_DB", "db");
        std::env::set_var("POSTGRES_PORT", "5432");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(
            settings.database.url,
            "postgres://user:p%40ss%3Aw%2Frd%25@localhost:5432/db"
        );

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        for k in ["POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB", "POSTGRES_PORT"] {
            if !prev_keys.contains(k) {
                std::env::remove_var(k);
            }
        }
    }

    #[test]
    fn test_plain_jwt_secret_env_is_honored() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k.starts_with("KEYSTONE"))
            .collect();
        for (k, _) in &prev {
            std::env::remove_var(k);
        }
        let prev_jwt = std::env::var("JWT_SECRET").ok();
        std::env::set_var("JWT_SECRET", "unit-test-jwt-secret-0123456789");

        let settings = Settings::load().expect("settings should load");
        assert_eq!(settings.auth.jwt_secret, "unit-test-jwt-secret-0123456789");

        for (k, v) in prev {
            std::env::set_var(k, v);
        }
        if let Some(v) = prev_jwt {
            std::env::set_var("JWT_SECRET", v);
        } else {
            std::env::remove_var("JWT_SECRET");
        }
    }
}
