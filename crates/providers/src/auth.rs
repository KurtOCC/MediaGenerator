//! Credentials for the Azure OpenAI data plane.
//!
//! Preferred: an Entra ID token for `https://cognitiveservices.azure.com/.default`,
//! from a Managed Identity in Azure or the developer's `az login` session
//! locally. The chain is spelled out in [`Credentials::new`]; the same code
//! path covers both, so nothing about authentication differs by environment.
//!
//! Fallback: an API key, for local development only. The web layer refuses one
//! when `APP_ENV=production`.

use std::sync::Arc;

use azure_core::credentials::{Secret, TokenCredential};
use azure_identity::{ClientSecretCredential, DeveloperToolsCredential, ManagedIdentityCredential};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::{config::ProviderConfig, error::ProviderError};

/// Scope the data-plane token is requested for.
const SCOPE: &str = "https://cognitiveservices.azure.com/.default";

/// Header carrying the API key, when one is used.
const API_KEY_HEADER: HeaderName = HeaderName::from_static("api-key");

/// How the client proves who it is.
#[derive(Clone)]
pub enum Credentials {
    /// Entra ID token, refreshed by the credential as needed.
    Entra(Arc<dyn TokenCredential>),
    /// Static API key. Development only.
    ApiKey(Secret),
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entra(_) => f.write_str("Credentials::Entra"),
            Self::ApiKey(_) => f.write_str("Credentials::ApiKey(***)"),
        }
    }
}

impl Credentials {
    /// Chooses a credential from configuration.
    ///
    /// `azure_identity` 1.0 has no single "default" chain, so the order is
    /// spelled out here, from most to least specific:
    ///
    /// 1. a client secret, when one is configured,
    /// 2. a Managed Identity, which is how this runs in Azure,
    /// 3. the developer tools credential, which picks up `az login`,
    /// 4. an API key, for local development only.
    ///
    /// # Errors
    ///
    /// Returns an error when none of the four is available.
    pub fn new(config: &ProviderConfig) -> Result<Self, ProviderError> {
        if let Some(secret) = config.client_secret.as_ref() {
            let credential = ClientSecretCredential::new(
                &config.tenant_id,
                config.client_id.clone(),
                Secret::new(secret.clone()),
                None,
            )
            .map_err(|error| {
                ProviderError::Internal(format!("invalid client credentials: {error}"))
            })?;
            tracing::info!("using an Entra ID client secret for Azure AI Foundry");
            return Ok(Self::Entra(credential));
        }

        // The choice between a managed identity and the developer chain is made
        // on the environment, not by trying each in turn: constructing the
        // managed identity credential succeeds anywhere, and only acquiring a
        // token fails — after a retry timeout of nearly two minutes.
        if running_in_azure() {
            let credential = ManagedIdentityCredential::new(None).map_err(|error| {
                ProviderError::Internal(format!("managed identity unavailable: {error}"))
            })?;
            tracing::info!("using a Managed Identity for Azure AI Foundry");
            return Ok(Self::Entra(credential));
        }

        if let Ok(credential) = DeveloperToolsCredential::new(None) {
            tracing::info!("using developer tooling credentials for Azure AI Foundry");
            return Ok(Self::Entra(credential));
        }

        let Some(key) = config.api_key.as_ref() else {
            return Err(ProviderError::Internal(
                "no Entra credential available and no API key configured".to_owned(),
            ));
        };
        tracing::warn!(
            "falling back to an API key for Azure AI Foundry; this is for local development only"
        );
        Ok(Self::ApiKey(Secret::new(key.clone())))
    }

    /// Returns the authentication headers for one request.
    ///
    /// # Errors
    ///
    /// Returns an error when a token could not be acquired.
    pub async fn headers(&self) -> Result<HeaderMap, ProviderError> {
        let mut headers = HeaderMap::new();

        match self {
            Self::Entra(credential) => {
                let token = credential
                    .get_token(&[SCOPE], None)
                    .await
                    .map_err(|error| {
                        ProviderError::Internal(format!("could not acquire a token: {error}"))
                    })?;

                let value = format!("Bearer {}", token.token.secret());
                let mut header = HeaderValue::from_str(&value).map_err(|_| {
                    ProviderError::Internal("token is not a valid header value".to_owned())
                })?;
                // Keeps the token out of any log that dumps headers.
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header);
            }
            Self::ApiKey(key) => {
                let mut header = HeaderValue::from_str(key.secret()).map_err(|_| {
                    ProviderError::Internal("API key is not a valid header value".to_owned())
                })?;
                header.set_sensitive(true);
                headers.insert(API_KEY_HEADER, header);
            }
        }

        Ok(headers)
    }
}

/// Returns `true` when the process is running somewhere that offers a managed
/// identity.
///
/// Container Apps, App Service and Functions all advertise one by setting
/// `IDENTITY_ENDPOINT`; the older App Service runtime used `MSI_ENDPOINT`.
/// Neither is ever set on a developer machine.
fn running_in_azure() -> bool {
    std::env::var_os("IDENTITY_ENDPOINT").is_some() || std::env::var_os("MSI_ENDPOINT").is_some()
}
