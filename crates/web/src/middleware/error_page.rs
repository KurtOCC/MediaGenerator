//! Turning error responses into a readable page.
//!
//! [`AppError`](crate::error::AppError) answers in JSON, which is right for the
//! API and for HTMX but wrong for someone who typed a URL: a browser then shows
//! raw JSON on a blank page. This middleware re-renders those responses as HTML
//! when, and only when, the request looks like a page navigation.
//!
//! Doing it here rather than inside `IntoResponse` is deliberate: the choice
//! depends on the request (its path and its `Accept` header), and `into_response`
//! has no access to that.

use askama::Template;
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware::Next,
    response::Response,
};
use mediagenerator_domain::{ErrorCode, i18n::nb};

use crate::middleware::correlation::CorrelationId;

/// Largest error body that will be re-read. Ours are a couple of hundred bytes.
const MAX_BODY: usize = 8 * 1024;

/// Paths that always answer in JSON, whatever the browser asks for.
const API_PREFIX: &str = "/api/";

/// The rendered error page.
#[derive(Template)]
#[template(path = "feil.html")]
struct ErrorPageTemplate {
    /// Short headline, chosen from the error code.
    heading: &'static str,
    /// The Norwegian message from the original response.
    message: String,
    /// Whether to offer a fresh sign-in.
    show_retry: bool,
    /// Correlation id, so a support request can name this exact failure.
    correlation_id: String,
}

/// The JSON body every `AppError` produces.
#[derive(serde::Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

/// Re-renders JSON error responses as HTML for page navigations.
pub async fn render_html_errors(request: Request, next: Next) -> Response {
    let render_as_html = is_page_navigation(request.uri(), request.headers());
    let correlation_id = request
        .extensions()
        .get::<CorrelationId>()
        .map(|id| id.as_str().to_owned())
        .unwrap_or_default();

    let response = next.run(request).await;

    if !render_as_html || !is_json_error(&response) {
        return response;
    }

    let (parts, body) = response.into_parts();
    let status = parts.status;

    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        // The body could not be read back. Returning the original response is
        // not an option — it has already been consumed — so fall back to a page
        // built from the status alone.
        return page(
            status,
            ErrorCode::Internal,
            nb::ERR_INTERNAL.to_owned(),
            correlation_id,
        );
    };

    let Ok(parsed) = serde_json::from_slice::<ErrorBody>(&bytes) else {
        // Not one of ours: hand it back untouched rather than guessing.
        return Response::from_parts(parts, Body::from(bytes));
    };

    page(
        status,
        ErrorCode::from_str_or_internal(&parsed.code),
        parsed.message,
        correlation_id,
    )
}

/// Builds the HTML error response.
fn page(status: StatusCode, code: ErrorCode, message: String, correlation_id: String) -> Response {
    let template = ErrorPageTemplate {
        heading: heading_for(code),
        message,
        // Signing in again is the fix for an expired session, and for a failed
        // sign-in. It is not the fix for a missing role, and offering it there
        // would just send the user round the loop again.
        show_retry: matches!(code, ErrorCode::Unauthenticated | ErrorCode::Validation),
        correlation_id,
    };

    match template.render() {
        Ok(html) => {
            let mut response = Response::new(Body::from(html));
            *response.status_mut() = status;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            response
        }
        Err(error) => {
            tracing::error!(%error, "could not render the error page");
            let mut response = Response::new(Body::from(nb::ERR_INTERNAL));
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        }
    }
}

/// Returns the headline shown for an error code.
fn heading_for(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Unauthenticated => nb::ERROR_HEADING_SIGN_IN,
        ErrorCode::Forbidden => nb::ERROR_HEADING_NO_ACCESS,
        ErrorCode::NotFound => nb::ERROR_HEADING_NOT_FOUND,
        ErrorCode::RateLimited => nb::ERROR_HEADING_TOO_MUCH,
        _ => nb::ERROR_HEADING_SOMETHING_WRONG,
    }
}

