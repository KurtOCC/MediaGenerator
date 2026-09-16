//! Mediagenerator server entry point.
//!
//! Reads configuration from the environment, installs tracing, builds the
//! router and serves it until a shutdown signal arrives.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Context as _;
use mediagenerator_web::{AppConfig, AppState, build_router, telemetry};
use tokio::{net::TcpListener, signal};

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

    let state = AppState::new(config);
    let router = build_router(state);

    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;

    tracing::info!(
        %address,
        %base_url,
        environment = ?environment,
        version = env!("CARGO_PKG_VERSION"),
        "mediagenerator started"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("mediagenerator stopped");
    Ok(())
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
