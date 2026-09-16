//! Shared application state handed to every handler.

use std::sync::Arc;

use mediagenerator_auth::{OidcProvider, RolePolicy};
use mediagenerator_providers::Providers;
use mediagenerator_storage::{BlobStore, Database};

use crate::{config::AppConfig, jobs::Jobs};

/// Immutable state cloned into each request.
///
/// Cloning is cheap: the configuration sits behind an [`Arc`], and the pool and
/// the queue handle are themselves cheap to clone.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Validated application configuration.
    pub config: Arc<AppConfig>,
    /// OpenID Connect relying party for Entra ID.
    pub oidc: Arc<OidcProvider>,
    /// App role required to use the application, if any.
    pub role_policy: RolePolicy,
    /// PostgreSQL connection pool.
    pub db: Database,
    /// The three Azure AI Foundry clients.
    pub providers: Arc<Providers>,
    /// Private container holding generated media.
    pub blobs: Arc<BlobStore>,
    /// Handle to the background job queue.
    pub jobs: Jobs,
}

impl AppState {
    /// Builds the shared state from a loaded configuration and an open pool.
    ///
    /// # Errors
    ///
    /// Returns an error when the OpenID Connect settings are invalid. No
    /// network call is made here.
    pub fn new(
        config: AppConfig,
        db: Database,
        providers: Arc<Providers>,
        blobs: Arc<BlobStore>,
        jobs: Jobs,
    ) -> Result<Self, mediagenerator_auth::AuthError> {
        let role_policy = RolePolicy::new(config.required_app_role());
        let oidc = OidcProvider::new(config.oidc())?;

        Ok(Self {
            config: Arc::new(config),
            oidc: Arc::new(oidc),
            role_policy,
            db,
            providers,
            blobs,
            jobs,
        })
    }
}
