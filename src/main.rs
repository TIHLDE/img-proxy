use std::process::ExitCode;

use img_proxy::{app, config::Config};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("config error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(cfg.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(%err, listen = %cfg.listen, "bind failed");
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(listen = %cfg.listen, imagor = cfg.imagor_url, "img-proxy started");

    let router = match app(cfg) {
        Ok(router) => router,
        Err(err) => {
            tracing::error!(%err, "failed to create HTTP client");
            return ExitCode::FAILURE;
        }
    };

    // Start the webserver
    if let Err(err) = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(%err, "server error");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// Resolves on Ctrl-C or SIGTERM.
async fn shutdown_signal() {
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(err) => {
            tracing::warn!(%err, "SIGTERM handler unavailable, only listening for Ctrl-C");
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}
