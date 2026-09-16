//! Shared application state handed to every handler.

use std::sync::Arc;

use mediagenerator_auth::{OidcProvider, RolePolicy};

use crate::config::AppConfig;

/// Immutable state cloned into each request.
///
/// Cloning is cheap: everything sits behind an [`Arc`]. Later phases add the
/// database pool, the job queue handle and the provider clients here.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Validated application configuration.
    pub config: Arc<AppConfig>,
    /// OpenID Connect relying party for Entra ID.
    pub oidc: Arc<OidcProvider>,
    /// App role required to use the application, if any.
    pub role_policy: RolePolicy,
}

impl AppState {
    /// Builds the shared state from a loaded configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the OpenID Connect settings are invalid. No
    /// network call is made here.
    pub fn new(config: AppConfig) -> Result<Self, mediagenerator_auth::AuthError> {
        let role_policy = RolePolicy::new(config.required_app_role());
        let oidc = OidcProvider::new(config.oidc())?;

        Ok(Self {
            config: Arc::new(config),
            oidc: Arc::new(oidc),
            role_policy,
        })
    }
}
