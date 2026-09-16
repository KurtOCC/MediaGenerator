//! HTTP layer for Mediagenerator: configuration, router assembly, middleware
//! and request handlers.
//!
//! The router is built by [`build_router`] so that integration tests can drive
//! the exact same stack the binary serves, without binding a port.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod middleware;
pub mod routes;
pub mod state;
pub mod telemetry;

use std::time::Duration;

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
};
use tower::{ServiceBuilder, util::option_layer};
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, limit::RequestBodyLimitLayer,
    sensitive_headers::SetSensitiveRequestHeadersLayer, services::ServeDir,
    set_header::SetResponseHeaderLayer, timeout::TimeoutLayer, trace::TraceLayer,
};

pub use config::AppConfig;
pub use error::{AppError, AppResult};
pub use state::AppState;

/// Largest accepted request body. Prompts are text, so this is generous.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// Upper bound on how long a single request may take.
///
/// Generation itself is asynchronous and polled, so no user-facing request
/// needs to be long-running. Server-Sent Events get an exemption in phase 4.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long browsers may cache static assets.
const ASSET_CACHE_CONTROL: &str = "public, max-age=3600";

/// Builds the complete application router.
///
/// Layers are listed outermost first: the correlation span wraps everything so
/// that all downstream logs carry the ID, and the static file service is
/// mounted last so it never shadows an application route.
pub fn build_router(state: AppState) -> Router {
    let environment = state.config.app_env;
    // Cache-Control is attached to the static file service only; API and HTML
    // responses must not inherit it.
    let assets = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static(ASSET_CACHE_CONTROL),
        ))
        .service(
            ServeDir::new(state.config.assets_dir.clone())
                .precompressed_gzip()
                .append_index_html_on_directories(false),
        );

    Router::new()
        .merge(routes::health::router())
        .nest_service("/assets", assets)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(CompressionLayer::new())
        // No cross-origin caller is expected: the UI is served from the same
        // origin. An empty policy means the browser blocks everything else.
        .layer(CorsLayer::new())
        .layer(middleware::security_headers::content_security_policy())
        .layer(middleware::security_headers::no_sniff())
        .layer(middleware::security_headers::referrer_policy())
        .layer(middleware::security_headers::frame_options())
        .layer(option_layer(
            middleware::security_headers::strict_transport_security(environment),
        ))
        .layer(TraceLayer::new_for_http())
        .layer(SetSensitiveRequestHeadersLayer::new([
            header::AUTHORIZATION,
            header::COOKIE,
        ]))
        .layer(axum::middleware::from_fn(
            middleware::correlation::propagate,
        ))
        .with_state(state)
}
