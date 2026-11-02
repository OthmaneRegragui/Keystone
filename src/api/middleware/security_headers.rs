//! Middleware that sets security-related response headers on every response.

use axum::body::Body;
use axum::http::{header, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;

// `script-src 'self' 'unsafe-inline' 'unsafe-eval'` and
// `style-src 'self' 'unsafe-inline'` are required because the bundled UI ships
// inline <script>/<style> blocks, self-hosted vendor assets, and Alpine.js,
// which compiles its `x-` directives with eval at runtime. Removing the eval
// allowance requires switching to Alpine's CSP build and pre-registering every
// component; until then this is a deliberate, documented gap.
//
// `https://challenges.cloudflare.com` (Cloudflare Turnstile, the optional login
// CAPTCHA) is the one external origin allowed, and only in the three directives
// the widget actually needs: its script, the iframe it renders in, and the XHR
// it uses to fetch and validate challenges. It is inert when no captcha is
// configured, since nothing on the page then references it.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self' 'unsafe-inline' 'unsafe-eval' \
    https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
    media-src 'self' blob:; connect-src 'self' https://challenges.cloudflare.com; \
    frame-src https://challenges.cloudflare.com; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; \
    form-action 'self'";

// HSTS is only meaningful — and only honored by browsers — over HTTPS, so it
// is emitted only in production, where the deployment is expected to run
// behind TLS. The flag is set once from main.rs via
// `keystone::error::set_production`.
const HSTS: &str = "max-age=31536000; includeSubDomains";

/// Adds a standard set of security headers to every response.
pub async fn security_headers(req: Request<Body>, next: Next) -> Response {
    let mut response = next.run(req).await;
    let headers = response.headers_mut();

    // Only set the app-wide CSP when the handler did not already provide one.
    // Handlers that serve user content inline (e.g. the sandboxed policy for
    // raw files) install a stricter policy that must not be overwritten.
    if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        );
    }
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
