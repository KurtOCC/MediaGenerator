//! Domain-level error type shared across the workspace.
//!
//! Every variant carries a stable, machine-readable [`ErrorCode`] that is
//! persisted with a job and a Norwegian, user-facing message resolved through
//! [`crate::i18n`]. Keeping the code and the text apart means the API can stay
//! stable while the wording is translated or reworded.

use std::fmt;

use thiserror::Error;

/// Stable, machine-readable error identifiers.
///
/// These strings are written to the database (`jobs.error_code`) and returned in
/// JSON payloads, so they must not be renamed without a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The submitted input failed validation.
    Validation,
    /// The caller is not authenticated.
    Unauthenticated,
    /// The caller is authenticated but lacks the required role.
    Forbidden,
    /// The requested entity does not exist, or is not visible to the caller.
    NotFound,
    /// The caller exceeded a rate limit or quota.
    RateLimited,
    /// The upstream provider rejected the prompt through its content filter.
    ContentFilter,
    /// An upstream dependency failed or timed out.
    Upstream,
    /// An unexpected internal failure.
    Internal,
}

impl ErrorCode {
    /// Returns the stable string representation used in storage and APIs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
            Self::ContentFilter => "content_filter",
            Self::Upstream => "upstream",
            Self::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors produced by domain logic (validation, state transitions, invariants).
#[derive(Debug, Error)]
pub enum DomainError {
    /// Input failed validation; the payload is a Norwegian message for the user.
    #[error("{0}")]
    Validation(String),

    /// The entity was not found.
    #[error("{0}")]
    NotFound(String),
}

impl DomainError {
    /// Returns the stable error code for this error.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::NotFound(_) => ErrorCode::NotFound,
        }
    }
}
