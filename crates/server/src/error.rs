//! One error type for every route: a status, a stable `code`, and the text the web shows.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cairn_store::StoreError;
use serde::Serialize;

/// The stable `code` of an error body; the web switches on it, never on the text.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    Forbidden,
    Validation,
    SessionRequired,
    InvalidCredentials,
    Quality,
    Conflict,
    NotFound,
    Internal,
}

/// The JSON of every error response.
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[derive(Serialize)]
pub struct ErrorBody {
    pub error: String,
    pub code: ErrorCode,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: ErrorCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, ErrorCode::Validation, message)
    }

    /// The console's cue to send the person to sign in: a missing, unknown, expired or revoked
    /// web session, or a device token on a console route. The web switches on this code.
    pub fn session_required(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::SessionRequired,
            message,
        )
    }

    /// A failed sign-in or a wrong current password; one text for every cause.
    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::InvalidCredentials,
            "invalid account credentials",
        )
    }
}

impl From<StoreError> for ApiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Validation(m) => Self::invalid(m),
            StoreError::Quality(m) => Self::new(StatusCode::BAD_REQUEST, ErrorCode::Quality, m),
            StoreError::Conflict(m) => Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, m),
            StoreError::NotFound(m) => Self::new(
                StatusCode::NOT_FOUND,
                ErrorCode::NotFound,
                format!("{m} not found"),
            ),
            StoreError::Forbidden(m) => Self::forbidden(m),
            StoreError::LegacyDatabase | StoreError::Database(_) => {
                // The detail can name tables and values; it stays in the server log.
                eprintln!("store error: {error}");
                Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorCode::Internal,
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
            Json(ErrorBody {
                error: self.message,
                code: self.code,
            }),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;
