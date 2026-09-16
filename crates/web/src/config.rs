//! Application configuration, read from the environment (12-factor).
//!
//! Every setting is supplied as an environment variable; see `.env.example` for
//! the full list. Values are read once at start-up through [`figment`] and then
//! shared immutably. Missing required values are a start-up error — the process
//! must never boot half-configured.

use std::fmt;

use figment::{
    Figment,
    providers::{Env, Serialized},
};
use serde::{Deserialize, Serialize};

/// Minimum accepted length of `SESSION_SECRET`, in bytes.
///
/// Cookie signing keys must carry at least 512 bits of entropy.
const MIN_SESSION_SECRET_LEN: usize = 64;

/// Which deployment environment the process is running in.
///
/// Controls log format (human-readable vs JSON) and cookie hardening.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    /// Local development: pretty logs, `Secure` cookies relaxed for `http://localhost`.
    Development,
    /// Production: JSON logs, HSTS and `Secure` cookies enforced.
    Production,
}

impl Environment {
    /// Returns `true` when running in production.
    pub const fn is_production(self) -> bool {
        matches!(self, Self::Production)
    }
}

/// Where session state is persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStore {
    /// PostgreSQL, shared across instances and surviving a restart.
    Postgres,
    /// In-process memory. Development only: sessions are lost on restart and
    /// are not shared between replicas.
    Memory,
}

