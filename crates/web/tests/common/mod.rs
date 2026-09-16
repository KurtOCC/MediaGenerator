//! Shared helpers for the web crate's end-to-end tests.
//!
//! Each integration test binary compiles this module separately, so items used
//! by only one of them would otherwise be reported as dead code.

#![allow(dead_code)]

use mediagenerator_web::{
    AppConfig, AppState,
    config::{Environment, Secret},
};

/// Builds a configuration that is valid but points at nothing real.
///
/// Phase 1 exercises only routes that never touch Azure or the database, so
/// placeholder endpoints are sufficient; later phases replace the relevant
/// fields with test doubles.
pub fn test_config() -> AppConfig {
    AppConfig {
        app_base_url: "http://localhost:8080".to_owned(),
        app_port: 0,
        app_env: Environment::Development,
        rust_log: "warn".to_owned(),
        assets_dir: "assets".to_owned(),
        database_url: Secret::new("postgres://localhost/mediagenerator_test"),
        session_secret: Secret::new("x".repeat(64)),
        azure_tenant_id: "00000000-0000-0000-0000-000000000000".to_owned(),
        azure_client_id: "00000000-0000-0000-0000-000000000001".to_owned(),
        azure_client_secret: None,
        oidc_redirect_uri: "http://localhost:8080/auth/callback".to_owned(),
        required_app_role: None,
        azure_openai_endpoint: "https://example.openai.azure.com".to_owned(),
        azure_openai_api_version: "2024-10-21".to_owned(),
        image_deployment: "gpt-image-1".to_owned(),
        audio_deployment: "gpt-4o-mini-tts".to_owned(),
        video_deployment: "sora".to_owned(),
        azure_openai_api_key: None,
        azure_storage_account: "examplestorage".to_owned(),
        azure_storage_container: "media".to_owned(),
        sas_ttl_minutes: 60,
        max_prompt_chars: 4000,
        rate_limit_per_hour: 20,
        retention_days: 90,
    }
}

/// Builds application state backed by [`test_config`].
pub fn test_state() -> AppState {
    AppState::new(test_config())
}
