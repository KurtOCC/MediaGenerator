//! Mediagenerator server entry point.
//!
//! Reads configuration from the environment, installs tracing, opens the
//! session store, builds the router and serves it until a shutdown signal
//! arrives.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context as _;
use mediagenerator_web::{
    AppConfig, AppState, build_router, config::SessionStore as SessionStoreKind,
    session::session_layer, telemetry,
};
use tokio::{net::TcpListener, signal};
use tower_sessions::MemoryStore;
use tower_sessions_sqlx_store::PostgresStore;

/// Connection pool size for the session store.
///
/// Sessions are read once per request, so a small pool is enough; phase 4 adds
/// a separate, larger pool for the application tables.
const SESSION_POOL_SIZE: u32 = 5;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // A .env file is a local-development convenience only; in Azure the values
    // come from Container Apps secrets backed by Key Vault.
    let _ = dotenvy::dotenv();

    let config = AppConfig::from_env().context("failed to load configuration")?;
    telemetry::init(&config.rust_log, config.app_env).context("failed to install tracing")?;

    let address = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.app_port));
    let base_url = config.app_base_url.clone();
    let environment = config.app_env;
    let store_kind = config.session_store;

    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;

    tracing::info!(
        %address,
        %base_url,
        environment = ?environment,
        session_store = ?store_kind,
        version = env!("CARGO_PKG_VERSION"),
        "mediagenerator started"
    );

    // The two stores are different types, so the router is built inside each
    // branch rather than behind a trait object.
    match store_kind {
        SessionStoreKind::Postgres => {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(SESSION_POOL_SIZE)
                .connect(config.database_url.expose())
                .await
                .context("failed to connect to the session database")?;

            let store = PostgresStore::new(pool);
            store
                .migrate()
                .await
                .context("failed to migrate the session table")?;

            let layer = session_layer(&config, store).context("failed to build session layer")?;
            let state = AppState::new(config).context("failed to build application state")?;
            serve(listener, build_router(state, layer)).await?;
        }
        SessionStoreKind::Memory => {
            tracing::warn!(
                "using the in-memory session store: sessions are lost on restart and are not \
                 shared between instances"
            );
            let layer = session_layer(&config, MemoryStore::default())
                .context("failed to build session layer")?;
            let state = AppState::new(config).context("failed to build application state")?;
            serve(listener, build_router(state, layer)).await?;
        }
    }

    tracing::info!("mediagenerator stopped");
    Ok(())
}

/// Serves `router` on `listener` until a shutdown signal arrives.
async fn serve(listener: TcpListener, router: axum::Router) -> anyhow::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")
}

/// Resolves when the process is asked to shut down.
///
/// Container Apps sends `SIGTERM`; `Ctrl+C` is handled for local runs. In-flight
/// requests are allowed to finish before the listener closes.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = signal::ctrl_c().await {
            tracing::error!(%error, "failed to listen for Ctrl+C");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to listen for SIGTERM"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => tracing::info!("received Ctrl+C, shutting down"),
        () = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}
