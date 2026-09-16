//! Tracing set-up.
//!
//! Development uses a compact, human-readable format; production emits one JSON
//! object per event so Azure Container Apps / Log Analytics can index the
//! fields (including `correlation_id`) without parsing prose.

use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Environment;

/// Installs the global tracing subscriber.
///
/// `filter` is a `RUST_LOG`-style directive string, e.g. `info,mediagenerator_web=debug`.
///
/// # Errors
///
/// Returns an error when `filter` is not a valid filter directive, or when a
/// subscriber has already been installed.
pub fn init(filter: &str, environment: Environment) -> Result<(), TelemetryError> {
    let env_filter = EnvFilter::try_new(filter)?;
    let registry = tracing_subscriber::registry().with(env_filter);

    if environment.is_production() {
        registry
            .with(
                fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(true),
            )
            .try_init()?;
    } else {
        registry
            .with(fmt::layer().compact().with_target(true))
            .try_init()?;
    }
    Ok(())
}

/// Errors raised while installing the tracing subscriber.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// The `RUST_LOG` directive string could not be parsed.
    #[error("invalid log filter directive")]
    Filter(#[from] tracing_subscriber::filter::ParseError),

    /// A global subscriber was already installed.
    #[error("a tracing subscriber is already installed")]
    AlreadyInitialised(#[from] tracing_subscriber::util::TryInitError),
}
