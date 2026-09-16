//! Provider errors, mapped to something the user can act on.

use mediagenerator_domain::{ErrorCode, i18n::nb};
use thiserror::Error;

/// A call to Azure AI Foundry did not produce media.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The request was malformed or the parameters were rejected (HTTP 400).
    #[error("provider rejected the request: {0}")]
    Validation(String),

    /// Authentication or authorisation failed (HTTP 401/403).
    #[error("provider refused the credentials")]
    Unauthorized,

    /// The deployment named in configuration does not exist (HTTP 404).
    #[error("deployment not found: {0}")]
    DeploymentNotFound(String),

    /// Rate limited or out of quota (HTTP 429).
    #[error("provider rate limited the request")]
    RateLimited,

    /// The content filter rejected the prompt or the result.
    ///
    /// Its own variant because it is the one failure the user can actually do
    /// something about, and it deserves its own message.
    #[error("content filter rejected the request")]
    ContentFilter,

    /// The provider failed or was unreachable (HTTP 5xx, timeout, DNS).
    #[error("provider unavailable")]
    Upstream(String),

    /// The response did not look like what the API documents.
    #[error("unexpected response from the provider: {0}")]
    Malformed(String),

    /// The asynchronous job did not finish within the allowed time.
    #[error("generation timed out")]
    Timeout,

    /// Something went wrong on our side while preparing the call.
    #[error("internal provider error: {0}")]
    Internal(String),
}

impl ProviderError {
    /// Returns the stable error code stored on the job.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) => ErrorCode::Validation,
            Self::Unauthorized | Self::DeploymentNotFound(_) | Self::Internal(_) => {
                ErrorCode::Internal
            }
            Self::RateLimited => ErrorCode::RateLimited,
            Self::ContentFilter => ErrorCode::ContentFilter,
            Self::Upstream(_) | Self::Malformed(_) | Self::Timeout => ErrorCode::Upstream,
        }
    }

    /// Returns the Norwegian message shown to the user.
    ///
    /// A misconfigured deployment and a refused credential are operator
    /// problems, not user problems, so both collapse to the generic message —
    /// the detail is in the log.
    pub const fn user_message(&self) -> &'static str {
        match self {
            Self::ContentFilter => nb::ERR_CONTENT_FILTER,
            Self::RateLimited => nb::ERR_RATE_LIMITED,
            Self::Timeout => nb::ERR_GENERATION_TIMEOUT,
            Self::Validation(_) => nb::ERR_VALIDATION,
            Self::Upstream(_) | Self::Malformed(_) => nb::ERR_UPSTREAM,
            Self::Unauthorized | Self::DeploymentNotFound(_) | Self::Internal(_) => {
                nb::ERR_INTERNAL
            }
        }
    }

    /// Returns `true` when retrying the same call might succeed.
    ///
    /// A content filter rejection or a bad parameter will fail identically
    /// every time; retrying those only wastes quota.
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::Upstream(_))
    }
}
