use img_proxy::{app, config::Config};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cfg = Config::from_env().unwrap_or_else(|err| {
        eprintln!("config error: {err}");
        std::process::exit(1);
    });

    let listener = tokio::net::TcpListener::bind(cfg.listen)
        .await
        .expect("bind");

    tracing::info!(listen = %cfg.listen, imagor = cfg.imagor_url, "img-proxy started");

    // Start the webserver
    axum::serve(listener, app(cfg))
        .with_graceful_shutdown(async {
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        })
        .await
        .expect("server");
}
