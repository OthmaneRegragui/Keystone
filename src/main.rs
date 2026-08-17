use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Router;
use tokio::net::TcpListener;
use axum::middleware as axum_mw;
use axum::http::{Method, HeaderValue, header};
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use keystone::AppState;
use keystone::api::middleware::{
    assign_request_id, catch_panic, rate_limit, request_logging, security_headers, RateLimiter,
};
use keystone::utils::auth::jwt::JwtService;
use keystone::utils::auth::session::SessionService;
use keystone::db::Database;
use keystone::db::repos::{AdminSettingRepository, BucketRepository, UserRepository};
use keystone::storage::StorageRegistry;
use keystone::storage::local::LocalFsBackend;

const DASHBOARD_HTML: &str = include_str!("static/dashboard.html");
const FILES_HTML: &str = include_str!("static/files.html");
const ACCOUNT_HTML: &str = include_str!("static/account.html");
const ADMIN_HTML: &str = include_str!("static/admin.html");
const LOGIN_HTML: &str = include_str!("static/login.html");
const REGISTER_HTML: &str = include_str!("static/register.html");
const SETUP_HTML: &str = include_str!("static/setup.html");
const DOCS_HTML: &str = include_str!("static/docs.html");
const ORPHANS_HTML: &str = include_str!("static/orphans.html");
const BOTS_HTML: &str = include_str!("static/bots.html");

// Vendored assets (self-hosted so the UI works fully offline — no CDN).
const ALPINE_JS: &str = include_str!("static/vendor/alpine.min.js");
const TAILWIND_JS: &str = include_str!("static/vendor/tailwind.min.js");
const LOGO_SVG: &str = include_str!("static/logo.svg");

const KEYSTONE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let settings = keystone::config::Settings::load().expect("failed to load configuration");

    // Record whether we are in production: internal error details are redacted
    // from client responses and HSTS is emitted only then.
    keystone::error::set_production(settings.is_production());

    tracing::info!("Keystone v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Environment: {}", settings.app_env);

    // Fail closed in production: refuse to start with default or weak secrets.
    if settings.is_production() {
        let jwt_secret = &settings.auth.jwt_secret;
        if jwt_secret.contains("change-me") || jwt_secret.len() < 32 {
            panic!(
                "refusing to start in production: JWT_SECRET is unset, the default, or shorter \
                 than 32 bytes. Generate one with: openssl rand -base64 32"
            );
        }
        let encryption_token = &settings.security.encryption_token;
        if encryption_token.contains("change-me") || encryption_token.len() < 16 {
            panic!(
                "refusing to start in production: ENCRYPTION_TOKEN is unset, the default, or \
                 shorter than 16 bytes. Generate one with: openssl rand -base64 32"
            );
        }
    } else {
        if settings.auth.jwt_secret.contains("change-me") {
            tracing::warn!(
                "JWT_SECRET is still the default value; set a strong random secret before deploying"
            );
        }
        if settings.security.encryption_token.contains("change-me") {
            tracing::warn!(
                "ENCRYPTION_TOKEN is still the default value; set a strong random token before deploying"
            );
        }
    }

    let db = Database::new_with_config(&settings.database)
        .await
        .expect("failed to connect to database");

    let jwt_service = JwtService::new(
        &settings.auth.jwt_secret,
        (settings.auth.jwt_expiration_secs / 60) as u64,
    );

    // Refresh tokens live for REFRESH_EXPIRY_DAYS days (default 30), matching
    // the .env.example documentation, instead of a hardcoded 12 hours.
    let refresh_expiry_days: u64 = std::env::var("REFRESH_EXPIRY_DAYS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(30);
    let session_service = SessionService::new(refresh_expiry_days.saturating_mul(24 * 60));

    let mut storage = StorageRegistry::new();

    // No buckets are ever auto-seeded — not even on a fresh install. The
    // buckets table starts empty and stays empty until an admin creates a
    // bucket through the admin UI (which registers its storage backend on
    // the fly). Here we only register backends for buckets that already
    // exist in the database, so their files stay reachable across restarts.
    match BucketRepository::list(db.pool()).await {
        Ok(existing) => {
            for b in &existing {
                let path = std::path::PathBuf::from(&b.path);
                let backend = LocalFsBackend::new(&path)
                    .unwrap_or_else(|e| panic!("failed to init storage at '{}': {e}", path.display()));
                tracing::info!("Storage [{}]: {}", b.name, b.path);
                storage.register(b.name.clone(), Arc::new(backend));
            }
        }
        Err(e) => {
            // Do NOT treat a failed list as "no buckets": that could hide
            // existing buckets and lose access to their files. Fail fast instead.
            panic!("failed to list existing buckets at startup: {e}");
        }
    }

    let state = Arc::new(AppState {
        db,
        jwt_service,
        storage: RwLock::new(storage),
        session_service,
        config: settings.clone(),
    });

    // axum's default body limit (2 MiB) would otherwise reject multipart
    // uploads larger than that before the handler's own per-field cap applies.
    let upload_limit = settings
        .storage
        .max_upload_size_mb
        .saturating_mul(1024 * 1024)
        .saturating_add(4 * 1024 * 1024) as usize;
    let api_routes =
        keystone::api_routes().layer(axum::extract::DefaultBodyLimit::max(upload_limit));

    let cors_origins: Vec<HeaderValue> = settings.cors.allowed_origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    let cors_methods: Vec<Method> = settings.cors.allowed_methods
        .iter()
        .filter_map(|m| m.parse().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(cors_origins)
        .allow_methods(cors_methods)
        .allow_credentials(settings.cors.allow_credentials);

    // Per-IP rate limiter, honoring RateLimitConfig. Applied to every route
    // (including the unauthenticated /auth/login and /auth/register), so
    // brute-force and request-flooding are throttled.
    let limiter = Arc::new(RateLimiter::new(
        settings.rate_limit.enabled,
        settings.rate_limit.requests_per_second,
        settings.rate_limit.burst_size,
        settings.rate_limit.trust_proxy_headers,
    ));

    let app = Router::new()
        .merge(api_routes)
        .fallback(ui_handler)
        .layer(axum_mw::from_fn(security_headers))
        .layer(cors)
        .layer(axum_mw::from_fn(catch_panic))
        .layer(axum_mw::from_fn(request_logging))
        .layer(axum_mw::from_fn(assign_request_id))
        .layer(axum_mw::from_fn_with_state(limiter.clone(), rate_limit))
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state.clone());

    let address = settings.server.address();
    let listener = TcpListener::bind(&address)
        .await
        .expect("failed to bind to address");

    tracing::info!("Server listening on http://{address}");

    // `into_make_service_with_connect_info` makes the TCP peer address
    // available to the rate limiter via `ConnectInfo<SocketAddr>`.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown");
}

