//! Shared HTTP behaviour: timeouts, retries and error mapping.
//!
//! Every provider call goes through [`send_with_retry`], so the retry policy,
//! the `Retry-After` handling and the mapping from status code to
//! [`ProviderError`] exist once rather than three times.

use std::time::Duration;

use rand::RngExt as _;
use reqwest::{Client, Response, StatusCode, header::HeaderMap};

use crate::error::ProviderError;

/// How long a single HTTP call may take.
///
/// Image generation regularly takes tens of seconds, so this is generous. The
/// job itself is asynchronous, so nothing user-facing is blocked by it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

/// How many times a retryable failure is retried.
const MAX_ATTEMPTS: u32 = 4;

/// Delay before the first retry. Doubles each attempt.
const BASE_BACKOFF: Duration = Duration::from_millis(500);

/// Ceiling on a single backoff, before jitter.
const MAX_BACKOFF: Duration = Duration::from_secs(20);

/// Longest `Retry-After` that is honoured by waiting.
///
/// Beyond this the job is failed instead: telling the user to come back in a
/// minute is better than holding a worker for ten.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Builds the shared HTTP client.
///
/// # Errors
///
/// Returns an error when the client cannot be constructed.
pub fn build_http_client() -> Result<Client, ProviderError> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        // Following a redirect from a data-plane call would be an SSRF vector.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            ProviderError::Internal(format!("could not build an HTTP client: {error}"))
        })
}

