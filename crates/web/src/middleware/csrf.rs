//! Cross-site request forgery protection for state-changing requests.
//!
//! Three independent defences are in play:
//!
//! * `SameSite=Lax` on the session cookie, which keeps the browser from
//!   attaching it to a cross-site `POST` at all,
//! * [`verify_origin`], which rejects any state-changing request whose `Origin`
//!   (or, failing that, `Referer`) is not this application, and
//! * [`verify_token`], a double-submit token checked on `/api/*`.
//!
//! The origin check is the one that still holds if a browser ever relaxes its
//! `SameSite` handling; the token is the one that holds if a reverse proxy ever
//! strips `Origin`. The sign-out form is an ordinary browser `POST` and relies
//! on the first two.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use mediagenerator_domain::{ErrorCode, i18n::nb};
use serde::Serialize;

/// Session key holding the per-session CSRF token.
const TOKEN_KEY: &str = "csrf.token";

/// Header HTMX sends the token in.
const TOKEN_HEADER: &str = "x-csrf-token";

/// Returns the CSRF token for this session, creating one if needed.
///
/// The token is rendered into the page and sent back on every HTMX request. A
/// cross-site page cannot read it, so it cannot forge the header — the
/// double-submit half of the protection, on top of the origin check below.
///
/// # Errors
///
/// Returns an error when the session store cannot be read or written.
pub async fn token(
    session: &tower_sessions::Session,
) -> Result<String, tower_sessions::session::Error> {
    if let Some(existing) = session.get::<String>(TOKEN_KEY).await? {
        return Ok(existing);
    }
    let fresh = uuid::Uuid::new_v4().simple().to_string();
    session.insert(TOKEN_KEY, &fresh).await?;
    Ok(fresh)
}

/// Rejects state-changing API requests whose token is missing or wrong.
///
/// Applied to `/api/*` only: those are all issued by HTMX, which attaches the
/// header. The sign-out form is an ordinary browser POST and is covered by
/// [`verify_origin`] together with the `SameSite=Lax` cookie.
pub async fn verify_token(request: Request, next: Next) -> Response {
    if is_safe(request.method()) {
        return next.run(request).await;
    }

    let Some(session) = request
        .extensions()
        .get::<tower_sessions::Session>()
        .cloned()
    else {
        tracing::error!("verify_token ran without a session layer");
        return reject();
    };

    let expected = match session.get::<String>(TOKEN_KEY).await {
        Ok(Some(expected)) => expected,
        Ok(None) => {
            tracing::warn!("state-changing request against a session with no CSRF token");
            return reject();
        }
        Err(error) => {
            tracing::error!(%error, "could not read the CSRF token from the session");
            return reject();
        }
    };

    let presented = request
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
        next.run(request).await
    } else {
        tracing::warn!(
            path = %request.uri().path(),
            "rejected a state-changing request with a missing or wrong CSRF token"
        );
        reject()
    }
}

/// Compares two byte strings without leaking their contents through timing.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// The origin every state-changing request must come from.
#[derive(Debug, Clone)]
pub struct Origin(String);

impl Origin {
    /// Derives the expected origin from the public base URL.
    ///
    /// `APP_BASE_URL` is already validated to be absolute and without a
    /// trailing slash, so it is its own origin.
    pub fn from_base_url(base_url: &str) -> Self {
        Self(base_url.to_owned())
    }

    /// Returns the expected origin as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// JSON body returned when the origin check fails.
#[derive(Debug, Serialize)]
struct RejectionBody {
    code: &'static str,
    message: &'static str,
}

/// Rejects state-changing requests that do not originate from this application.
///
/// Safe methods (`GET`, `HEAD`, `OPTIONS`, `TRACE`) pass through untouched.
pub async fn verify_origin(
    State(expected): State<Origin>,
    request: Request,
    next: Next,
) -> Response {
    if is_safe(request.method()) {
        return next.run(request).await;
    }

    match claimed_origin(request.headers()) {
        Some(origin) if origin == expected.as_str() => next.run(request).await,
        other => {
            tracing::warn!(
                method = %request.method(),
                path = %request.uri().path(),
                claimed_origin = other.as_deref().unwrap_or("<none>"),
                expected_origin = expected.as_str(),
                "rejected a state-changing request with a foreign origin"
            );
            reject()
        }
    }
}

/// Returns `true` for methods that do not change state.
fn is_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    )
}

/// Returns the origin the request claims to come from.
///
/// `Origin` is preferred; `Referer` is the fallback for the rare user agent
/// that omits `Origin` on same-origin form posts, and only its scheme, host and
/// port are considered.
fn claimed_origin(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        && origin != "null"
    {
        return Some(origin.to_owned());
    }

    let referer = headers
        .get(header::REFERER)
        .and_then(|value| value.to_str().ok())?;
    origin_of(referer)
}

/// Extracts `scheme://host[:port]` from an absolute URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// Builds the 403 returned when the origin check fails.
fn reject() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(RejectionBody {
            code: ErrorCode::Validation.as_str(),
            message: nb::ERR_CSRF,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_methods_are_recognised() {
        assert!(is_safe(&Method::GET));
        assert!(is_safe(&Method::HEAD));
        assert!(!is_safe(&Method::POST));
        assert!(!is_safe(&Method::DELETE));
    }

    #[test]
    fn origin_is_extracted_from_a_referer() {
        assert_eq!(
            origin_of("https://media.oslofjord.com/historikk?side=2"),
            Some("https://media.oslofjord.com".to_owned())
        );
        assert_eq!(
            origin_of("http://localhost:8080/"),
            Some("http://localhost:8080".to_owned())
        );
        assert_eq!(origin_of("not-a-url"), None);
        assert_eq!(origin_of("https://"), None);
    }

    #[test]
    fn the_origin_header_wins_over_the_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "http://localhost:8080".parse().expect("hv"));
        headers.insert(
            header::REFERER,
            "https://evil.example/x".parse().expect("hv"),
        );
        assert_eq!(
            claimed_origin(&headers),
            Some("http://localhost:8080".to_owned())
        );
    }

    #[test]
    fn a_null_origin_falls_through_to_the_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "null".parse().expect("hv"));
        headers.insert(
            header::REFERER,
            "http://localhost:8080/".parse().expect("hv"),
        );
        assert_eq!(
            claimed_origin(&headers),
            Some("http://localhost:8080".to_owned())
        );
    }

    #[test]
    fn a_request_without_either_header_has_no_origin() {
        assert_eq!(claimed_origin(&HeaderMap::new()), None);
    }
}
