//! Shared helpers for the web crate end-to-end tests.
//!
//! Each integration test binary compiles this module separately, so items used
//! by only one of them would otherwise be reported as dead code.

#![allow(dead_code)]

use std::sync::Arc;

use axum::Router;
use mediagenerator_providers::Providers;
use mediagenerator_storage::BlobStore;
use mediagenerator_storage::Database;
use mediagenerator_web::{
    AppConfig, AppState, build_router,
    config::{Environment, Secret, SessionStore},
    jobs::{Jobs, Runtime},
    session::session_layer,
};
use sqlx::postgres::PgPool;
use tower_sessions::MemoryStore;

/// Builds a configuration that is valid but points at nothing real.
///
/// The Entra ID and Azure endpoints are placeholders: no test in this phase
/// reaches the network, because every test either stops at a guard or at a
/// probe endpoint.
pub fn test_config() -> AppConfig {
    AppConfig {
        app_base_url: "http://localhost:8080".to_owned(),
        app_port: 0,
        app_env: Environment::Development,
        rust_log: "warn".to_owned(),
        // Tests run with the crate directory as the working directory, so the
        // relative default would not find the repository-level assets.
        assets_dir: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").to_owned(),
        database_url: Secret::new("postgres://localhost/mediagenerator_test"),
        session_secret: Secret::new("x".repeat(64)),
        session_store: SessionStore::Memory,
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

/// Opens a pool that never connects unless a query is run.
///
/// The tests in this crate stop at a guard, at the origin check or at a probe
/// endpoint, so none of them reaches the database. A lazy pool keeps the suite
/// runnable with no server anywhere, including in CI.
fn lazy_pool(config: &AppConfig) -> Database {
    PgPool::connect_lazy(config.database_url.expose())
        .expect("the test connection string should parse")
}

/// Builds application state backed by [`test_config`].
///
/// Must be called from inside a Tokio runtime: starting the job queue spawns
/// the worker tasks.
pub fn test_state() -> AppState {
    let config = test_config();
    let db = lazy_pool(&config);
    let providers = Arc::new(Providers::new(config.providers()).expect("providers should build"));
    let blobs = Arc::new(
        BlobStore::new(
            &config.azure_storage_account,
            &config.azure_storage_container,
        )
        .expect("blob client should build"),
    );
    let jobs = Jobs::spawn(Runtime {
        db: db.clone(),
        providers: Arc::clone(&providers),
        blobs: Arc::clone(&blobs),
    });
    AppState::new(config, db, providers, blobs, jobs).expect("test state should build")
}

/// Builds the full router with an in-memory session store.
pub fn test_router() -> Router {
    router_with(test_config())
}

/// Builds the full router from a caller-supplied configuration.
pub fn router_with(config: AppConfig) -> Router {
    let layer = session_layer(&config, MemoryStore::default())
        .expect("test session secret should be long enough");
    let db = lazy_pool(&config);
    let providers = Arc::new(Providers::new(config.providers()).expect("providers should build"));
    let blobs = Arc::new(
        BlobStore::new(
            &config.azure_storage_account,
            &config.azure_storage_container,
        )
        .expect("blob client should build"),
    );
    let jobs = Jobs::spawn(Runtime {
        db: db.clone(),
        providers: Arc::clone(&providers),
        blobs: Arc::clone(&blobs),
    });
    let state = AppState::new(config, db, providers, blobs, jobs).expect("test state should build");
    build_router(state, layer)
}
