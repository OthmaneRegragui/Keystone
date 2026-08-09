use axum::body::Body;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// Middleware that catches panics in downstream handlers/services and
/// converts them into a structured 500 JSON response (RFC 7807) instead
/// of crashing the server task.
pub async fn catch_panic(req: Request<Body>, next: Next) -> Response {
    let result = AssertUnwindSafe(next.run(req))
        .catch_unwind()
        .await;

    match result {
        Ok(response) => response,
        Err(panic) => {
            // The panic payload is logged server-side only; the client always
            // receives the same generic 500 JSON so no internals leak.
            if let Some(msg) = panic.downcast_ref::<&str>() {
                tracing::error!(panic = %msg, "request handler panicked");
            } else if let Some(msg) = panic.downcast_ref::<String>() {
                tracing::error!(panic = %msg, "request handler panicked");
            } else {
                tracing::error!("request handler panicked");
            }
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({
                    "type": "/errors/INTERNAL_ERROR",
                    "title": "Internal Server Error",
                    "status": 500,
                    "detail": "an unexpected error occurred",
                })),
            )
                .into_response()
        }
    }
}
