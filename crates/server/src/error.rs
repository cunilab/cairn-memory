//! One error type for every route: a status, a stable `code`, and the text the web shows.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cairn_store::StoreError;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "validation", message)
    }

    pub fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "authentication rate limit exceeded",
        )
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Validation(m) => Self::invalid(m),
            StoreError::Conflict(m) => Self::new(StatusCode::CONFLICT, "conflict", m),
            StoreError::NotFound(m) => {
                Self::new(StatusCode::NOT_FOUND, "not_found", format!("{m} not found"))
            }
            StoreError::Forbidden(m) => Self::forbidden(m),
            StoreError::LegacyDatabase | StoreError::Database(_) => {
                // The detail can name tables and values; it stays in the server log.
                eprintln!("store error: {error}");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "internal error",
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error": self.message, "code": self.code})),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;
