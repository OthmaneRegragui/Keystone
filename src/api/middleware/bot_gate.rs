//! Bot-only and user-only API gates.
//!
//! Bots get a dedicated endpoint namespace under `/api/bot/*` that accepts only
//! bot API keys. Conversely the regular user-facing endpoints (`/api/buckets`,
//! `/api/files/*`, `/api/folders/*`, `/api/admin/*`, …) must reject bot keys so
//! a bot can never reach anything but buckets and file/folder operations.
//! The two middlewares below enforce exactly that boundary.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::api::extractors::AuthUser;
use crate::error::AppError;
use crate::AppState;

/// Extract the authenticated user from the request, reusing the request's own
/// `Arc<AppState>` extension (installed by the server before routing).
async fn extract_auth(parts: &mut axum::http::request::Parts) -> Result<AuthUser, AppError> {
    let state = parts
        .extensions
        .get::<Arc<AppState>>()
        .cloned()
        .ok_or_else(|| AppError::Internal("AppState missing from request extensions".into()))?;
    AuthUser::from_request_parts(parts, &state).await
}

/// Gate for the `/api/bot/*` namespace: only bot API keys may pass. Regular
/// users, JWT sessions and ordinary API keys are rejected with 403, and
/// requests with missing/invalid credentials get the appropriate 401/403
/// error response directly.
pub async fn bot_only(req: Request<Body>, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let auth = extract_auth(&mut parts).await;
    let req = Request::from_parts(parts, body);
    match auth {
        Ok(a) if a.is_bot() => next.run(req).await,
        Ok(_) => AppError::Forbidden(
            "this endpoint is reserved for bot API keys; use the /api/bot endpoints".into(),
        )
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// Gate for the regular user-facing endpoints: bots are rejected with 403 and
/// directed to `/api/bot/*`. Unauthenticated or non-bot requests pass through
/// so the handler can enforce its own auth/authorization rules.
pub async fn reject_bots(req: Request<Body>, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let auth = extract_auth(&mut parts).await;
    let req = Request::from_parts(parts, body);
    match auth {
        Ok(a) if a.is_bot() => {
            AppError::Forbidden("bots must use the /api/bot endpoints for file operations".into())
                .into_response()
        }
        _ => next.run(req).await,
    }
}