/// Sends a request, retrying transient failures with exponential backoff.
///
/// `build` is called afresh for each attempt because a `reqwest::Request` is
/// consumed when sent and a body is not always cloneable.
///
/// Only rate limits and upstream failures are retried; a rejected prompt or a
/// bad parameter fails identically every time and retrying it wastes quota.
///
/// # Errors
///
/// Returns the mapped error from the last attempt.
pub async fn send_with_retry<F>(
    build: F,
    correlation_id: &str,
    operation: &str,
) -> Result<Response, ProviderError>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt = 1;

    loop {
        let started = std::time::Instant::now();
        let outcome = build().send().await;

        let error = match outcome {
            Ok(response) if response.status().is_success() => {
                tracing::debug!(
                    correlation_id,
                    operation,
                    attempt,
                    status = response.status().as_u16(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "provider call succeeded"
                );
                return Ok(response);
            }
            Ok(response) => {
                let status = response.status();
                let retry_after = retry_after(response.headers());
                // The body carries the provider's own error code, which is how
                // a content-filter rejection is told apart from a bad request.
                let body = response.text().await.unwrap_or_default();
                let error = map_status(status, &body);

                tracing::warn!(
                    correlation_id,
                    operation,
                    attempt,
                    status = status.as_u16(),
                    elapsed_ms = started.elapsed().as_millis(),
                    error = %error,
                    "provider call failed"
                );

                if error.is_retryable() && attempt < MAX_ATTEMPTS {
                    // Honour Retry-After when the service gives one; it knows
                    // more about its own capacity than any backoff formula.
                    let delay = retry_after.unwrap_or_else(|| backoff(attempt));
                    if delay > MAX_RETRY_AFTER {
                        tracing::warn!(
                            correlation_id,
                            operation,
                            retry_after_s = delay.as_secs(),
                            "Retry-After is longer than we are willing to wait"
                        );
                        return Err(error);
                    }
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
                return Err(error);
            }
            Err(transport) => {
                let error = if transport.is_timeout() {
                    ProviderError::Timeout
                } else {
                    ProviderError::Upstream(transport.to_string())
                };

                tracing::warn!(
                    correlation_id,
                    operation,
                    attempt,
                    error = %error,
                    "provider call could not be completed"
                );

                if attempt < MAX_ATTEMPTS && !matches!(error, ProviderError::Timeout) {
                    tokio::time::sleep(backoff(attempt)).await;
                    attempt += 1;
                    continue;
                }
                error
            }
        };

        return Err(error);
    }
}

/// Returns the delay before retry `attempt`, with jitter.
///
/// Full jitter: the delay is uniform in `[0, capped]` rather than exactly
/// `capped`. Without it, several workers rate-limited at the same moment would
/// retry in lockstep and rate-limit each other again.
fn backoff(attempt: u32) -> Duration {
    let exponential = BASE_BACKOFF.saturating_mul(2_u32.saturating_pow(attempt - 1));
    let capped = exponential.min(MAX_BACKOFF);
    let millis = rand::rng().random_range(0..=capped.as_millis().max(1) as u64);
    Duration::from_millis(millis)
}

/// Reads `Retry-After`, which Azure sends as whole seconds.
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Maps an HTTP status and body to a provider error.
pub fn map_status(status: StatusCode, body: &str) -> ProviderError {
    // A content filter rejection arrives as a 400, so the body has to be read
    // to tell it apart from an ordinary bad request.
    if looks_like_content_filter(body) {
        return ProviderError::ContentFilter;
    }

    match status {
        StatusCode::BAD_REQUEST => ProviderError::Validation(summarise(body)),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderError::Unauthorized,
        StatusCode::NOT_FOUND => ProviderError::DeploymentNotFound(summarise(body)),
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimited,
        _ if status.is_server_error() => ProviderError::Upstream(summarise(body)),
        _ => ProviderError::Upstream(format!("unexpected status {status}: {}", summarise(body))),
    }
}

/// Returns `true` when the body looks like a content-filter rejection.
fn looks_like_content_filter(body: &str) -> bool {
    let lowered = body.to_lowercase();
    lowered.contains("content_filter")
        || lowered.contains("responsibleaipolicyviolation")
        || lowered.contains("content_policy_violation")
}

/// Shortens a response body for logging.
///
/// Bounded so a large error page cannot flood the log, and cut on a character
/// boundary so the result stays valid UTF-8.
fn summarise(body: &str) -> String {
    const MAX: usize = 400;
    if body.len() <= MAX {
        return body.trim().to_owned();
    }
    let mut end = MAX;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", body[..end].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_filter_rejection_is_recognised_despite_its_400() {
        let body = r#"{"error":{"code":"content_filter","message":"avvist"}}"#;
        assert!(matches!(
            map_status(StatusCode::BAD_REQUEST, body),
            ProviderError::ContentFilter
        ));
    }

    #[test]
    fn a_responsible_ai_violation_is_also_a_content_filter() {
        let body = r#"{"error":{"code":"ResponsibleAIPolicyViolation"}}"#;
        assert!(matches!(
            map_status(StatusCode::BAD_REQUEST, body),
            ProviderError::ContentFilter
        ));
    }

    #[test]
    fn statuses_map_to_the_documented_errors() {
        assert!(matches!(
            map_status(StatusCode::BAD_REQUEST, "{}"),
            ProviderError::Validation(_)
        ));
        assert!(matches!(
            map_status(StatusCode::UNAUTHORIZED, ""),
            ProviderError::Unauthorized
        ));
        assert!(matches!(
            map_status(StatusCode::FORBIDDEN, ""),
            ProviderError::Unauthorized
        ));
        assert!(matches!(
            map_status(StatusCode::NOT_FOUND, "DeploymentNotFound"),
            ProviderError::DeploymentNotFound(_)
        ));
        assert!(matches!(
            map_status(StatusCode::TOO_MANY_REQUESTS, ""),
            ProviderError::RateLimited
        ));
        assert!(matches!(
            map_status(StatusCode::INTERNAL_SERVER_ERROR, ""),
            ProviderError::Upstream(_)
        ));
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(ProviderError::RateLimited.is_retryable());
        assert!(ProviderError::Upstream(String::new()).is_retryable());
        // Retrying these would fail identically and burn quota.
        assert!(!ProviderError::ContentFilter.is_retryable());
        assert!(!ProviderError::Validation(String::new()).is_retryable());
        assert!(!ProviderError::Unauthorized.is_retryable());
    }

    #[test]
    fn backoff_grows_but_stays_within_the_ceiling() {
        for attempt in 1..=8 {
            assert!(backoff(attempt) <= MAX_BACKOFF);
        }
    }

    #[test]
    fn retry_after_is_read_as_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().expect("hv"));
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(30)));

        // A date-formatted Retry-After is ignored rather than mis-parsed.
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2026 07:28:00 GMT".parse().expect("hv"),
        );
        assert_eq!(retry_after(&headers), None);
    }

    #[test]
    fn a_long_body_is_truncated_on_a_character_boundary() {
        let body = "æ".repeat(500);
        let summary = summarise(&body);
        assert!(summary.len() <= 405);
        assert!(summary.ends_with('…'));
    }
}
