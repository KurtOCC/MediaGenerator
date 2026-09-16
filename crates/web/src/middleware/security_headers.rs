//! Security response headers.
//!
//! The Content-Security-Policy is deliberately strict: no `unsafe-inline` and
//! no third-party origins. HTMX and the small amount of vanilla JS are served
//! from `/assets`, and Tailwind is compiled to a static stylesheet, so nothing
//! needs an inline `<script>` or `<style>`.

use axum::http::{HeaderName, HeaderValue, header};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::config::Environment;

/// The Content-Security-Policy applied to every response.
///
/// `blob:` is allowed for `img-src` and `media-src` because generated media is
/// fetched through a time-limited SAS URL and previewed from an object URL.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     script-src 'self'; \
     style-src 'self'; \
     img-src 'self' data: blob: https://*.blob.core.windows.net; \
     media-src 'self' blob: https://*.blob.core.windows.net; \
     font-src 'self'; \
     connect-src 'self'; \
     form-action 'self'; \
     frame-ancestors 'none'; \
     base-uri 'none'; \
     object-src 'none'";

/// `Strict-Transport-Security` value: two years, subdomains included.
const HSTS: &str = "max-age=63072000; includeSubDomains";

/// Header name for the CSP, which `http` does not provide as a constant.
const CSP_HEADER: HeaderName = HeaderName::from_static("content-security-policy");

/// Returns the CSP layer.
pub fn content_security_policy() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        CSP_HEADER,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    )
}

/// Returns the `X-Content-Type-Options: nosniff` layer.
pub fn no_sniff() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    )
}

/// Returns the `Referrer-Policy` layer.
pub fn referrer_policy() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(
        header::REFERRER_POLICY,
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    )
}

/// Returns the `X-Frame-Options: DENY` layer.
///
/// Redundant with `frame-ancestors 'none'`, kept for older user agents.
pub fn frame_options() -> SetResponseHeaderLayer<HeaderValue> {
    SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"))
}

/// Returns the HSTS layer, or `None` outside production.
///
/// Sending HSTS over plain `http://localhost` would pin developers' browsers to
/// HTTPS for a host that does not serve it.
pub fn strict_transport_security(
    environment: Environment,
) -> Option<SetResponseHeaderLayer<HeaderValue>> {
    environment.is_production().then(|| {
        SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(HSTS),
        )
    })
}
