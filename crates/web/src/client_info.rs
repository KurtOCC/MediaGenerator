//! Client details recorded in the audit trail.

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{header, request::Parts},
};
use std::{convert::Infallible, net::SocketAddr};

/// Where a request came from, as far as the application can tell.
#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    /// Client IP address.
    pub ip: Option<String>,
    /// Client user agent.
    pub user_agent: Option<String>,
}

impl<S> FromRequestParts<S> for ClientInfo
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self {
            ip: client_ip(parts),
            user_agent: parts
                .headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned),
        })
    }
}

/// Determines the client IP.
///
/// Azure Container Apps terminates TLS in front of the process, so the peer
/// address is the ingress, not the user. `X-Forwarded-For` is read for that
/// reason — but only its first entry, and only as the audit record of what the
/// proxy claimed. It is never used for an access decision, which is why a
/// spoofed header here cannot grant anything.
fn client_ip(parts: &Parts) -> Option<String> {
    if let Some(forwarded) = parts
        .headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
    {
        let candidate = first.trim();
        if !candidate.is_empty() && candidate.parse::<std::net::IpAddr>().is_ok() {
            return Some(candidate.to_owned());
        }
    }

    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip().to_string())
}
