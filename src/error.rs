use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::OnceLock;

/// Whether the server runs in production mode. Set once from `main.rs` via
/// [`set_production`]; defaults to `false` so unit tests (which never set it)
/// keep the existing detailed error behavior.
static PRODUCTION: OnceLock<bool> = OnceLock::new();

/// Record whether the server runs in production. Called exactly once at
/// startup; later calls are ignored.
pub fn set_production(value: bool) {
    let _ = PRODUCTION.set(value);
}

/// Returns `true` when the server runs in production mode.
pub fn is_production() -> bool {
    *PRODUCTION.get().unwrap_or(&false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    NotFound,
    Unauthorized,
    Forbidden,
    BadRequest,
    Conflict,
    InternalError,
    StorageError,
    ValidationFailed,
    FileAlreadyExists,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotFound => "NOT_FOUND",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::Forbidden => "FORBIDDEN",
            Self::BadRequest => "BAD_REQUEST",
            Self::Conflict => "CONFLICT",
            Self::InternalError => "INTERNAL_ERROR",
            Self::StorageError => "STORAGE_ERROR",
            Self::ValidationFailed => "VALIDATION_FAILED",
            Self::FileAlreadyExists => "FILE_ALREADY_EXISTS",
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Conflict => StatusCode::CONFLICT,
            Self::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::StorageError => StatusCode::INTERNAL_SERVER_ERROR,
            Self::ValidationFailed => StatusCode::UNPROCESSABLE_ENTITY,
            Self::FileAlreadyExists => StatusCode::CONFLICT,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("file already exists: {0}")]
    FileAlreadyExists(String),
}

impl AppError {
    pub fn error_code(&self) -> ErrorCode {
        match self {
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::Unauthorized(_) => ErrorCode::Unauthorized,
            Self::Forbidden(_) => ErrorCode::Forbidden,
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::Internal(_) => ErrorCode::InternalError,
            Self::Storage(_) => ErrorCode::StorageError,
            Self::Validation(_) => ErrorCode::ValidationFailed,
            Self::FileAlreadyExists(_) => ErrorCode::FileAlreadyExists,
        }
    }

    pub fn status_code(&self) -> StatusCode {
        self.error_code().status_code()
    }

    fn detail(&self) -> String {
        // `Internal`/`Storage` messages can embed database errors, filesystem
        // paths, and other implementation details. In production those must
        // never reach the client; the real detail is still logged server-side
        // so operators can diagnose without exposing internals.
        if is_production() {
            match self {
                Self::Internal(msg) | Self::Storage(msg) => {
                    tracing::error!(
                        error_code = %self.error_code().as_str(),
                        detail = %msg,
                        "error detail suppressed for client"
                    );
                    return "an internal error occurred".to_string();
                }
                _ => {}
            }
        }
        self.to_string()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub detail: String,
    #[serde(rename = "instance", skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ProblemDetails {
    pub fn from_error(error: &AppError) -> Self {
        let code = error.error_code();
        let status = code.status_code();

        Self {
            problem_type: format!("/errors/{}", code.as_str()),
            title: humanize_error_code(code).to_string(),
            status: Some(status.as_u16()),
            detail: error.detail(),
            instance: None,
        }
    }
}

fn humanize_error_code(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NotFound => "Resource Not Found",
        ErrorCode::Unauthorized => "Authentication Required",
        ErrorCode::Forbidden => "Access Denied",
        ErrorCode::BadRequest => "Invalid Request",
        ErrorCode::Conflict => "Resource Conflict",
        ErrorCode::InternalError => "Internal Server Error",
        ErrorCode::StorageError => "Storage Error",
        ErrorCode::ValidationFailed => "Validation Failed",
        ErrorCode::FileAlreadyExists => "File Already Exists",
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let problem = ProblemDetails::from_error(&self);
        let body = json!(problem);

        (status, axum::Json(body)).into_response()
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!("I/O error: {err}");
        Self::Internal("an internal I/O error occurred".to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        tracing::error!("JSON error: {err}");
        Self::Internal("invalid data format".to_string())
    }
}

impl From<axum::extract::multipart::MultipartError> for AppError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        Self::BadRequest(err.to_string())
    }
}

