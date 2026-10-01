pub mod config;
pub mod proxy;
pub mod request;
pub mod sign;

use std::{sync::Arc, time::Duration};

use axum::{Router, routing::get};

pub fn app(cfg: config::Config) -> Router {
    // The reqwest handler for the axum state
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("reqwest client");

    // Register a health endpoint and the proxy handler
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .fallback(proxy::handle)
        .with_state(Arc::new(proxy::AppState { cfg, client }))
}
