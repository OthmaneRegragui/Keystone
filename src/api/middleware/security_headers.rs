//! Middleware that sets security-related response headers on every response.

use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

// `script-src 'self' 'unsafe-inline' 'unsafe-eval'` and
// `style-src 'self' 'unsafe-inline'` are required because the bundled UI ships
// inline <script>/<style> blocks, self-hosted vendor assets, and Alpine.js,
// which compiles its `x-` directives with eval at runtime. Everything else is
// locked down.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob:; \
connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

// HSTS is only meaningful — and only honored by browsers — over HTTPS, so it
// is emitted only in production, where the deployment is expected to run
// behind TLS. The flag is set once from main.rs via
// `keystone::error::set_production`.
const HSTS: &str = "max-age=31536000; includeSubDomains";

/// Adds a standard set of security headers to every response.
pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));

    if crate::error::is_production() {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        );
    }

    response
}
