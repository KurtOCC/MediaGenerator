//! Route guards.
//!
//! [`require_auth`] rejects anyone without a session user; [`require_role`]
//! additionally checks an app role. Both are `axum` middleware and must sit
//! inside the session layer, which is what puts [`Session`] into the request
//! extensions.
//!
//! Rejections are content-negotiated so the same guard works for the HTML pages,
//! for HTMX partials and for the JSON API:
//!
//! * a JSON API path gets `401`/`403` with a JSON body,
//! * an HTMX request gets `204` plus `HX-Redirect`, because HTMX swaps response
//!   bodies rather than following redirects,
//! * an ordinary page navigation gets a `303` to `/auth/login`.

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use mediagenerator_domain::{ErrorCode, i18n::nb};
use serde::Serialize;
use tower_sessions::Session;

use crate::session::{SessionUser, current_user};

/// Path prefix whose failures are answered with JSON rather than a redirect.
const API_PREFIX: &str = "/api/";

/// Where an unauthenticated browser is sent.
const LOGIN_PATH: &str = "/auth/login";

/// Query parameter carrying the path to return to after sign-in.
const RETURN_TO_PARAM: &str = "return_to";

/// The role policy applied by [`require_role`].
#[derive(Debug, Clone, Default)]
pub struct RolePolicy {
    /// Role that must be present, or `None` to allow every signed-in user.
    required: Option<String>,
}

impl RolePolicy {
    /// Builds a policy requiring `role`.
    ///
    /// A blank or missing role disables the check, which is how
    /// `REQUIRED_APP_ROLE=` turns the guard off without a code change.
    pub fn new(role: Option<&str>) -> Self {
        Self {
            required: role
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .map(ToOwned::to_owned),
        }
    }

    /// Returns the required role, if the check is enabled.
    pub fn required(&self) -> Option<&str> {
        self.required.as_deref()
    }

    /// Returns `true` when `user` satisfies this policy.
    pub fn permits(&self, user: &SessionUser) -> bool {
        self.required
            .as_deref()
            .is_none_or(|role| user.has_role(role))
    }
}

/// Rejects requests that carry no authenticated session.
///
/// On success the [`SessionUser`] is inserted into the request extensions, so
/// downstream handlers can take it with `Extension<SessionUser>` without
/// touching the session store again.
pub async fn require_auth(mut request: Request, next: Next) -> Response {
    let Some(session) = request.extensions().get::<Session>().cloned() else {
        // The session layer is missing from the stack. That is a wiring bug,
        // not a client error, so it must not look like a failed sign-in.
        tracing::error!("require_auth ran without a session layer");
        return internal_error();
    };

    let user = match current_user(&session).await {
        Ok(Some(user)) => user,
        Ok(None) => return unauthenticated(request.uri(), request.headers()),
        Err(error) => {
            tracing::error!(%error, "could not read the session");
            return internal_error();
        }
    };

    request.extensions_mut().insert(user);
    next.run(request).await
}

/// Rejects requests whose user does not hold the configured app role.
///
/// Runs after [`require_auth`], which is what puts the user in the extensions.
/// Wire it up with `axum::middleware::from_fn_with_state(policy, require_role)`.
pub async fn require_role(
    State(policy): State<RolePolicy>,
    request: Request,
    next: Next,
) -> Response {
    let Some(required) = policy.required() else {
        return next.run(request).await;
    };

    let Some(user) = request.extensions().get::<SessionUser>() else {
        tracing::error!("require_role ran before require_auth");
        return internal_error();
    };

    if !user.has_role(required) {
        tracing::info!(
            oid = %user.oid,
            required_role = %required,
            "access denied: user lacks the required app role"
        );
        return forbidden(request.uri(), request.headers());
    }

    next.run(request).await
}

/// JSON body for a rejected API request.
#[derive(Debug, Serialize)]
struct RejectionBody {
    code: &'static str,
    message: &'static str,
}

