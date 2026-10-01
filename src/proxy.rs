use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    config::Config,
    request::{Access, ImageRequest},
};

pub struct AppState {
    pub cfg: Config,
    pub client: reqwest::Client,
}

/// Request headers forwarded to imagor (content negotiation and revalidation).
const FORWARD_REQUEST: [header::HeaderName; 3] = [
    header::ACCEPT,
    header::IF_NONE_MATCH,
    header::IF_MODIFIED_SINCE,
];

/// Response headers forwarded from imagor. Everything else is dropped.
const FORWARD_RESPONSE: [header::HeaderName; 8] = [
    header::CONTENT_TYPE,
    header::CONTENT_DISPOSITION,
    header::CONTENT_LENGTH,
    header::ETAG,
    header::LAST_MODIFIED,
    header::CACHE_CONTROL,
    header::EXPIRES,
    header::VARY,
];

/// Content types allowed out of the proxy. imagor sniffs these from the file's
/// magic bytes, so a PDF renamed to `.jpg` is still caught on `?original`.
/// SVG is excluded on purpose: it can carry scripts.
const IMAGE_CONTENT_TYPES: [&str; 5] = [
    "image/jpeg",
    "image/png",
    "image/webp",
    "image/avif",
    "image/gif",
];

pub async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response {
    // Dont allow other methods than GET and HEAD
    match *req.method() {
        Method::GET | Method::HEAD => {}
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Create a image request for Imagor
    let image = match ImageRequest::parse(&state.cfg, req.uri().path(), req.uri().query(), now) {
        Ok(image) => image,
        Err(reject) => {
            let reason = reject.canonical_reason();
            tracing::debug!(path = req.uri().path(), ?reason, "rejected");

            return reject.into_response();
        }
    };

    // Format the imagor url
    let url = format!("{}{}", state.cfg.imagor_url, image.imagor_path(&state.cfg));

    let mut upstream = state.client.request(req.method().clone(), &url);

    // Add the supported headers to the Imagor
    for name in FORWARD_REQUEST {
        if let Some(value) = req.headers().get(&name) {
            upstream = upstream.header(name, value);
        }
    }

    // Send the request
    let resp = match upstream.send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::error!(%err, "imagor request failed");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let status = resp.status();
    if status.is_server_error() {
        tracing::error!(%status, url, "imagor error");
        return StatusCode::BAD_GATEWAY.into_response();
    }

    if status.is_client_error() {
        // imagor error bodies can contain bucket names and paths; don't leak them.
        tracing::debug!(%status, url, "imagor rejected");
        return status.into_response();
    }

    // Reject media types not valid in IMAGE_CONTENT_TYPES
    if status == StatusCode::OK {
        let content_type = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim);
        if !content_type.is_some_and(|ct| IMAGE_CONTENT_TYPES.contains(&ct)) {
            tracing::warn!(?content_type, url, "refusing non-image response");
            return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
        }
    }

    let mut out = Response::builder().status(status);
    let headers = out.headers_mut().expect("builder has no errors yet");

    // Forward the imagor headers for cache control etc...
    for name in FORWARD_RESPONSE {
        if let Some(value) = resp.headers().get(&name) {
            headers.insert(name, value.clone());
        }
    }

    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );

    // `?original` serves uploads byte for byte, which may be SVG or worse.
    // Nothing served from this origin should ever run scripts.
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; sandbox"),
    );

    // Set the max age for the image if the image is private
    if let Access::Private { exp } = image.access {
        // Never let shared caches keep private images, and never past `exp`.
        let max_age = exp.saturating_sub(now).min(state.cfg.private_max_age);
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_str(&format!("private, max-age={max_age}")).expect("valid header"),
        );
        headers.remove(header::EXPIRES);
    }

    // Forward the byte stream
    out.body(Body::from_stream(resp.bytes_stream()))
        .expect("valid response")
}
