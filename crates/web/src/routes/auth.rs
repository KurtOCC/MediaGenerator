//! Sign-in and sign-out endpoints.
//!
//! * `GET  /auth/login`    — starts the Authorization Code flow with PKCE.
//! * `GET  /auth/callback` — handles the redirect back from Entra ID.
//! * `POST /auth/logout`   — clears the session and ends it at Entra ID.
//!
//! `/auth/login` and `/auth/callback` are necessarily unauthenticated.
//! `/auth/logout` is a `POST` so that it cannot be triggered by a link, an
//! image or a prefetch, and it passes through the origin check like every other
//! state-changing request.

use axum::{
    Router,
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use mediagenerator_auth::{
    AuthError, SessionUser,
    session::{current_user, id_token, put_login_flow, put_user, take_login_flow},
};
use mediagenerator_domain::i18n::nb;
use serde::Deserialize;
use tower_sessions::Session;

use crate::{error::AppError, state::AppState};

/// Where a user lands after signing in when nothing else was requested.
const DEFAULT_RETURN_TO: &str = "/";

/// Returns the authentication routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", post(logout))
}

/// Query parameters accepted by `GET /auth/login`.
#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    /// Local path to return to after signing in.
    #[serde(default)]
    return_to: Option<String>,
}

/// Starts a sign-in.
///
/// An already-signed-in user is sent straight on, so that a stale `/auth/login`
/// bookmark does not force a pointless round trip to the identity provider.
async fn login(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<LoginQuery>,
) -> Result<Response, AppError> {
    let return_to = sanitise_return_to(query.return_to.as_deref());

    if current_user(&session).await.map_err(internal)?.is_some() {
        return Ok(Redirect::to(&return_to).into_response());
    }

    let (authorization_url, flow) = state.oidc.begin_login(return_to).await.map_err(upstream)?;

    put_login_flow(&session, flow).await.map_err(internal)?;

    tracing::info!("redirecting to the identity provider");
    Ok(Redirect::to(authorization_url.as_str()).into_response())
}

/// Query parameters on the redirect back from Entra ID.
///
/// Either `code` or `error` is present; both are absent only if the request was
/// not produced by the identity provider.
#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    /// Authorization code to exchange for tokens.
    #[serde(default)]
    code: Option<String>,
    /// CSRF state, echoed back from the authorization request.
    #[serde(default)]
    state: Option<String>,
    /// Error code, when the provider refused.
    #[serde(default)]
    error: Option<String>,
    /// Human-readable error text from the provider. Logged, never displayed.
    #[serde(default)]
    error_description: Option<String>,
}

/// Completes a sign-in.
async fn callback(
    State(state): State<AppState>,
    session: Session,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    // Taken, not read: a callback may be handled at most once, so a replay
    // finds no flow.
    let flow = take_login_flow(&session).await.map_err(auth_failed)?;

    if let Some(provider_error) = query.error {
        // The description is attacker-influenceable text; it is logged for
        // support but never rendered.
        tracing::warn!(
            error = %provider_error,
            description = query.error_description.as_deref().unwrap_or(""),
            "identity provider refused the sign-in"
        );
        return Err(auth_failed(AuthError::Provider(provider_error)));
    }

    let (code, returned_state) = match (query.code, query.state) {
        (Some(code), Some(returned_state)) => (code, returned_state),
        _ => return Err(auth_failed(AuthError::StateMismatch)),
    };

    let (claims, raw_id_token) = state
        .oidc
        .complete_login(&flow, code, &returned_state)
        .await
        .map_err(auth_failed)?;

    let user = SessionUser::from_claims(&claims).map_err(auth_failed)?;

    // Enforce the app role before anything is written to the session, so a user
    // without access never ends up with a half-established login.
    if !state.role_policy.permits(&user) {
        tracing::info!(
            oid = %user.oid,
            required_role = state.role_policy.required().unwrap_or(""),
            "sign-in refused: user lacks the required app role"
        );
        return Err(AppError::Forbidden);
    }

    // A new session ID after authentication, so a session fixed by an attacker
    // before sign-in cannot be used afterwards.
    session
        .cycle_id()
        .await
        .map_err(|error| internal(AuthError::Session(error)))?;
    put_user(&session, &user, &raw_id_token)
        .await
        .map_err(internal)?;

    tracing::info!(oid = %user.oid, "user signed in");

    let destination = sanitise_return_to(Some(&flow.return_to));
    Ok(Redirect::to(&destination).into_response())
}