/// Returns `true` when the request targets the JSON API.
fn wants_json(uri: &Uri) -> bool {
    uri.path().starts_with(API_PREFIX)
}

/// Returns `true` when the request was issued by HTMX.
fn is_htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// Builds the sign-in URL, preserving where the user was heading.
///
/// Only the path and query are kept, and they are percent-encoded into a query
/// parameter, so this cannot be turned into an open redirect to another host.
fn login_url(uri: &Uri) -> String {
    let target = uri.path_and_query().map_or("/", |pq| pq.as_str());
    if target == "/" {
        return LOGIN_PATH.to_owned();
    }
    let encoded = percent_encode(target);
    format!("{LOGIN_PATH}?{RETURN_TO_PARAM}={encoded}")
}

/// Percent-encodes a string for use as a query parameter value.
fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Builds the response for a request with no authenticated session.
fn unauthenticated(uri: &Uri, headers: &HeaderMap) -> Response {
    if wants_json(uri) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(RejectionBody {
                code: ErrorCode::Unauthenticated.as_str(),
                message: nb::ERR_UNAUTHENTICATED,
            }),
        )
            .into_response();
    }

    let destination = login_url(uri);

    if is_htmx(headers) {
        // HTMX swaps the body of a 3xx response instead of navigating, so a
        // redirect has to be expressed as a header it understands.
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Ok(value) = destination.parse() {
            response.headers_mut().insert("hx-redirect", value);
        }
        return response;
    }

    Redirect::to(&destination).into_response()
}

/// Builds the response for an authenticated user who lacks the required role.
fn forbidden(uri: &Uri, headers: &HeaderMap) -> Response {
    if wants_json(uri) || is_htmx(headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(RejectionBody {
                code: ErrorCode::Forbidden.as_str(),
                message: nb::ERR_FORBIDDEN,
            }),
        )
            .into_response();
    }

    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        nb::ERR_FORBIDDEN,
    )
        .into_response()
}

/// Builds a generic 500 that leaks nothing about what went wrong.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(RejectionBody {
            code: ErrorCode::Internal.as_str(),
            message: nb::ERR_INTERNAL,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn user(roles: &[&str]) -> SessionUser {
        SessionUser {
            oid: "oid".to_owned(),
            email: None,
            upn: None,
            display_name: "Hans Kristiansen".to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            groups: Vec::new(),
            authenticated_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn a_blank_role_disables_the_check() {
        for blank in [None, Some(""), Some("   ")] {
            let policy = RolePolicy::new(blank);
            assert_eq!(policy.required(), None);
            assert!(policy.permits(&user(&[])));
        }
    }

    #[test]
    fn a_configured_role_is_enforced() {
        let policy = RolePolicy::new(Some("Mediagenerator.User"));
        assert!(policy.permits(&user(&["Mediagenerator.User"])));
        assert!(!policy.permits(&user(&["Something.Else"])));
        assert!(!policy.permits(&user(&[])));
    }

    #[test]
    fn login_url_keeps_the_path_and_query() {
        let uri: Uri = "/historikk?side=2".parse().expect("valid uri");
        assert_eq!(
            login_url(&uri),
            "/auth/login?return_to=%2Fhistorikk%3Fside%3D2"
        );
    }

    #[test]
    fn login_url_omits_return_to_for_the_front_page() {
        let uri: Uri = "/".parse().expect("valid uri");
        assert_eq!(login_url(&uri), "/auth/login");
    }

    #[test]
    fn api_paths_are_answered_with_json() {
        assert!(wants_json(
            &"/api/jobs/1".parse::<Uri>().expect("valid uri")
        ));
        assert!(!wants_json(
            &"/historikk".parse::<Uri>().expect("valid uri")
        ));
        // A path that merely starts with the same letters must not match.
        assert!(!wants_json(&"/apifoo".parse::<Uri>().expect("valid uri")));
    }
}
