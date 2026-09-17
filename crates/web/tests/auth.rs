//! End-to-end tests for the authentication guards and the `/auth` routes.
//!
//! None of these reach the network: every case is decided by a guard, by the
//! origin check or by the absence of a login flow in the session, all of which
//! happen before any call to Entra ID.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use mediagenerator_domain::i18n::nb;
use tower::ServiceExt as _;

/// Sends a request through the full router.
async fn send(request: Request<Body>) -> axum::response::Response {
    common::test_router()
        .oneshot(request)
        .await
        .expect("router is infallible")
}

/// Sends a bare `GET`.
async fn get(path: &str) -> axum::response::Response {
    let request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request builder produced an invalid request");
    send(request).await
}

/// Reads a JSON body.
async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body should be readable");
    serde_json::from_slice(&bytes).expect("body should be valid JSON")
}

#[tokio::test]
async fn an_unauthenticated_page_request_is_redirected_to_sign_in() {
    let response = get("/").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login")
    );
}

#[tokio::test]
async fn the_original_destination_is_preserved_across_sign_in() {
    // A protected path that carries a query string. `/historikk` would be the
    // natural example but it does not exist until phase 6, and an unknown path
    // is a 404 rather than a redirect.
    let response = get("/?side=2").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login?return_to=%2F%3Fside%3D2")
    );
}

#[tokio::test]
async fn an_unknown_path_is_a_404_rather_than_a_redirect_to_sign_in() {
    // The guards must not wrap the fallback: a path that does not exist should
    // say so, not send an anonymous visitor through the identity provider.
    let response = get("/finnes-ikke").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(json(response).await["message"], nb::ERR_NOT_FOUND);
}

#[tokio::test]
async fn an_unauthenticated_api_request_gets_401_json_in_norwegian() {
    let response = get("/api/me").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = json(response).await;
    assert_eq!(body["code"], "unauthenticated");
    assert_eq!(body["message"], nb::ERR_UNAUTHENTICATED);
}

#[tokio::test]
async fn an_unauthenticated_htmx_request_gets_an_hx_redirect() {
    let request = Request::builder()
        .uri("/")
        .header("hx-request", "true")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;

    // HTMX swaps the body of a 3xx instead of navigating, so the redirect has
    // to travel in a header it understands.
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get("hx-redirect")
            .and_then(|value| value.to_str().ok()),
        Some("/auth/login")
    );
}

#[tokio::test]
async fn logout_without_an_origin_header_is_refused() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/auth/logout")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(response).await["message"], nb::ERR_CSRF);
}

#[tokio::test]
async fn logout_from_a_foreign_origin_is_refused() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/auth/logout")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(json(response).await["message"], nb::ERR_CSRF);
}

#[tokio::test]
async fn the_origin_check_leaves_safe_methods_alone() {
    // A GET with a foreign Origin is not state-changing and must still work.
    let request = Request::builder()
        .uri("/health")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    assert_eq!(send(request).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_callback_without_a_login_flow_is_rejected() {
    // A replayed or bookmarked callback: the session holds no flow, so the
    // request is refused before any token exchange is attempted.
    let response = get("/auth/callback?code=abc&state=xyz").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(response).await["message"], nb::ERR_LOGIN_FAILED);
}

#[tokio::test]
async fn a_callback_carrying_a_provider_error_is_rejected() {
    let response = get("/auth/callback?error=access_denied").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // The provider error_description is logged, never shown.
    assert_eq!(json(response).await["message"], nb::ERR_LOGIN_FAILED);
}

#[tokio::test]
async fn the_session_cookie_is_hardened() {
    // Touching /auth/callback is enough to make the session layer issue a
    // cookie; what matters here is the attributes it carries.
    let response = get("/auth/callback?code=abc&state=xyz").await;

    let cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("mediagenerator.sid="));

    if let Some(cookie) = cookie {
        assert!(
            cookie.contains("HttpOnly"),
            "cookie must be HttpOnly: {cookie}"
        );
        assert!(
            cookie.contains("SameSite=Lax"),
            "cookie must be Lax: {cookie}"
        );
        assert!(
            cookie.contains("Path=/"),
            "cookie must be path-scoped: {cookie}"
        );
        // Secure is off in development, because it is served over plain http.
        assert!(
            !cookie.contains("Secure"),
            "Secure must not be set over http in development: {cookie}"
        );
    }
}

