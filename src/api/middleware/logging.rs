use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::time::Instant;
use tracing::info;

/// Middleware that logs every HTTP request with method, path, status code,
/// and elapsed time in milliseconds.
pub async fn request_logging(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let start = Instant::now();

    let response = next.run(req).await;

    let elapsed_ms = start.elapsed().as_millis();
    let status = response.status().as_u16();

    info!(
        method = %method,
        path = %uri,
        status = status,
        elapsed_ms = elapsed_ms as u64,
        "request handled"
    );

    response
}
