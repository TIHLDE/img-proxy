pub mod config;
pub mod proxy;
pub mod request;
pub mod sign;

use std::{sync::Arc, time::Duration};

use axum::{Router, routing::get};

/// Builds the router with the health endpoint and the proxy handler.
///
/// # Errors
///
/// Returns an error if the HTTP client for imagor cannot be created.
pub fn app(cfg: config::Config) -> Result<Router, reqwest::Error> {
    // The reqwest handler for the axum state
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    // Register a health endpoint and the proxy handler
    Ok(Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback(proxy::handle)
        .with_state(Arc::new(proxy::AppState { cfg, client })))
}
