use axum::body::Body;
use axum::http::{header::HeaderName, Request};
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// Middleware that assigns a unique request ID to every incoming request.
/// The ID is stored in request extensions and also returned as an
/// `X-Request-ID` response header.
pub async fn assign_request_id(mut req: Request<Body>, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    req.extensions_mut().insert(request_id.clone());

    let mut response = next.run(req).await;

    if let Ok(header_val) = request_id.parse() {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), header_val);
    }

    response
}
