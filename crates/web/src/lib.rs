//! HTTP layer for Mediagenerator: configuration, router assembly, middleware
//! and request handlers.
//!
//! The router is built by [`build_router`] so that integration tests can drive
//! the exact same stack the binary serves, without binding a port.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod middleware;
pub mod render;
pub mod routes;
pub mod session;
pub mod state;
pub mod telemetry;

use std::time::Duration;

use axum::{
    Router,
    http::{HeaderValue, StatusCode, header},
    middleware::{from_fn, from_fn_with_state},
};
use mediagenerator_auth::{require_auth, require_role};
use tower::{ServiceBuilder, util::option_layer};
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, limit::RequestBodyLimitLayer,
    sensitive_headers::SetSensitiveRequestHeadersLayer, services::ServeDir,
    set_header::SetResponseHeaderLayer, timeout::TimeoutLayer, trace::TraceLayer,
};
use tower_sessions::{SessionManagerLayer, SessionStore, service::SignedCookie};

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

/// Handles any path no route matched.
async fn not_found() -> AppError {
    AppError::NotFound
}

/// Builds the complete application router.
///
/// Routes fall into three groups:
///
/// * `/health`, `/ready` and `/assets/*` — reachable without a session,
/// * `/auth/*` — necessarily unauthenticated, since they establish the session,
/// * everything else — behind `require_auth` and, when configured,
///   `require_role`.
///
/// Layers are listed innermost first. The session layer wraps all three groups,
/// because `/auth/login` needs to write to a session before anyone is signed in,
/// and the correlation span wraps everything so that all logs carry the ID.
pub fn build_router<Store>(
    state: AppState,
    session_layer: SessionManagerLayer<Store, SignedCookie>,
) -> Router
where
    Store: SessionStore + Clone + 'static,
{
    let environment = state.config.app_env;
    let origin = middleware::csrf::Origin::from_base_url(&state.config.app_base_url);

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

    // The double-submit token is checked on /api/* only. Those requests are all
    // issued by HTMX, which sends the header; the sign-out form is an ordinary
    // browser POST, covered by the origin check and the SameSite=Lax cookie.
    let api = routes::api::router().layer(from_fn(middleware::csrf::verify_token));

    // `require_auth` is applied last, so it is the outermost of the two and
    // runs first: `require_role` can then rely on the user being present.
    let protected = routes::pages::router()
        .merge(api)
        .layer(from_fn_with_state(state.role_policy.clone(), require_role))
        .layer(from_fn(require_auth));

    Router::new()
        .merge(routes::health::router())
        .merge(routes::auth::router())
        .merge(protected)
        .nest_service("/assets", assets)
        // Set outside the guarded router: without this, the guards would also
        // wrap the default fallback and an unknown path would bounce an
        // anonymous visitor through sign-in instead of simply not existing.
        .fallback(not_found)
        .layer(session_layer)
        .layer(from_fn_with_state(origin, middleware::csrf::verify_origin))
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
        .layer(from_fn(middleware::correlation::propagate))
        .with_state(state)
}
