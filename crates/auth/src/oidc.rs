//! OpenID Connect relying party for Microsoft Entra ID.
//!
//! Implements the Authorization Code Flow with PKCE:
//!
//! 1. [`OidcProvider::begin_login`] builds the authorization URL and returns the
//!    PKCE verifier, CSRF state and nonce that the caller must keep server-side.
//! 2. The user authenticates at Entra ID and is redirected back with a code.
//! 3. [`OidcProvider::complete_login`] verifies `state`, exchanges the code and
//!    validates the ID token signature, issuer, audience, nonce and expiry.
//!
//! Discovery metadata and the signing keys (JWKS) are fetched once and cached
//! for [`METADATA_TTL`], then refreshed lazily. Entra ID rotates its signing
//! keys, so the cache must not be permanent.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndSessionUrl,
    IssuerUrl, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier,
    PostLogoutRedirectUrl, RedirectUrl, Scope, TokenResponse, core::CoreAuthenticationFlow,
    reqwest,
};
use tokio::sync::RwLock;
use url::Url;

use crate::{
    claims::{EntraClient, EntraIdTokenClaims},
    config::OidcConfig,
    error::AuthError,
    session::LoginFlow,
};

/// How long discovery metadata and JWKS are reused before being refetched.
pub const METADATA_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// Timeout for every outbound call to the identity provider.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Scopes requested in addition to `openid`, which `openidconnect` always adds.
///
/// `profile` yields the display name and `email` the address; both are optional
/// claims on the Entra ID app registration.
const SCOPES: [&str; 2] = ["profile", "email"];

/// Cached provider metadata together with a ready-to-use client.
struct Metadata {
    /// Client configured from the discovery document, with the JWKS embedded.
    client: EntraClient,
    /// RP-initiated logout endpoint, when the provider advertises one.
    end_session_endpoint: Option<EndSessionUrl>,
    /// When this entry was fetched.
    fetched_at: Instant,
}

impl Metadata {
    /// Returns `true` when the cached entry is older than [`METADATA_TTL`].
    fn is_stale(&self) -> bool {
        self.fetched_at.elapsed() >= METADATA_TTL
    }
}

/// Relying party that performs sign-in and sign-out against Entra ID.
pub struct OidcProvider {
    config: OidcConfig,
    http: reqwest::Client,
    metadata: RwLock<Option<Arc<Metadata>>>,
}

impl std::fmt::Debug for OidcProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcProvider")
            .field("tenant_id", &self.config.tenant_id)
            .field("client_id", &self.config.client_id)
            .finish_non_exhaustive()
    }
}

impl OidcProvider {
    /// Builds a provider from configuration.
    ///
    /// No network call is made here; discovery happens on first use so that a
    /// brief Entra ID outage cannot prevent the process from starting.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration is invalid or the HTTP client
    /// cannot be constructed.
    pub fn new(config: OidcConfig) -> Result<Self, AuthError> {
        config.validate()?;

        // Redirects are disabled deliberately: following them from a token or
        // JWKS request would open the client up to SSRF.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| AuthError::Discovery(Box::new(error)))?;

        Ok(Self {
            config,
            http,
            metadata: RwLock::new(None),
        })
    }

    /// Returns the configuration this provider was built from.
    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    /// Returns cached metadata, refreshing it when absent or stale.
    async fn metadata(&self) -> Result<Arc<Metadata>, AuthError> {
        if let Some(cached) = self.metadata.read().await.as_ref()
            && !cached.is_stale()
        {
            return Ok(Arc::clone(cached));
        }

        let mut guard = self.metadata.write().await;
        // Another task may have refreshed while we waited for the write lock.
        if let Some(cached) = guard.as_ref()
            && !cached.is_stale()
        {
            return Ok(Arc::clone(cached));
        }

        let fresh = Arc::new(self.discover().await?);
        *guard = Some(Arc::clone(&fresh));
        Ok(fresh)
    }

    /// Fetches the discovery document and the JWKS, and builds a client.
    async fn discover(&self) -> Result<Metadata, AuthError> {
        let issuer = IssuerUrl::new(self.config.issuer_url())
            .map_err(|error| AuthError::InvalidUrl(error.to_string()))?;

        tracing::debug!(issuer = %issuer.as_str(), "fetching OpenID Connect metadata");

        let discovered =
            openidconnect::ProviderMetadataWithLogout::discover_async(issuer, &self.http)
                .await
                .map_err(|error| AuthError::Discovery(Box::new(error)))?;

        let end_session_endpoint = discovered
            .additional_metadata()
            .end_session_endpoint
            .clone();

        let redirect = RedirectUrl::new(self.config.redirect_uri.clone())
            .map_err(|error| AuthError::InvalidUrl(error.to_string()))?;

        let client = EntraClient::from_provider_metadata(
            discovered,
            ClientId::new(self.config.client_id.clone()),
            self.config.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);

        Ok(Metadata {
            client,
            end_session_endpoint,
            fetched_at: Instant::now(),
        })
    }

