//! Shared application state handed to every handler.

use std::sync::Arc;

use crate::config::AppConfig;

/// Immutable state cloned into each request.
///
/// Cloning is cheap: the configuration sits behind an [`Arc`]. Later phases add
/// the database pool, the job queue handle and the provider clients here.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Validated application configuration.
    pub config: Arc<AppConfig>,
}

impl AppState {
    /// Builds the shared state from a loaded configuration.
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(config),
        }
    }
}
