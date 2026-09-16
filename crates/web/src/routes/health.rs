//! Liveness and readiness probes.
//!
//! Both endpoints are deliberately unauthenticated so that Azure Container Apps
//! and the Docker `HEALTHCHECK` can reach them. They expose no user data.

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;

use crate::state::AppState;

/// Package version, used to correlate a running instance with a build.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Body returned by both probes.
#[derive(Debug, Serialize)]
struct HealthBody {
    /// `"ok"` when the probe passes.
    status: &'static str,
    /// Build version of the running binary.
    version: &'static str,
}

/// Returns the health routes: `GET /health` and `GET /ready`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
}

/// Liveness probe: the process is running and can serve requests.
///
/// Never touches downstream dependencies — a failing database must not cause a
/// restart loop.
async fn health() -> Json<HealthBody> {
    Json(HealthBody {
        status: "ok",
        version: VERSION,
    })
}

/// Readiness probe: the instance is ready to receive traffic.
///
/// From phase 4 this also checks the database connection pool; today it mirrors
/// the liveness probe.
async fn ready(State(_state): State<AppState>) -> (StatusCode, Json<HealthBody>) {
    (
        StatusCode::OK,
        Json(HealthBody {
            status: "ok",
            version: VERSION,
        }),
    )
}