    /// Starts a sign-in and returns where to send the browser.
    ///
    /// The returned [`LoginFlow`] holds the PKCE verifier, the CSRF state and
    /// the nonce. It must be stored server-side in the session and never put in
    /// a URL or a cookie the client can read.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery fails.
    pub async fn begin_login(&self, return_to: String) -> Result<(Url, LoginFlow), AuthError> {
        let metadata = self.metadata().await?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();

        let mut request = metadata.client.authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        );
        for scope in SCOPES {
            request = request.add_scope(Scope::new(scope.to_owned()));
        }

        let (url, state, nonce) = request.set_pkce_challenge(challenge).url();

        let flow = LoginFlow {
            pkce_verifier: verifier.into_secret(),
            state: state.into_secret(),
            nonce: nonce.secret().clone(),
            return_to,
        };
        Ok((url, flow))
    }

    /// Completes a sign-in: verifies `state`, exchanges the code, validates the
    /// ID token and returns its verified claims plus the raw token.
    ///
    /// The raw ID token is returned so it can be passed back as `id_token_hint`
    /// on sign-out; it must be treated as a credential.
    ///
    /// # Errors
    ///
    /// Returns an error when `state` does not match, the code exchange fails,
    /// or the ID token fails any verification step.
    pub async fn complete_login(
        &self,
        flow: &LoginFlow,
        code: String,
        state: &str,
    ) -> Result<(EntraIdTokenClaims, String), AuthError> {
        if !constant_time_eq(flow.state.as_bytes(), state.as_bytes()) {
            return Err(AuthError::StateMismatch);
        }

        let metadata = self.metadata().await?;

        let token_response = metadata
            .client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|error| AuthError::TokenExchange(Box::new(error)))?
            .set_pkce_verifier(PkceCodeVerifier::new(flow.pkce_verifier.clone()))
            .request_async(&self.http)
            .await
            .map_err(|error| AuthError::TokenExchange(Box::new(error)))?;

        let id_token = token_response.id_token().ok_or(AuthError::MissingIdToken)?;

        // Verifies the signature against the cached JWKS, plus issuer,
        // audience, nonce and expiry.
        let verifier = metadata.client.id_token_verifier();
        let nonce = Nonce::new(flow.nonce.clone());
        let claims = id_token
            .claims(&verifier, &nonce)
            .map_err(|error| AuthError::InvalidIdToken(Box::new(error)))?;

        // Guards against an access token from a different response being
        // substituted for this one.
        if let Some(expected) = claims.access_token_hash() {
            let signing_alg = id_token
                .signing_alg()
                .map_err(|error| AuthError::InvalidIdToken(Box::new(error)))?;
            let signing_key = id_token
                .signing_key(&verifier)
                .map_err(|error| AuthError::InvalidIdToken(Box::new(error)))?;
            let actual = AccessTokenHash::from_token(
                token_response.access_token(),
                signing_alg,
                signing_key,
            )
            .map_err(|error| AuthError::InvalidIdToken(Box::new(error)))?;
            if actual != *expected {
                return Err(AuthError::AccessTokenMismatch);
            }
        }

        Ok((claims.clone(), id_token.to_string()))
    }

    /// Returns the URL that ends the session at Entra ID.
    ///
    /// Falls back to the local post-logout URL when the provider advertises no
    /// end-session endpoint: the local session is already gone at that point,
    /// so the user is signed out of this application either way.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery fails or the post-logout URL is invalid.
    pub async fn logout_url(&self, id_token_hint: Option<&str>) -> Result<Url, AuthError> {
        let metadata = self.metadata().await?;

        let post_logout = PostLogoutRedirectUrl::new(self.config.post_logout_redirect_uri.clone())
            .map_err(|error| AuthError::InvalidUrl(error.to_string()))?;

        let Some(endpoint) = metadata.end_session_endpoint.clone() else {
            tracing::warn!("provider advertises no end_session_endpoint; staying local");
            return Url::parse(&self.config.post_logout_redirect_uri)
                .map_err(|error| AuthError::InvalidUrl(error.to_string()));
        };

        let request = openidconnect::LogoutRequest::from(endpoint)
            .set_client_id(ClientId::new(self.config.client_id.clone()))
            .set_post_logout_redirect_uri(post_logout);

        let mut url = request.http_get_url();
        if let Some(hint) = id_token_hint {
            // Passing the already-verified raw token through the query
            // parameter avoids re-parsing a token we just validated.
            url.query_pairs_mut().append_pair("id_token_hint", hint);
        }
        Ok(url)
    }
}

/// Compares two byte strings without leaking their contents through timing.
///
/// Used for the `state` parameter, which is a secret shared between the session
/// and the redirect. A short-circuiting `==` would reveal the matching prefix
/// length to an attacker able to measure response times.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_equality() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn issuer_url_follows_the_entra_v2_pattern() {
        let config = OidcConfig {
            tenant_id: "9f08805a-b141-4a2d-923f-80d0576d602a".to_owned(),
            client_id: "client".to_owned(),
            client_secret: None,
            redirect_uri: "http://localhost:8080/auth/callback".to_owned(),
            post_logout_redirect_uri: "http://localhost:8080/".to_owned(),
            required_app_role: None,
        };
        assert_eq!(
            config.issuer_url(),
            "https://login.microsoftonline.com/9f08805a-b141-4a2d-923f-80d0576d602a/v2.0"
        );
    }
}
