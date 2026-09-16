//! Mediagenerator server entry point.
//!
//! Reads configuration from the environment, installs tracing, opens the
//! database, starts the job workers, builds the router and serves it until a
//! shutdown signal arrives.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context as _;
use mediagenerator_web::{
    AppConfig, AppState, build_router, config::SessionStore as SessionStoreKind, jobs::Jobs,
    session::session_layer, telemetry,
};
use tokio::{net::TcpListener, signal};
use tower_sessions::MemoryStore;
use tower_sessions_sqlx_store::PostgresStore;

/// Connection pool size.
///
/// Sized for the Burstable tier the development database runs on, which allows
/// far fewer connections than a larger SKU.
const POOL_SIZE: u32 = 10;

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

    // Opens the pool and applies the migrations embedded in the binary.
    let db = mediagenerator_storage::connect(config.database_url.expose(), POOL_SIZE)
        .await
        .context("failed to open the database")?;

    // Anything left running by a previous process will never be finished by
    // this one, so it is failed now rather than left spinning forever.
    mediagenerator_web::jobs::fail_orphans(&db)
        .await
        .context("failed to sweep interrupted jobs")?;

    let jobs = Jobs::spawn(db.clone());

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

    // The two session stores are different types, so the router is built inside
    // each branch rather than behind a trait object.
    match store_kind {
        SessionStoreKind::Postgres => {
            let store = PostgresStore::new(db.clone());
            store
                .migrate()
                .await
                .context("failed to migrate the session table")?;

            let layer = session_layer(&config, store).context("failed to build session layer")?;
            let state =
                AppState::new(config, db, jobs).context("failed to build application state")?;
            serve(listener, build_router(state, layer)).await?;
        }
        SessionStoreKind::Memory => {
            tracing::warn!(
                "using the in-memory session store: sessions are lost on restart and are not \
                 shared between instances"
            );
            let layer = session_layer(&config, MemoryStore::default())
                .context("failed to build session layer")?;
            let state =
                AppState::new(config, db, jobs).context("failed to build application state")?;
            serve(listener, build_router(state, layer)).await?;
        }
    }

    tracing::info!("mediagenerator stopped");
    Ok(())
}

/// Serves `router` on `listener` until a shutdown signal arrives.
///
/// `into_make_service_with_connect_info` is what makes the peer address
/// available to the audit trail.
async fn serve(listener: TcpListener, router: axum::Router) -> anyhow::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