/// Signs the user out locally and at the identity provider.
///
/// The local session is flushed first: even if the redirect to Entra ID fails,
/// the user is signed out of this application.
async fn logout(State(state): State<AppState>, session: Session) -> Result<Response, AppError> {
    let hint = id_token(&session).await.map_err(internal)?;

    if let Some(user) = current_user(&session).await.map_err(internal)? {
        tracing::info!(oid = %user.oid, "user signed out");
    }

    session
        .flush()
        .await
        .map_err(|error| internal(AuthError::Session(error)))?;

    let url = state
        .oidc
        .logout_url(hint.as_deref())
        .await
        .map_err(upstream)?;

    Ok(Redirect::to(url.as_str()).into_response())
}

/// Restricts a return path to a local one.
///
/// Anything that is not a plain absolute path on this host is replaced by the
/// front page. Without this, `?return_to=https://evil.example` or the
/// protocol-relative `//evil.example` would turn sign-in into an open redirect.
fn sanitise_return_to(candidate: Option<&str>) -> String {
    let Some(value) = candidate else {
        return DEFAULT_RETURN_TO.to_owned();
    };

    let is_local_path = value.starts_with('/')
        && !value.starts_with("//")
        // A backslash is normalised to a forward slash by some user agents, so
        // "/\evil.example" can be read as protocol-relative.
        && !value.starts_with("/\\")
        && !value.contains(['\r', '\n'])
        && !value.chars().any(char::is_control);

    if is_local_path {
        value.to_owned()
    } else {
        tracing::debug!("discarded a non-local return_to");
        DEFAULT_RETURN_TO.to_owned()
    }
}

/// Maps a failed sign-in to a user-facing error.
///
/// Every failure mode collapses to the same message: telling the caller which
/// check failed would help an attacker probe the flow.
fn auth_failed(error: AuthError) -> AppError {
    tracing::warn!(%error, "sign-in failed");
    match error.code() {
        mediagenerator_domain::ErrorCode::Forbidden => AppError::Forbidden,
        mediagenerator_domain::ErrorCode::Internal => AppError::Internal(anyhow::Error::new(error)),
        _ => AppError::Validation(nb::ERR_LOGIN_FAILED.to_owned()),
    }
}

/// Maps an identity-provider outage to a user-facing error.
fn upstream(error: AuthError) -> AppError {
    tracing::error!(%error, "identity provider unavailable");
    AppError::Validation(nb::ERR_UPSTREAM.to_owned())
}

/// Maps an internal failure, keeping the cause in the log only.
fn internal(error: AuthError) -> AppError {
    AppError::Internal(anyhow::Error::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_paths_survive() {
        assert_eq!(sanitise_return_to(Some("/historikk")), "/historikk");
        assert_eq!(
            sanitise_return_to(Some("/historikk?side=2")),
            "/historikk?side=2"
        );
        assert_eq!(sanitise_return_to(Some("/")), "/");
    }

    #[test]
    fn absolute_and_protocol_relative_urls_are_discarded() {
        for hostile in [
            "https://evil.example",
            "//evil.example",
            "/\\evil.example",
            "http://localhost:8080/ok",
        ] {
            assert_eq!(sanitise_return_to(Some(hostile)), DEFAULT_RETURN_TO);
        }
    }

    #[test]
    fn control_characters_are_discarded() {
        assert_eq!(
            sanitise_return_to(Some("/a\r\nSet-Cookie: x=y")),
            DEFAULT_RETURN_TO
        );
        assert_eq!(sanitise_return_to(Some("/a\u{0}b")), DEFAULT_RETURN_TO);
    }

    #[test]
    fn a_missing_value_becomes_the_front_page() {
        assert_eq!(sanitise_return_to(None), DEFAULT_RETURN_TO);
        assert_eq!(sanitise_return_to(Some("")), DEFAULT_RETURN_TO);
    }
}
