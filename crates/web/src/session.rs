//! Session cookie policy and store selection.
//!
//! Sessions are server-side: the browser holds nothing but a signed cookie
//! carrying an opaque session ID. Signing it with `SESSION_SECRET` means a
//! tampered or forged ID is rejected before the store is ever queried.

use time::Duration;
use tower_sessions::{
    Expiry, SessionManagerLayer, SessionStore as SessionStoreTrait, cookie::Key,
    service::SignedCookie,
};

use crate::config::{AppConfig, Environment};

/// Name of the session cookie.
///
/// The `__Host-` prefix would be stronger, but it requires `Secure`, which
/// cannot be set for `http://localhost`. The name is therefore plain and the
/// hardening is applied through the attributes below.
const COOKIE_NAME: &str = "mediagenerator.sid";

/// How long a session survives without activity.
const INACTIVITY_TIMEOUT: Duration = Duration::hours(8);

/// Minimum length of a cookie signing key, imposed by the `cookie` crate.
const MIN_KEY_LEN: usize = 64;

/// Builds the session layer for a given store.
///
/// Generic over the store so the binary can use PostgreSQL while tests use an
/// in-memory store, without either duplicating the cookie policy.
///
/// # Errors
///
/// Returns an error when `SESSION_SECRET` is too short to derive a signing key.
pub fn session_layer<Store>(
    config: &AppConfig,
    store: Store,
) -> Result<SessionManagerLayer<Store, SignedCookie>, SessionError>
where
    Store: SessionStoreTrait + Clone,
{
    let secret = config.session_secret.expose().as_bytes();
    if secret.len() < MIN_KEY_LEN {
        return Err(SessionError::KeyTooShort {
            needed: MIN_KEY_LEN,
            got: secret.len(),
        });
    }
    let key = Key::try_from(secret).map_err(|_| SessionError::KeyTooShort {
        needed: MIN_KEY_LEN,
        got: secret.len(),
    })?;

    Ok(SessionManagerLayer::new(store)
        .with_name(COOKIE_NAME)
        .with_path("/")
        // Not readable from JavaScript: an XSS bug must not yield the session.
        .with_http_only(true)
        // Lax, not Strict: the OIDC provider redirects the browser back to
        // /auth/callback as a top-level GET, and Strict would withhold the
        // cookie on that navigation, losing the login flow.
        .with_same_site(tower_sessions::cookie::SameSite::Lax)
        // Secure cannot be set over plain http://localhost, or the browser
        // would discard the cookie during local development.
        .with_secure(config.app_env == Environment::Production)
        .with_expiry(Expiry::OnInactivity(INACTIVITY_TIMEOUT))
        .with_signed(key))
}

/// Errors raised while building the session layer.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// `SESSION_SECRET` is shorter than the signing key requires.
    #[error("SESSION_SECRET must be at least {needed} bytes, got {got}")]
    KeyTooShort {
        /// Required length in bytes.
        needed: usize,
        /// Length that was supplied.
        got: usize,
    },
}