impl From<axum::extract::multipart::MultipartRejection> for AppError {
    fn from(err: axum::extract::multipart::MultipartRejection) -> Self {
        Self::BadRequest(err.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes_match_status() {
        let cases: Vec<(AppError, StatusCode)> = vec![
            (AppError::NotFound("test".into()), StatusCode::NOT_FOUND),
            (AppError::Unauthorized("test".into()), StatusCode::UNAUTHORIZED),
            (AppError::Forbidden("test".into()), StatusCode::FORBIDDEN),
            (AppError::BadRequest("test".into()), StatusCode::BAD_REQUEST),
            (AppError::Conflict("test".into()), StatusCode::CONFLICT),
            (AppError::Internal("test".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Storage("test".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AppError::Validation("test".into()), StatusCode::UNPROCESSABLE_ENTITY),
            (AppError::FileAlreadyExists("test".into()), StatusCode::CONFLICT),
        ];

        for (error, expected_status) in cases {
            assert_eq!(error.status_code(), expected_status);
        }
    }

    #[test]
    fn test_error_code_as_str() {
        assert_eq!(ErrorCode::NotFound.as_str(), "NOT_FOUND");
        assert_eq!(ErrorCode::InternalError.as_str(), "INTERNAL_ERROR");
        assert_eq!(ErrorCode::StorageError.as_str(), "STORAGE_ERROR");
        assert_eq!(ErrorCode::ValidationFailed.as_str(), "VALIDATION_FAILED");
        assert_eq!(ErrorCode::FileAlreadyExists.as_str(), "FILE_ALREADY_EXISTS");
    }

    #[test]
    fn test_problem_details_from_error() {
        let error = AppError::NotFound("file not found".to_string());
        let problem = ProblemDetails::from_error(&error);

        assert_eq!(problem.status, Some(404));
        assert_eq!(problem.detail, "resource not found: file not found");
        assert!(problem.problem_type.contains("NOT_FOUND"));
        assert_eq!(problem.title, "Resource Not Found");
        assert!(problem.instance.is_none());
    }

    #[test]
    fn test_problem_details_serialization() {
        let error = AppError::Validation("email is required".to_string());
        let problem = ProblemDetails::from_error(&error);
        let json = serde_json::to_value(&problem).unwrap();

        assert_eq!(json["type"], "/errors/VALIDATION_FAILED");
        assert_eq!(json["status"], 422);
        assert_eq!(json["detail"], "validation failed: email is required");
        assert_eq!(json["title"], "Validation Failed");
    }

    #[test]
    fn test_file_already_exists_problem_details() {
        let error = AppError::FileAlreadyExists("a file named 'dup.txt' already exists".to_string());
        let problem = ProblemDetails::from_error(&error);
        let json = serde_json::to_value(&problem).unwrap();

        assert_eq!(json["type"], "/errors/FILE_ALREADY_EXISTS");
        assert_eq!(json["status"], 409);
        assert_eq!(
            json["detail"],
            "file already exists: a file named 'dup.txt' already exists"
        );
        assert_eq!(json["title"], "File Already Exists");
    }

    #[test]
    fn test_into_response_status() {
        let error = AppError::BadRequest("invalid input".to_string());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_into_response_body_json() {
        let error = AppError::Unauthorized("missing token".to_string());
        let response = error.into_response();
        let status = response.status();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn test_app_error_display() {
        assert_eq!(
            AppError::NotFound("x".into()).to_string(),
            "resource not found: x"
        );
        assert_eq!(
            AppError::Internal("boom".into()).to_string(),
            "internal error: boom"
        );
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let app_err: AppError = io_err.into();
        assert_eq!(app_err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let app_err: AppError = json_err.into();
        assert_eq!(app_err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
