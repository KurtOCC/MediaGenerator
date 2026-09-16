//! End-to-end tests for the unauthenticated probe endpoints and the global
//! middleware stack, driven through the real router.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use mediagenerator_web::build_router;
use tower::ServiceExt as _;

/// Sends a `GET` through the full router and returns the response.
async fn get(path: &str) -> axum::response::Response {
    let router = build_router(common::test_state());
    let request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    router.oneshot(request).await.expect("router is infallible")
}

#[tokio::test]
async fn health_reports_ok_without_authentication() {
    let response = get("/health").await;
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("body should be readable");
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).expect("body should be valid JSON");

    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn ready_reports_ok_without_authentication() {
    let response = get("/ready").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_carry_the_security_headers() {
    let response = get("/health").await;
    let headers = response.headers();

    let csp = headers
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("CSP header should be set");
    assert!(csp.contains("default-src 'self'"));
    assert!(
        !csp.contains("unsafe-inline"),
        "CSP must not allow inline script or style"
    );

    assert_eq!(
        headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .map(|v| v.as_bytes()),
        Some(&b"nosniff"[..])
    );
    assert_eq!(
        headers.get(header::X_FRAME_OPTIONS).map(|v| v.as_bytes()),
        Some(&b"DENY"[..])
    );
    assert!(headers.get(header::REFERRER_POLICY).is_some());
}

#[tokio::test]
async fn hsts_is_absent_outside_production() {
    let response = get("/health").await;
    assert!(
        response
            .headers()
            .get(header::STRICT_TRANSPORT_SECURITY)
            .is_none(),
        "HSTS must not be sent over plain http in development"
    );
}

#[tokio::test]
async fn a_correlation_id_is_generated_when_the_caller_sends_none() {
    let response = get("/health").await;
    let id = response
        .headers()
        .get("x-correlation-id")
        .and_then(|value| value.to_str().ok())
        .expect("a correlation id should be generated");
    assert!(
        uuid::Uuid::parse_str(id).is_ok(),
        "expected a UUID, got {id}"
    );
}

#[tokio::test]
async fn a_sane_inbound_correlation_id_is_echoed_back() {
    let router = build_router(common::test_state());
    let request = Request::builder()
        .uri("/health")
        .header("x-correlation-id", "abc-123")
        .body(Body::empty())
        .expect("request builder produced an invalid request");

    let response = router.oneshot(request).await.expect("router is infallible");
    assert_eq!(
        response
            .headers()
            .get("x-correlation-id")
            .and_then(|value| value.to_str().ok()),
        Some("abc-123")
    );
}

#[tokio::test]
async fn unknown_paths_return_404() {
    let response = get("/finnes-ikke").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
