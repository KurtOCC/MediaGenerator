//! The application-wide error type returned from request handlers.
//!
//! [`AppError`] keeps two things apart: what the user is told (a Norwegian
//! message from [`mediagenerator_domain::i18n`]) and what is logged (the full
//! technical cause). Internal details never reach the response body.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mediagenerator_domain::{DomainError, ErrorCode, i18n::nb};
use serde::Serialize;

/// An error that can be returned from any handler.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Input failed validation. The payload is shown to the user verbatim.
    #[error("validation failed: {0}")]
    Validation(String),

    /// No valid session; the user must sign in.
    #[error("unauthenticated")]
    Unauthenticated,

    /// Signed in, but lacking the required app role.
    #[error("forbidden")]
    Forbidden,

    /// The entity does not exist or is not owned by the caller.
    #[error("not found")]
    NotFound,

    /// A per-user or global rate limit was exceeded.
    #[error("rate limited")]
    RateLimited,

    /// The CSRF token was missing or did not match.
    #[error("csrf check failed")]
    Csrf,

    /// Anything unexpected. The inner error is logged, never shown.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// Returns the stable error code for this error.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::Unauthenticated => ErrorCode::Unauthenticated,
            Self::Forbidden => ErrorCode::Forbidden,
            Self::NotFound => ErrorCode::NotFound,
            Self::RateLimited => ErrorCode::RateLimited,
            Self::Csrf => ErrorCode::Validation,
            Self::Internal(_) => ErrorCode::Internal,
        }
    }

    /// Returns the HTTP status code this error maps to.
    pub const fn status(&self) -> StatusCode {
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Csrf => StatusCode::FORBIDDEN,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns the Norwegian message shown to the user.
    pub fn user_message(&self) -> &str {
        match self {
            Self::Validation(message) => message,
            Self::Unauthenticated => nb::ERR_UNAUTHENTICATED,
            Self::Forbidden => nb::ERR_FORBIDDEN,
            Self::NotFound => nb::ERR_NOT_FOUND,
            Self::RateLimited => nb::ERR_RATE_LIMITED,
            Self::Csrf => nb::ERR_CSRF,
            Self::Internal(_) => nb::ERR_INTERNAL,
        }
    }
}

impl From<DomainError> for AppError {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::Validation(message) => Self::Validation(message),
            DomainError::NotFound(_) => Self::NotFound,
        }
    }
}

/// The JSON body returned for a failed request.
#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    /// Stable, machine-readable error code.
    code: &'a str,
    /// Norwegian message intended for display.
    message: &'a str,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Log the technical cause exactly once, at the boundary. Only 5xx is
        // an operational problem; client errors stay at debug level.
        if matches!(self, Self::Internal(_)) {
            tracing::error!(error = ?self, code = %self.code(), "request failed");
        } else {
            tracing::debug!(error = %self, code = %self.code(), "request rejected");
        }

        let body = ErrorBody {
            code: self.code().as_str(),
            message: self.user_message(),
        };
        (self.status(), Json(body)).into_response()
    }
}

/// Convenience alias for handler results.
pub type AppResult<T> = Result<T, AppError>;