#[tokio::test]
async fn health_and_ready_stay_reachable_without_a_session() {
    assert_eq!(get("/health").await.status(), StatusCode::OK);
    assert_eq!(get("/ready").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn generate_requires_a_session() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/generate")
        .header(header::ORIGIN, "http://localhost:8080")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("prompt=test&media_type=image"))
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(response).await["message"], nb::ERR_UNAUTHENTICATED);
}

#[tokio::test]
async fn the_static_pages_are_all_behind_the_guard() {
    for path in ["/", "/historikk", "/eksempler"] {
        assert_eq!(
            get(path).await.status(),
            StatusCode::SEE_OTHER,
            "{path} should require a session"
        );
    }
}

#[tokio::test]
async fn assets_are_served_without_a_session() {
    // The stylesheet and the scripts must load before anyone has signed in,
    // otherwise the sign-in page itself would be unstyled.
    for path in [
        "/assets/css/app.css",
        "/assets/js/htmx.min.js",
        "/assets/js/app.js",
    ] {
        let response = get(path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path} should be served");
    }
}

#[tokio::test]
async fn assets_are_not_cached_by_the_api_cache_control() {
    let response = get("/assets/css/app.css").await;
    assert!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .is_some_and(|value| value.as_bytes().starts_with(b"public")),
        "static assets should be cacheable"
    );

    let health = get("/health").await;
    assert!(
        health.headers().get(header::CACHE_CONTROL).is_none(),
        "probe responses must not inherit the asset cache policy"
    );
}

#[tokio::test]
async fn the_archive_pages_are_behind_the_guard() {
    for path in [
        "/historikk",
        "/eksempler",
        "/eksempler?eier=alle&type=lyd&sortering=eldst&side=2",
    ] {
        assert_eq!(
            get(path).await.status(),
            StatusCode::SEE_OTHER,
            "{path} should require a session"
        );
    }
}

#[tokio::test]
async fn an_asset_cannot_be_reached_without_a_session() {
    // The SAS is minted behind an ownership check; without a session there is
    // nobody to check ownership against.
    let response = get("/api/assets/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(response).await["message"], nb::ERR_UNAUTHENTICATED);
}

/// The `Accept` header a browser sends on a top-level navigation.
const BROWSER_ACCEPT: &str =
    "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,*/*;q=0.8";

#[tokio::test]
async fn a_browser_navigation_gets_an_html_error_page_not_json() {
    // `Accept-Encoding` matters: the compression layer wraps the body, and an
    // error-page middleware placed outside it would be handed gzip and quietly
    // pass the JSON through. That is exactly what shipped the first time.
    let request = Request::builder()
        .uri("/finnes-ikke")
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .header(header::ACCEPT_ENCODING, "gzip, deflate, br")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        content_type.starts_with("text/html"),
        "expected an HTML page, got {content_type}"
    );

    // The body itself is checked without Accept-Encoding: the compression
    // layer sits outside this one and will gzip the rendered page, which is
    // correct and which the browser handles.
    let plain = Request::builder()
        .uri("/finnes-ikke")
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(plain).await;
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body should be readable");
    let html = String::from_utf8(bytes.to_vec()).expect("the page should be UTF-8");

    assert!(html.contains("<!doctype html>"));
    assert!(html.contains(nb::ERR_NOT_FOUND));
    assert!(html.contains("lang=\"nb\""));
}

#[tokio::test]
async fn a_forbidden_sign_in_is_a_page_and_not_raw_json() {
    // What the first real user saw: a black page of JSON after being refused
    // the app role.
    let request = Request::builder()
        .uri("/auth/callback?error=access_denied")
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .header(header::ACCEPT_ENCODING, "gzip")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")),
        "a failed sign-in must render as a page"
    );
}

#[tokio::test]
async fn the_api_still_answers_in_json_even_for_a_browser_accept_header() {
    // The negotiation must not leak into the API: HTMX and fetch() both send
    // browser-ish Accept headers.
    let request = Request::builder()
        .uri("/api/me")
        .header(header::ACCEPT, BROWSER_ACCEPT)
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json(response).await["code"], "unauthenticated");
}
