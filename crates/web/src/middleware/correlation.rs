//! Correlation ID propagation.
//!
//! Every request gets a correlation ID — reused from the inbound
//! `x-correlation-id` header when the caller supplied a sane one, otherwise
//! freshly generated. The ID is attached to the tracing span that wraps the
//! whole request, so every log line emitted downstream (including provider
//! calls) carries it, and it is echoed back on the response so a user can quote
//! it in a support request.

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;
use uuid::Uuid;

/// The header carrying the correlation ID, inbound and outbound.
pub const CORRELATION_ID_HEADER: HeaderName = HeaderName::from_static("x-correlation-id");

/// Upper bound on an accepted inbound correlation ID, to keep logs bounded.
const MAX_INBOUND_LEN: usize = 128;

/// The correlation ID of the current request, available via `Extension`.
#[derive(Debug, Clone)]
pub struct CorrelationId(pub String);

impl CorrelationId {
    /// Returns the ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Middleware that establishes the request span and propagates the correlation ID.
pub async fn propagate(mut request: Request, next: Next) -> Response {
    let correlation_id = request
        .headers()
        .get(&CORRELATION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| is_acceptable(value))
        .map_or_else(|| Uuid::new_v4().to_string(), ToOwned::to_owned);

    let span = tracing::info_span!(
        "http_request",
        correlation_id = %correlation_id,
        method = %request.method(),
        path = %request.uri().path(),
    );

    request
        .extensions_mut()
        .insert(CorrelationId(correlation_id.clone()));

    let mut response = next.run(request).instrument(span).await;

    if let Ok(value) = HeaderValue::from_str(&correlation_id) {
        response.headers_mut().insert(CORRELATION_ID_HEADER, value);
    }
    response
}

/// Returns `true` when an inbound correlation ID is safe to reuse in logs.
///
/// Restricted to printable ASCII without whitespace, and length-capped, so a
/// caller cannot inject newlines or unbounded data into the log stream.
fn is_acceptable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_INBOUND_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'"')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_plain_uuid() {
        assert!(is_acceptable("0b1e6f7e-9a1c-4b2e-9a3f-6b0b7f7a1c2d"));
    }

    #[test]
    fn rejects_empty_whitespace_and_oversized_values() {
        assert!(!is_acceptable(""));
        assert!(!is_acceptable("abc def"));
        assert!(!is_acceptable("a\nb"));
        assert!(!is_acceptable(&"a".repeat(MAX_INBOUND_LEN + 1)));
    }

    #[test]
    fn rejects_quotes_that_could_break_json_logs() {
        assert!(!is_acceptable("a\"b"));
    }
}
