//! Configuration needed to talk to Microsoft Entra ID.

use crate::error::AuthError;

/// Settings for the OpenID Connect relying party.
///
/// Built by the web layer from the process configuration so that this crate
/// stays independent of how configuration is loaded.
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Entra ID directory (tenant) ID.
    pub tenant_id: String,
    /// Application (client) ID from the app registration.
    pub client_id: String,
    /// Client secret. `None` when authenticating with a Managed Identity.
    pub client_secret: Option<String>,
    /// Redirect URI, identical to the one registered in Entra ID.
    pub redirect_uri: String,
    /// Where to send the browser after a successful sign-out.
    pub post_logout_redirect_uri: String,
    /// App role required to use the application. `None` disables the check.
    pub required_app_role: Option<String>,
}

impl OidcConfig {
    /// Returns the issuer URL for this tenant.
    ///
    /// `openidconnect` appends `/.well-known/openid-configuration` when
    /// discovering, producing the documented Entra ID v2.0 discovery URL.
    pub fn issuer_url(&self) -> String {
        format!("https://login.microsoftonline.com/{}/v2.0", self.tenant_id)
    }

    /// Validates the settings that must hold before any network call is made.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::InvalidUrl`] when a URL is not absolute.
    pub fn validate(&self) -> Result<(), AuthError> {
        for (name, value) in [
            ("OIDC_REDIRECT_URI", &self.redirect_uri),
            ("APP_BASE_URL", &self.post_logout_redirect_uri),
        ] {
            if !(value.starts_with("https://") || value.starts_with("http://")) {
                return Err(AuthError::InvalidUrl(format!(
                    "{name} must be an absolute http(s) URL"
                )));
            }
        }
        Ok(())
    }

    /// Returns the required app role, treating blank as "not configured".
    pub fn required_app_role(&self) -> Option<&str> {
        self.required_app_role
            .as_deref()
            .map(str::trim)
            .filter(|role| !role.is_empty())
    }
}