/// Returns `true` when the request came from a browser navigating to a page.
///
/// An HTMX request is excluded even though it accepts HTML: it swaps the body
/// into an existing page, where a full document would be nonsense.
fn is_page_navigation(uri: &Uri, headers: &HeaderMap) -> bool {
    if uri.path().starts_with(API_PREFIX) || headers.contains_key("hx-request") {
        return false;
    }

    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// Returns `true` when the response is an error carrying a JSON body.
fn is_json_error(response: &Response) -> bool {
    if !(response.status().is_client_error() || response.status().is_server_error()) {
        return false;
    }

    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(accept: Option<&str>, htmx: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(accept) = accept {
            headers.insert(header::ACCEPT, accept.parse().expect("valid header"));
        }
        if htmx {
            headers.insert("hx-request", "true".parse().expect("valid header"));
        }
        headers
    }

    /// The Accept header Chrome and Firefox send on a top-level navigation.
    const BROWSER_ACCEPT: &str =
        "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,*/*;q=0.8";

    #[test]
    fn a_browser_navigation_gets_html() {
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(is_page_navigation(
            &uri,
            &headers(Some(BROWSER_ACCEPT), false)
        ));

        let callback: Uri = "/auth/callback?code=x".parse().expect("valid uri");
        assert!(is_page_navigation(
            &callback,
            &headers(Some(BROWSER_ACCEPT), false)
        ));
    }

    #[test]
    fn the_api_always_answers_in_json() {
        let uri: Uri = "/api/jobs/1".parse().expect("valid uri");
        assert!(!is_page_navigation(
            &uri,
            &headers(Some(BROWSER_ACCEPT), false)
        ));
    }

    #[test]
    fn htmx_is_left_alone() {
        // A full document swapped into a fragment would be nonsense.
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(!is_page_navigation(
            &uri,
            &headers(Some(BROWSER_ACCEPT), true)
        ));
    }

    #[test]
    fn a_client_that_did_not_ask_for_html_keeps_its_json() {
        let uri: Uri = "/".parse().expect("valid uri");
        assert!(!is_page_navigation(
            &uri,
            &headers(Some("application/json"), false)
        ));
        assert!(!is_page_navigation(&uri, &headers(None, false)));
    }

    #[test]
    fn only_json_error_responses_are_rewritten() {
        let json_404 = {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_FOUND;
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        };
        assert!(is_json_error(&json_404));

        let html_200 = {
            let mut response = Response::new(Body::empty());
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            );
            response
        };
        assert!(!is_json_error(&html_200));

        // A redirect to the sign-in page must pass through untouched.
        let redirect = {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::SEE_OTHER;
            response
        };
        assert!(!is_json_error(&redirect));
    }

    #[test]
    fn signing_in_again_is_offered_only_where_it_would_help() {
        assert!(matches!(
            heading_for(ErrorCode::Forbidden),
            nb::ERROR_HEADING_NO_ACCESS
        ));

        let no_access = ErrorPageTemplate {
            heading: heading_for(ErrorCode::Forbidden),
            message: nb::ERR_FORBIDDEN.to_owned(),
            show_retry: false,
            correlation_id: "abc-123".to_owned(),
        };
        let html = no_access.render().expect("the error page should render");

        assert!(html.contains(nb::ERR_FORBIDDEN));
        assert!(html.contains(nb::ERROR_HEADING_NO_ACCESS));
        // Sending someone without the role back through sign-in just loops.
        assert!(!html.contains("/auth/login"));
        assert!(html.contains("abc-123"));
        assert!(html.contains("lang=\"nb\""));
    }

    #[test]
    fn an_expired_session_is_offered_a_fresh_sign_in() {
        let html = ErrorPageTemplate {
            heading: heading_for(ErrorCode::Unauthenticated),
            message: nb::ERR_UNAUTHENTICATED.to_owned(),
            show_retry: true,
            correlation_id: String::new(),
        }
        .render()
        .expect("the error page should render");

        assert!(html.contains("/auth/login"));
        // With no correlation id there is nothing to quote, so the line is gone.
        assert!(!html.contains(nb::SUPPORT_REFERENCE));
    }
}
