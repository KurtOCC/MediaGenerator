//! Errors produced by the authentication layer.

use mediagenerator_domain::{ErrorCode, i18n::nb};
use thiserror::Error;

/// Something went wrong while signing a user in or out.
#[derive(Debug, Error)]
pub enum AuthError {
    /// OpenID Connect discovery or JWKS retrieval failed.
    #[error("OpenID Connect discovery failed")]
    Discovery(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The identity provider returned an error on the redirect back to us.
    ///
    /// The payload is the provider's `error` parameter, never its
    /// `error_description`, which may echo attacker-controlled text.
    #[error("identity provider returned error: {0}")]
    Provider(String),

    /// The `state` parameter was missing, or did not match the session.
    ///
    /// Almost always a stale or replayed callback, but it is also what a CSRF
    /// attempt against the callback would look like.
    #[error("state parameter did not match the session")]
    StateMismatch,

    /// The callback arrived without a login flow in the session.
    #[error("no login flow in progress for this session")]
    NoFlowInProgress,

    /// The login flow in the session is older than the permitted window.
    #[error("login flow expired")]
    FlowExpired,

    /// Exchanging the authorization code for tokens failed.
    #[error("authorization code exchange failed")]
    TokenExchange(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The token response contained no ID token.
    #[error("identity provider returned no ID token")]
    MissingIdToken,

    /// The ID token failed verification: signature, issuer, audience, nonce or expiry.
    #[error("ID token verification failed")]
    InvalidIdToken(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The access token hash in the ID token did not match the access token.
    #[error("access token hash mismatch")]
    AccessTokenMismatch,

    /// The ID token verified, but carried no usable user identifier.
    #[error("ID token carried no object id or subject")]
    MissingIdentity,

    /// The user is authenticated but lacks the required app role.
    #[error("user lacks required app role")]
    MissingRole,

    /// Reading or writing the session store failed.
    #[error("session store error")]
    Session(#[from] tower_sessions::session::Error),

    /// A URL from configuration or discovery could not be parsed.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

impl AuthError {
    /// Returns the stable error code for this error.
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::MissingRole => ErrorCode::Forbidden,
            Self::Discovery(_) | Self::TokenExchange(_) => ErrorCode::Upstream,
            Self::Session(_) | Self::InvalidUrl(_) => ErrorCode::Internal,
            _ => ErrorCode::Unauthenticated,
        }
    }

    /// Returns the Norwegian message to show the user.
    ///
    /// Deliberately coarse: a failed sign-in must not tell the caller which
    /// specific check failed.
    pub const fn user_message(&self) -> &'static str {
        match self {
            Self::MissingRole => nb::ERR_FORBIDDEN,
            Self::Discovery(_) | Self::TokenExchange(_) => nb::ERR_UPSTREAM,
            Self::Session(_) | Self::InvalidUrl(_) => nb::ERR_INTERNAL,
            _ => nb::ERR_LOGIN_FAILED,
        }
    }

    /// Returns `true` when retrying the sign-in is likely to help.
    ///
    /// A stale callback or an expired flow is fixed by starting over; a missing
    /// role is not.
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::StateMismatch | Self::NoFlowInProgress | Self::FlowExpired
        )
    }
}
