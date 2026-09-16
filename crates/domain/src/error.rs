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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl ErrorCode {
    /// Parses a code written by an earlier build, falling back to
    /// [`ErrorCode::Internal`].
    ///
    /// Rows outlive deployments. A code this build does not recognise is still
    /// a real failure, so it degrades to the generic one rather than erroring.
    pub fn from_str_or_internal(value: &str) -> Self {
        match value {
            "validation" => Self::Validation,
            "unauthenticated" => Self::Unauthenticated,
            "forbidden" => Self::Forbidden,
            "not_found" => Self::NotFound,
            "rate_limited" => Self::RateLimited,
            "content_filter" => Self::ContentFilter,
            "upstream" => Self::Upstream,
            _ => Self::Internal,
        }
    }

    /// Returns the Norwegian message shown for this code.
    pub const fn user_message(self) -> &'static str {
        match self {
            Self::Validation => crate::i18n::nb::ERR_VALIDATION,
            Self::Unauthenticated => crate::i18n::nb::ERR_UNAUTHENTICATED,
            Self::Forbidden => crate::i18n::nb::ERR_FORBIDDEN,
            Self::NotFound => crate::i18n::nb::ERR_NOT_FOUND,
            Self::RateLimited => crate::i18n::nb::ERR_RATE_LIMITED,
            Self::ContentFilter => crate::i18n::nb::ERR_CONTENT_FILTER,
            Self::Upstream => crate::i18n::nb::ERR_UPSTREAM,
            Self::Internal => crate::i18n::nb::ERR_INTERNAL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_round_trips_through_its_string() {
        for code in [
            ErrorCode::Validation,
            ErrorCode::Unauthenticated,
            ErrorCode::Forbidden,
            ErrorCode::NotFound,
            ErrorCode::RateLimited,
            ErrorCode::ContentFilter,
            ErrorCode::Upstream,
            ErrorCode::Internal,
        ] {
            assert_eq!(ErrorCode::from_str_or_internal(code.as_str()), code);
        }
    }

    #[test]
    fn a_code_from_a_future_build_degrades_to_internal() {
        assert_eq!(
            ErrorCode::from_str_or_internal("noe_nytt"),
            ErrorCode::Internal
        );
    }
}