async fn ui_handler(
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Response {
    let path = uri.path();

    match path {
        "/static/vendor/alpine.min.js" => (
            [
                (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            ALPINE_JS,
        )
            .into_response(),
        "/static/vendor/tailwind.min.js" => (
            [
                (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            TAILWIND_JS,
        )
            .into_response(),
        "/logo.svg" => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "public, max-age=86400"),
            ],
            LOGO_SVG,
        )
            .into_response(),
        "/auth/login" | "/login" => HtmlResponse(versioned(LOGIN_HTML)).into_response(),
        "/auth/register" | "/register" => {
            let blocked = AdminSettingRepository::get_bool(state.db.pool(), "block_registrations")
                .await
                .unwrap_or(true);
            if blocked {
                Redirect::to("/login").into_response()
            } else {
                HtmlResponse(versioned(REGISTER_HTML)).into_response()
            }
        }
        "/setup" => {
            // Setup is only valid before the first user exists.
            let has_users = UserRepository::count(state.db.pool())
                .await
                .map(|c| c > 0)
                .unwrap_or(false);
            if has_users {
                Redirect::to("/login").into_response()
            } else {
                HtmlResponse(versioned(SETUP_HTML)).into_response()
            }
        }
        _ => {
            let has_users = UserRepository::count(state.db.pool())
                .await
                .map(|c| c > 0)
                .unwrap_or(false);

            if !has_users {
                return Redirect::to("/setup").into_response();
            }

            match path {
                "/" | "/dashboard" => HtmlResponse(versioned(DASHBOARD_HTML)).into_response(),
                "/files" => HtmlResponse(versioned(FILES_HTML)).into_response(),
                "/account" => HtmlResponse(versioned(ACCOUNT_HTML)).into_response(),
                "/admin" => HtmlResponse(versioned(ADMIN_HTML)).into_response(),
                "/docs" => admin_page(headers, &state, DOCS_HTML).await,
                "/orphans" => admin_page(headers, &state, ORPHANS_HTML).await,
                "/bots" => HtmlResponse(versioned(BOTS_HTML)).into_response(),
                _ => (StatusCode::NOT_FOUND, "Not Found").into_response(),
            }
        }
    }
}

struct HtmlResponse(String);

/// Serve an admin-only page. The UI authenticates via the Authorization
/// header (JWT stored in localStorage), so we can only enforce this when the
/// header is present; plain browser navigation relies on the client-side
/// role guard in each page.
async fn admin_page(
    headers: axum::http::HeaderMap,
    state: &Arc<AppState>,
    html: &'static str,
) -> Response {
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let (scheme, token) = auth.split_once(' ').unwrap_or(("", auth));
        if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
            return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
        }
        match state.jwt_service.validate_token(token) {
            Ok(claims) => {
                if claims.role != "admin" {
                    return (StatusCode::FORBIDDEN, "Forbidden").into_response();
                }
            }
            Err(_) => return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response(),
        }
    }
    HtmlResponse(versioned(html)).into_response()
}

fn versioned(html: &'static str) -> String {
    html.replace("__KEYSTONE_VERSION__", KEYSTONE_VERSION)
}

impl IntoResponse for HtmlResponse {
    fn into_response(self) -> Response {
        axum::response::Html(self.0).into_response()
    }
}