/// A configuration value that must never be logged or rendered.
///
/// `Debug` is redacted, so a secret cannot leak through `tracing` output or a
/// `{:?}` of the whole [`AppConfig`].
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value as a secret.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the underlying value. Call this only where the secret is used.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Returns the length of the secret without revealing it.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` when the secret is the empty string.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The complete, validated application configuration.
///
/// Field names map one-to-one to the upper-case environment variable of the
/// same name, e.g. `app_base_url` ← `APP_BASE_URL`.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// Public base URL the application is served from, without a trailing slash.
    pub app_base_url: String,
    /// TCP port to bind. Default: `8080`.
    pub app_port: u16,
    /// Deployment environment. Default: `development`.
    pub app_env: Environment,
    /// `tracing` filter directives. Default: `info`.
    pub rust_log: String,
    /// Directory served at `/assets`, relative to the working directory.
    /// Default: `assets`.
    pub assets_dir: String,

    /// PostgreSQL connection string used by sqlx and the session store.
    pub database_url: Secret,
    /// Key used to sign session cookies; at least 64 bytes.
    pub session_secret: Secret,
    /// Where sessions are stored. Default: `postgres`.
    pub session_store: SessionStore,

    /// Entra ID directory (tenant) ID.
    pub azure_tenant_id: String,
    /// Entra ID application (client) ID.
    pub azure_client_id: String,
    /// Entra ID client secret. Omit when using Managed Identity.
    pub azure_client_secret: Option<Secret>,
    /// Redirect URI registered on the Entra ID app registration.
    pub oidc_redirect_uri: String,
    /// App role required to use the application. Empty or unset disables the check.
    pub required_app_role: Option<String>,

    /// Azure OpenAI / AI Foundry resource endpoint, without a trailing slash.
    pub azure_openai_endpoint: String,
    /// `api-version` query parameter used for every Azure OpenAI call.
    pub azure_openai_api_version: String,
    /// Deployment name for image generation.
    pub image_deployment: String,
    /// Deployment name for text-to-speech.
    pub audio_deployment: String,
    /// Deployment name for video generation (Sora).
    pub video_deployment: String,
    /// Optional API key, accepted for local development only.
    pub azure_openai_api_key: Option<Secret>,

    /// Azure Storage account name holding generated media.
    pub azure_storage_account: String,
    /// Private blob container name.
    pub azure_storage_container: String,
    /// Lifetime of generated SAS URLs, in minutes. Default: `60`.
    pub sas_ttl_minutes: u64,

    /// Maximum accepted prompt length, in characters. Default: `4000`.
    pub max_prompt_chars: usize,
    /// Generations allowed per user per rolling hour. Default: `20`.
    pub rate_limit_per_hour: u32,
    /// Days prompts and media are retained before cleanup. Default: `90`.
    pub retention_days: u32,
}

impl AppConfig {
    /// Loads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns an error when a required variable is missing, has the wrong type
    /// or fails a semantic check (see [`AppConfig::validate`]).
    pub fn from_env() -> Result<Self, ConfigError> {
        let config: Self = Figment::new()
            .merge(Serialized::default("app_port", 8080_u16))
            .merge(Serialized::default("app_env", "development"))
            .merge(Serialized::default("rust_log", "info"))
            .merge(Serialized::default("assets_dir", "assets"))
            .merge(Serialized::default("sas_ttl_minutes", 60_u64))
            .merge(Serialized::default("max_prompt_chars", 4000_usize))
            .merge(Serialized::default("rate_limit_per_hour", 20_u32))
            .merge(Serialized::default("retention_days", 90_u32))
            .merge(Serialized::default("azure_storage_container", "media"))
            .merge(Serialized::default("session_store", "postgres"))
            .merge(Env::raw())
            .extract()
            .map_err(Box::new)?;

        config.validate()?;
        Ok(config)
    }

    /// Checks invariants that the type system cannot express.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] with a description of the first problem.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.session_secret.len() < MIN_SESSION_SECRET_LEN {
            return Err(ConfigError::Invalid(format!(
                "SESSION_SECRET must be at least {MIN_SESSION_SECRET_LEN} bytes"
            )));
        }
        if self.app_base_url.ends_with('/') {
            return Err(ConfigError::Invalid(
                "APP_BASE_URL must not end with a trailing slash".to_owned(),
            ));
        }
        if self.azure_openai_endpoint.ends_with('/') {
            return Err(ConfigError::Invalid(
                "AZURE_OPENAI_ENDPOINT must not end with a trailing slash".to_owned(),
            ));
        }
        if self.max_prompt_chars == 0 {
            return Err(ConfigError::Invalid(
                "MAX_PROMPT_CHARS must be greater than zero".to_owned(),
            ));
        }
        if self.app_env.is_production() && self.azure_openai_api_key.is_some() {
            return Err(ConfigError::Invalid(
                "AZURE_OPENAI_API_KEY is only allowed outside production; use Managed Identity"
                    .to_owned(),
            ));
        }
        if self.app_env.is_production() && self.session_store == SessionStore::Memory {
            return Err(ConfigError::Invalid(
                "SESSION_STORE=memory is not allowed in production; sessions would be lost on \
                 restart and unshared between replicas"
                    .to_owned(),
            ));
        }
        if !self.oidc_redirect_uri.starts_with(&self.app_base_url) {
            return Err(ConfigError::Invalid(
                "OIDC_REDIRECT_URI must live under APP_BASE_URL".to_owned(),
            ));
        }
        Ok(())
    }

    /// Returns the OIDC settings for the auth crate.
    pub fn oidc(&self) -> mediagenerator_auth::OidcConfig {
        mediagenerator_auth::OidcConfig {
            tenant_id: self.azure_tenant_id.clone(),
            client_id: self.azure_client_id.clone(),
            client_secret: self
                .azure_client_secret
                .as_ref()
                .map(|secret| secret.expose().to_owned()),
            redirect_uri: self.oidc_redirect_uri.clone(),
            post_logout_redirect_uri: format!("{}/", self.app_base_url),
            required_app_role: self.required_app_role.clone(),
        }
    }

    /// Returns the required app role, treating an empty string as "not set".
    pub fn required_app_role(&self) -> Option<&str> {
        self.required_app_role
            .as_deref()
            .map(str::trim)
            .filter(|role| !role.is_empty())
    }
}

/// Errors raised while loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A variable was missing or could not be parsed.
    #[error("could not read configuration from the environment")]
    Read(#[from] Box<figment::Error>),

    /// A value was present but semantically invalid.
    #[error("invalid configuration: {0}")]
    Invalid(String),
}
